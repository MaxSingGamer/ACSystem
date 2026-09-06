//! 系统账本账户（Server 管理后台）：替代原客户端登录 System 账户。
//!
//! - 客户端已取消 System 账户登录；本模块让后台管理员“代管”某个系统账户
//!   （如 AESystem / AlphaEU / PreIssuedAccount），以该账户身份查看余额/流水、
//!   发起转账、确认/拒收。页面与客户端钱包一致，但密钥与签名由服务端完成。
//! - 代管会话：管理员 token -> 系统账户 uid（内存态，随后台会话/退出清除）。

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use acs_core::account;
use acs_core::models::{AccountStatus, AccountType, Transaction, TransactionType};
use acs_core::transaction;

use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/sys/list", get(sys_list))
        .route("/api/admin/sys/act", post(sys_act))
        .route("/api/admin/sys/state", get(sys_state))
        .route("/api/admin/sys/transfer", post(sys_transfer))
        .route("/api/admin/sys/confirm", post(sys_confirm))
        .route("/api/admin/sys/reject", post(sys_reject))
        .route("/api/admin/sys/logout", post(sys_logout))
}

/// 当前管理员正在代管的系统账户 uid（未登录则 None）。
fn acting_uid(st: &AppState, auth: &AuthUser) -> Option<String> {
    st.sys_acting.lock().unwrap().get(&auth.token).cloned()
}

/// 列出 Active 系统账户（供后台选择进入哪个账本）。
async fn sys_list(
    State(st): State<AppState>,
    _auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let _ = account::recompute_all_balances(&conn);
    let mut stmt = conn
        .prepare("SELECT uid, email, balance, status FROM accounts_system ORDER BY uid")
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "uid": r.get::<_, String>(0)?,
                "email": r.get::<_, String>(1)?,
                "balance": r.get::<_, i64>(2)?,
                "status": r.get::<_, String>(3)?,
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(ApiErr::from_err)?);
    }
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct ActReq {
    pub uid: String,
}

/// 后台管理员「进入」某个系统账本账户（建立代管会话）。
async fn sys_act(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ActReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let uid = req.uid.trim().to_string();
    if uid.is_empty() {
        return Err(ApiErr::bad_request("缺少系统账户 uid"));
    }
    let conn = st.db.lock().unwrap();
    let acc = account::get_account(&conn, &uid, AccountType::System)
        .map_err(ApiErr::from)?
        .ok_or_else(|| ApiErr::not_found("系统账户不存在"))?;
    if acc.status != AccountStatus::Active {
        return Err(ApiErr::forbidden("该系统账户非 Active，无法登录账本"));
    }
    st.sys_acting.lock().unwrap().insert(auth.token.clone(), uid.clone());
    crate::log::info(&format!(
        "系统账本登录：管理员 {} 代管系统账户 {}",
        auth.username, uid
    ));
    Ok(Json(json!({ "ok": true, "uid": uid, "email": acc.email, "atype": "System" })))
}

/// 当前代管系统账户的账本状态（余额自动重算 + 流水）。
async fn sys_state(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(uid) = acting_uid(&st, &auth) else {
        return Ok(Json(json!({ "logged_in": false })));
    };
    let conn = st.db.lock().unwrap();
    account::recompute_all_balances(&conn).map_err(ApiErr::from)?;
    let acc = account::get_account(&conn, &uid, AccountType::System)
        .map_err(ApiErr::from)?
        .ok_or_else(|| ApiErr::not_found("系统账户不存在"))?;
    let txs = transaction::list_transactions_for(&conn, &uid, AccountType::System)
        .map_err(ApiErr::from)?;
    let pending = transaction::list_pending_for(&conn, &uid, AccountType::System)
        .map_err(ApiErr::from)?;
    let now = chrono::Utc::now().timestamp();
    Ok(Json(json!({
        "logged_in": true,
        "uid": acc.uid,
        "atype": "System",
        "email": acc.email,
        "balance": acc.balance,
        "status": acc.status.as_str(),
        "synced_at": now,
        "txs": txs.iter().map(|t| {
            let is_in = t.receiver == uid;
            json!({
                "tx_id": t.tx_id, "tx_type": t.tx_type.as_str(),
                "peer": if is_in { &t.sender } else { &t.receiver },
                "peer_type": if is_in { t.sender_type.as_str() } else { t.receiver_type.as_str() },
                "amount": t.amount, "ts": t.timestamp,
                "status": t.status.as_str(), "direction": if is_in { 1 } else { -1 },
            })
        }).collect::<Vec<_>>(),
        "pending": pending.iter().map(|t| json!({
            "tx_id": t.tx_id, "tx_type": t.tx_type.as_str(), "sender": t.sender,
            "sender_type": t.sender_type.as_str(), "amount": t.amount, "timestamp": t.timestamp,
        })).collect::<Vec<_>>(),
        "server_url": format!("后台管理 · 系统账本 {}", uid),
    })))
}

#[derive(Deserialize)]
pub struct TransferReq {
    pub to: String,
    #[serde(rename = "type", default)]
    pub to_type: String,
    pub amount: i64,
}

/// 以代管系统账户身份发起转账（服务端构建、签名并提交，Pending 待接收方确认）。
async fn sys_transfer(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<TransferReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(uid) = acting_uid(&st, &auth) else {
        return Err(ApiErr::forbidden("请先在后台进入系统账本账户"));
    };
    let to = req.to.trim().to_string();
    let to_type = if req.to_type.trim().is_empty() {
        AccountType::Individual
    } else {
        AccountType::from_str(&req.to_type)
            .ok_or_else(|| ApiErr::bad_request("无效接收方类型"))?
    };
    if to.is_empty() {
        return Err(ApiErr::bad_request("请输入接收方"));
    }
    if req.amount <= 0 {
        return Err(ApiErr::bad_request("金额须大于 0"));
    }

    // 先重算余额，保证余额/链头判断准确
    {
        let conn = st.db.lock().unwrap();
        account::recompute_all_balances(&conn).map_err(ApiErr::from)?;
        let sender = account::get_account(&conn, &uid, AccountType::System)
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::not_found("系统账户不存在"))?;
        if sender.status != AccountStatus::Active {
            return Err(ApiErr::forbidden("系统账户非 Active"));
        }
        if !account::account_exists(&conn, &to, to_type).map_err(ApiErr::from)? {
            return Err(ApiErr::not_found(format!("接收方账户不存在: {to}")));
        }
    }

    // 组装交易（与服务端账本权威链头一致）
    let mut tx = Transaction::new(
        TransactionType::Transfer,
        uid.clone(),
        AccountType::System,
        to.clone(),
        to_type,
        req.amount,
    );
    {
        let conn = st.db.lock().unwrap();
        let sender = account::get_account(&conn, &uid, AccountType::System)
            .map_err(ApiErr::from)?
            .unwrap();
        let receiver = account::get_account(&conn, &to, to_type)
            .map_err(ApiErr::from)?
            .unwrap();
        tx.sender_last_hash = Some(
            sender
                .last_tx_hash
                .unwrap_or_else(|| transaction::account_chain_seed(&uid, AccountType::System)),
        );
        tx.receiver_last_hash = Some(
            receiver
                .last_tx_hash
                .unwrap_or_else(|| transaction::account_chain_seed(&to, to_type)),
        );
    }
    tx.timestamp = chrono::Utc::now().timestamp();
    tx.tx_hash = transaction::compute_tx_hash(&tx);

    // 服务端用系统账户密钥签名
    let sig = sign_system(&st, &uid, tx.tx_hash.as_bytes())?;
    tx.sender_sig = sig;

    let mut conn = st.db.lock().unwrap();
    transaction::submit_tx(&mut conn, &tx).map_err(ApiErr::from)?;
    crate::log::info(&format!(
        "系统账本转账：{} (System) -> {} ({}) {} A€，tx={}",
        uid, to, to_type.as_str(), req.amount, tx.tx_id
    ));
    Ok(Json(json!({
        "ok": true, "tx_id": tx.tx_id, "status": "Pending",
        "message": format!("转账已提交（Pending），等待接收方确认"),
    })))
}

#[derive(Deserialize)]
pub struct TxnIdReq {
    pub tx_id: String,
    #[serde(default)]
    pub reason: String,
}

/// 以代管系统账户身份确认收款（服务端权威，无需客户端签名）。
async fn sys_confirm(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<TxnIdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(uid) = acting_uid(&st, &auth) else {
        return Err(ApiErr::forbidden("请先在后台进入系统账本账户"));
    };
    let mut conn = st.db.lock().unwrap();
    transaction::confirm_tx(&mut conn, &req.tx_id, &uid, AccountType::System).map_err(ApiErr::from)?;
    Ok(Json(json!({ "ok": true, "status": "Confirmed", "message": "已确认收款" })))
}

/// 以代管系统账户身份拒收。
async fn sys_reject(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<TxnIdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(uid) = acting_uid(&st, &auth) else {
        return Err(ApiErr::forbidden("请先在后台进入系统账本账户"));
    };
    let mut conn = st.db.lock().unwrap();
    transaction::reject_tx(&mut conn, &req.tx_id, &uid, AccountType::System, &req.reason)
        .map_err(ApiErr::from)?;
    Ok(Json(json!({ "ok": true, "status": "Rejected", "message": "已拒收" })))
}

/// 退出系统账本（结束代管会话，返回系统账户选择页）。
async fn sys_logout(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    st.sys_acting.lock().unwrap().remove(&auth.token);
    Ok(Json(json!({ "ok": true, "logged_in": false })))
}

/// 用系统账户 gpg 密钥对消息签名。口令从 `{uid}.key`（主密钥 AES 加密）解密取得。
fn sign_system(st: &AppState, uid: &str, msg: &[u8]) -> Result<String, ApiErr> {
    let fp = st
        .gpg
        .fingerprint(uid)
        .ok()
        .ok_or_else(|| ApiErr::internal(format!("gpg 中未找到系统账户 {uid} 的密钥")))?;
    let master = std::fs::read_to_string(st.data_dir.join("master.key"))
        .map_err(|_| ApiErr::internal("缺少 master.key（主密钥），无法为系统账户签名"))?;
    let enc = std::fs::read_to_string(st.data_dir.join(format!("{uid}.key")))
        .map_err(|_| ApiErr::internal(format!("缺少 {uid}.key（系统账户口令密文）")))?;
    let passphrase = crate::crypto::decrypt_secret(&enc, &master)
        .map_err(|e| ApiErr::internal(format!("解密系统账户口令失败：{e}")))?;
    st.gpg
        .sign_detached(&fp, &passphrase, msg)
        .map_err(|e| ApiErr::internal(format!("系统账户签名失败：{e}")))
}
