//! 根管理员密钥管理（安全分区）与铸造。
//!
//! - 每个根管理员一把 gpg 密钥（AES 上锁存 admins 表）
//! - 铸造（Mint）只可打入 PreIssuedAccount，需 root 先解锁自己的密钥
//! - 密钥可导出到服务端数据目录（~/.alpha_dir/acs-server）

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;

use acs_core::models::{AccountType, Transaction, TransactionType};
use acs_core::transaction;

use crate::api::audit::log_audit;
use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::{AppState, CentralState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/keys/status", get(key_status))
        .route("/api/admin/keys/unlock", post(key_unlock))
        .route("/api/admin/keys/lock", post(key_lock))
        .route("/api/admin/keys/export", post(key_export))
        .route("/api/admin/mint", post(mint))
}

async fn key_status(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let exists: bool = !conn
        .query_row(
            "SELECT COALESCE(pubkey,'') FROM admins WHERE id=?1",
            params![auth.admin_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default()
        .is_empty();
    let central = st.central.lock().unwrap();
    let unlocked = central.admin_uid.as_deref() == Some(auth.username.as_str()) && central.fingerprint.is_some();
    Ok(Json(json!({ "exists": exists, "unlocked": unlocked })))
}

#[derive(Deserialize)]
pub struct PwdReq {
    pub password: String,
}

async fn key_unlock(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<PwdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可解锁铸造密钥"));
    }
    let conn = st.db.lock().unwrap();
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT fingerprint, encrypted_seckey, key_passphrase_enc FROM admins WHERE id=?1",
            params![auth.admin_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(ApiErr::from_err)?;
    let Some((fp, secret, key_enc)) = row else {
        return Err(ApiErr::unauthorized("账号异常"));
    };
    if fp.is_empty() {
        return Err(ApiErr::bad_request("尚未生成密钥，请联系系统初始化"));
    }
    // 用登录密码解出密钥 passphrase
    let key_pass = crate::crypto::decrypt_secret(&key_enc, &req.password)
        .map_err(|_| ApiErr::forbidden("密码错误"))?;
    // 导入私钥并验证
    st.gpg.import_key(&secret).map_err(ApiErr::from)?;
    st.gpg.verify_passphrase(&fp, &key_pass).map_err(|_| ApiErr::forbidden("密钥密码校验失败"))?;
    *st.central.lock().unwrap() = CentralState {
        admin_uid: Some(auth.username.clone()),
        fingerprint: Some(fp.clone()),
        passphrase: Some(key_pass),
    };
    log_audit(&conn, &auth.username, "key_unlock", "");
    Ok(Json(json!({ "ok": true, "fingerprint": fp, "unlocked": true })))
}

async fn key_lock(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let mut central = st.central.lock().unwrap();
    if central.admin_uid.as_deref() == Some(auth.username.as_str()) {
        *central = CentralState::default();
    }
    Ok(Json(json!({ "ok": true, "unlocked": false })))
}

/// 导出本人密钥（armored 私钥）到服务端数据目录 <uid>.asc（~/.alpha_dir/acs-server）。
async fn key_export(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<PwdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT fingerprint, key_passphrase_enc FROM admins WHERE id=?1",
            params![auth.admin_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(ApiErr::from_err)?;
    let Some((fp, key_enc)) = row else {
        return Err(ApiErr::unauthorized("账号异常"));
    };
    let key_pass = crate::crypto::decrypt_secret(&key_enc, &req.password)
        .map_err(|_| ApiErr::forbidden("密码错误"))?;
    let armored = st.gpg.export_secret_key(&fp, &key_pass).map_err(ApiErr::from)?;

    let dir = std::env::current_dir().unwrap_or_default().join("alpha_dir");
    std::fs::create_dir_all(&dir).map_err(ApiErr::from_err)?;
    let path = dir.join(format!("{}.asc", auth.username));
    std::fs::write(&path, &armored).map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "key_export", &format!("{}", path.display()));
    Ok(Json(json!({ "ok": true, "path": path.to_string_lossy().to_string() })))
}

#[derive(Deserialize)]
pub struct MintReq {
    pub amount: i64,
}

/// 铸造（Mint）：根管理员解锁自己的密钥后，向 PreIssuedAccount 增发 A€。
async fn mint(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<MintReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可铸造"));
    }
    if req.amount <= 0 {
        return Err(ApiErr::bad_request("金额须大于 0"));
    }
    let conn = st.db.lock().unwrap();
    let pre = acs_core::account::require_account(&conn, "PreIssuedAccount", AccountType::System)
        .map_err(ApiErr::from)?;

    let mut tx = Transaction::new(
        TransactionType::Mint,
        auth.username.clone(),
        AccountType::System,
        "PreIssuedAccount".into(),
        AccountType::System,
        req.amount,
    );
    tx.receiver_last_hash = pre.last_tx_hash.clone();
    tx.tx_hash = transaction::compute_tx_hash(&tx);
    tx.central_sig = Some(sign_hash(&st, &auth, &tx.tx_hash)?);
    drop(conn);

    let mut conn = st.db.lock().unwrap();
    transaction::submit_tx(&mut conn, &tx).map_err(ApiErr::from)?;
    let tx_id = tx.tx_id.clone();
    drop(conn);

    let conn = st.db.lock().unwrap();
    log_audit(&conn, &auth.username, "mint", &format!("+{} -> PreIssuedAccount", req.amount));
    Ok(Json(json!({ "ok": true, "tx_id": tx_id, "amount": req.amount })))
}

/// 用当前解锁的（本人）密钥对哈希签名。
fn sign_hash(st: &AppState, auth: &AuthUser, tx_hash: &str) -> ApiResult<String> {
    let cs = st.central.lock().unwrap();
    let (fp, pp) = match (&cs.fingerprint, &cs.passphrase) {
        (Some(f), Some(p)) if cs.admin_uid.as_deref() == Some(auth.username.as_str()) => (f.clone(), p.clone()),
        _ => return Err(ApiErr::forbidden("请先解锁本人的铸造密钥")),
    };
    let sig = st.gpg.sign_detached(&fp, &pp, tx_hash.as_bytes()).map_err(ApiErr::from)?;
    Ok(sig)
}

/// 若有任一解锁的根密钥，对哈希签名（镜像快照可选签名）。
pub fn try_sign_hash(st: &AppState, hash: &str) -> Option<String> {
    let cs = st.central.lock().unwrap();
    let (fp, pp) = (cs.fingerprint.as_ref()?, cs.passphrase.as_ref()?);
    st.gpg.sign_detached(fp, pp, hash.as_bytes()).ok()
}
