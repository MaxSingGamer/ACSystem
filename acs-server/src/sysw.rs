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
        .route("/api/admin/sys/change-password", post(sys_change_password))
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
    /// 该系统账本账户的密码（＝该账户私钥的解密口令）。
    /// 代管期间所有签名都由服务端用该账户密钥完成，故进入账本必须二次确认身份。
    #[serde(default)]
    pub password: String,
}

/// 后台管理员「进入」某个系统账本账户（建立代管会话）。
/// 必须输入该账户密码：代管 ＝ 持有该账户密钥的使用权，口令错则拒绝进入（并计入失败锁定）。
async fn sys_act(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ActReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let uid = req.uid.trim().to_string();
    if uid.is_empty() {
        return Err(ApiErr::bad_request("缺少系统账户 uid"));
    }
    if req.password.is_empty() {
        return Err(ApiErr::bad_request("请输入账本账户密码"));
    }
    let now = chrono::Utc::now().timestamp();
    let lock_key = format!("sys:{uid}");
    if let Some(wait) = crate::auth::lock_wait(&st, &lock_key, now) {
        return Err(ApiErr::too_many_requests(format!(
            "尝试过于频繁，请 {wait} 秒后重试"
        )));
    }
    // 只持锁读一次（口令验算在锁外做，避免占用全局库锁）
    let (acc, enc, ledger_enc, must_change_db) = {
        let conn = st.db.lock().unwrap();
        let acc = account::get_account(&conn, &uid, AccountType::System)
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::not_found("系统账户不存在"))?;
        let (enc, ledger, must): (String, String, i64) = conn
            .query_row(
                "SELECT key_passphrase_enc, ledger_pw_enc, must_change_password \
                 FROM accounts_system WHERE uid=?1",
                rusqlite::params![uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or_default();
        (acc, enc, ledger, must)
    };
    if acc.status != AccountStatus::Active {
        return Err(ApiErr::forbidden("该系统账户非 Active，无法登录账本"));
    }
    let master = master_key(&st)?;
    // 口令校验：优先「账本访问口令」密文；旧库遗留（为空）回退为私钥口令密文
    let legacy = ledger_enc.trim().is_empty();
    let ok = if legacy {
        if enc.is_empty() {
            return Err(ApiErr::internal(
                "该系统账户未配置口令（请检查服务端初始化；旧库需重新生成系统账户）",
            ));
        }
        let key_pw = crate::crypto::decrypt_secret(&enc, &master)
            .map_err(|e| ApiErr::internal(format!("解密系统账户口令失败：{e}")))?;
        crate::password::ct_eq(&key_pw, &req.password)
    } else {
        let ledger_pw = crate::crypto::decrypt_secret(&ledger_enc, &master)
            .map_err(|e| ApiErr::internal(format!("解密账本访问口令失败：{e}")))?;
        crate::password::ct_eq(&ledger_pw, &req.password)
    };
    if !ok {
        crate::auth::register_fail(&st, &lock_key, now);
        return Err(ApiErr::unauthorized("账本账户密码错误"));
    }
    st.login_fails.lock().unwrap().remove(&lock_key);
    // 旧库遗留：把当前口令封存为「账本访问口令」，并保持「首次登录须改密」
    let must_change = if legacy {
        let conn = st.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE accounts_system SET ledger_pw_enc=?2, must_change_password=1 \
             WHERE uid=?1 AND ledger_pw_enc=''",
            rusqlite::params![uid, crate::crypto::encrypt_secret(&master, &req.password)],
        );
        true
    } else {
        must_change_db != 0
    };
    st.sys_acting.lock().unwrap().insert(auth.token.clone(), uid.clone());
    crate::log::info(format!(
        "系统账本登录：管理员 {} 代管系统账户 {}{}",
        auth.username,
        uid,
        if must_change { "（首次登录，须修改账本口令）" } else { "" }
    ));
    Ok(Json(json!({
        "ok": true, "uid": uid, "email": acc.email, "atype": "System",
        "must_change_password": must_change
    })))
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
    let must_change: i64 = conn
        .query_row(
            "SELECT must_change_password FROM accounts_system WHERE uid=?1",
            rusqlite::params![uid],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(Json(json!({
        "logged_in": true,
        "uid": acc.uid,
        "atype": "System",
        "email": acc.email,
        "balance": acc.balance,
        "status": acc.status.as_str(),
        "must_change_password": must_change != 0,
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
    ensure_password_changed(&st, &uid)?;
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

/// 以代管系统账户身份确认收款：由服务端用该账户密钥对 `tx_id` 现签，
/// 保证每笔交易都具备接收方签名（不使用明文标记冒充签名）。
async fn sys_confirm(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<TxnIdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(uid) = acting_uid(&st, &auth) else {
        return Err(ApiErr::forbidden("请先在后台进入系统账本账户"));
    };
    ensure_password_changed(&st, &uid)?;
    // 先签名再取连接锁：sign_system 内部会短暂锁库读口令密文，
    // 若在持有连接锁时调用会触发 std::sync::Mutex 自死锁。
    let receiver_sig = sign_system(&st, &uid, req.tx_id.as_bytes())?;
    let mut conn = st.db.lock().unwrap();
    transaction::confirm_tx(&mut conn, &req.tx_id, &uid, AccountType::System, &receiver_sig)
        .map_err(ApiErr::from)?;
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
    ensure_password_changed(&st, &uid)?;
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

#[derive(Deserialize)]
pub struct ChangePassReq {
    pub old_password: String,
    pub new_password: String,
}

/// 修改当前代管系统账本账户的**访问口令**（需已进入该账本）。
///
/// 只重新封存「账本访问口令」（存 `accounts_system.ledger_pw_enc`，AES-GCM，密钥为 master.key）：
/// 私钥口令密文 `key_passphrase_enc` 不动 —— 服务端签名仍用 master.key 解封它，
/// 因此改密不需要重新保管/替换密钥，也不会影响已有签名能力（也不需保存任何口令哈希）。
async fn sys_change_password(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ChangePassReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(uid) = acting_uid(&st, &auth) else {
        return Err(ApiErr::forbidden("请先在后台进入系统账本账户"));
    };
    if req.new_password.len() < 8 {
        return Err(ApiErr::bad_request("新口令至少 8 位"));
    }
    if req.new_password == req.old_password {
        return Err(ApiErr::bad_request("新口令不能与原口令相同"));
    }
    let now = chrono::Utc::now().timestamp();
    let lock_key = format!("sys:{uid}");
    if let Some(wait) = crate::auth::lock_wait(&st, &lock_key, now) {
        return Err(ApiErr::too_many_requests(format!(
            "尝试过于频繁，请 {wait} 秒后重试"
        )));
    }
    let (enc, ledger_enc) = {
        let conn = st.db.lock().unwrap();
        conn.query_row(
            "SELECT key_passphrase_enc, ledger_pw_enc FROM accounts_system WHERE uid=?1",
            rusqlite::params![uid],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|_| ApiErr::not_found("系统账户不存在"))?
    };
    let master = master_key(&st)?;
    let ok = if ledger_enc.trim().is_empty() {
        let key_pw = crate::crypto::decrypt_secret(&enc, &master)
            .map_err(|e| ApiErr::internal(format!("解密系统账户口令失败：{e}")))?;
        crate::password::ct_eq(&key_pw, &req.old_password)
    } else {
        let ledger_pw = crate::crypto::decrypt_secret(&ledger_enc, &master)
            .map_err(|e| ApiErr::internal(format!("解密账本访问口令失败：{e}")))?;
        crate::password::ct_eq(&ledger_pw, &req.old_password)
    };
    if !ok {
        crate::auth::register_fail(&st, &lock_key, now);
        return Err(ApiErr::unauthorized("原口令错误"));
    }
    st.login_fails.lock().unwrap().remove(&lock_key);
    {
        let conn = st.db.lock().unwrap();
        // 只重新封存「账本访问口令」；key_passphrase_enc（私钥口令）保持不动，
        // 因此服务端签名能力不受影响（与 auth.rs 里管理员改密的做法一致）。
        let new_enc = crate::crypto::encrypt_secret(&master, &req.new_password);
        conn.execute(
            "UPDATE accounts_system SET ledger_pw_enc=?2, must_change_password=0, changed_at=?3 \
             WHERE uid=?1",
            rusqlite::params![uid, new_enc, now],
        )
        .map_err(ApiErr::from_err)?;
        crate::api::audit::log_audit(
            &conn,
            &auth.username,
            "sys_change_password",
            &format!("uid={uid}"),
        );
    }
    crate::log::info(format!(
        "系统账本口令已更新：管理员 {} 修改了 {} 的访问口令",
        auth.username, uid
    ));
    Ok(Json(json!({ "ok": true, "uid": uid, "message": "账本口令已更新" })))
}

/// 首次登录强制改密：未改密前禁止资金操作（转账 / 确认 / 拒收）。
fn ensure_password_changed(st: &AppState, uid: &str) -> Result<(), ApiErr> {
    let must: i64 = {
        let conn = st.db.lock().unwrap();
        conn.query_row(
            "SELECT must_change_password FROM accounts_system WHERE uid=?1",
            rusqlite::params![uid],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    if must != 0 {
        return Err(ApiErr::forbidden(
            "首次登录须先修改该账本账户口令（左侧菜单「修改密码」）",
        ));
    }
    Ok(())
}

/// 读取主密钥（数据目录 `master.key`）：用于解密系统账户口令密文。
fn master_key(st: &AppState) -> Result<String, ApiErr> {
    std::fs::read_to_string(st.data_dir.join("master.key"))
        .map_err(|_| ApiErr::internal("缺少 master.key（主密钥），无法为系统账户签名"))
}

/// 用系统账户 gpg 密钥对消息签名。
/// 口令密文存于数据库 `accounts_system.key_passphrase_enc`（**不再从 {uid}.key 文件读取**），
/// 用数据目录 `master.key` 解密取得口令。
fn sign_system(st: &AppState, uid: &str, msg: &[u8]) -> Result<String, ApiErr> {
    let fp = st
        .gpg
        .fingerprint(uid)
        .ok()
        .ok_or_else(|| ApiErr::internal(format!("gpg 中未找到系统账户 {uid} 的密钥")))?;
    let enc: String = {
        let conn = st.db.lock().unwrap();
        conn.query_row(
            "SELECT key_passphrase_enc FROM accounts_system WHERE uid=?1",
            rusqlite::params![uid],
            |r| r.get(0),
        )
        .map_err(|_| ApiErr::internal(format!("系统账户 {uid} 缺少口令密文（未初始化）")))?
    };
    if enc.is_empty() {
        return Err(ApiErr::internal(format!("系统账户 {uid} 未设置口令密文")));
    }
    let master = master_key(st)?;
    let passphrase = crate::crypto::decrypt_secret(&enc, &master)
        .map_err(|e| ApiErr::internal(format!("解密系统账户口令失败：{e}")))?;
    st.gpg
        .sign_detached(&fp, &passphrase, msg)
        .map_err(|e| ApiErr::internal(format!("系统账户签名失败：{e}")))
}
