//! Client（Alpha Wallet / acs-mirror）接口：账户开立、交易提交、接收方确认/拒绝、待确认查询。
//!
//! 认证：client 凭 ed25519 签名（开立上传公钥、提交/确认用私钥签名），不依赖镜像 apikey。
//! 镜像 apikey 仅用于 acs-mirror 服务向中心拉取（/api/mirror/pull）。
//! 安全：提交前校验 tx_hash 一致性 + 发送方 ed25519 签名；确认前校验接收方签名。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;

use acs_core::account;
use acs_core::models::{Account, AccountStatus, AccountType, Transaction};
use acs_core::transaction;

use crate::api::{ApiErr, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/client/open", post(open_account))
        .route("/api/client/submit", post(submit))
        .route("/api/client/confirm", post(confirm))
        .route("/api/client/reject", post(confirm))
        .route("/api/client/pending", get(pending))
        .route("/api/client/fetch-key", post(fetch_key))
        .route("/api/client/members", get(list_public_members))
        .route("/api/client/close", post(close_account))
}

/// 公开：返回 AEU 已认定的成员国家与银行（Active），供 client 注册下拉选择。
async fn list_public_members(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let mut countries = Vec::new();
    let mut banks = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT name FROM member_countries WHERE status='Active' ORDER BY name")
            .map_err(ApiErr::from_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(ApiErr::from_err)?;
        for r in rows {
            countries.push(r.map_err(ApiErr::from_err)?);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT name FROM member_companies WHERE status='Active' ORDER BY name")
            .map_err(ApiErr::from_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(ApiErr::from_err)?;
        for r in rows {
            banks.push(r.map_err(ApiErr::from_err)?);
        }
    }
    Ok(Json(json!({ "countries": countries, "banks": banks, "systems": ["PreIssuedAccount", "AESystem", "AlphaEU"] })))
}

// ---------- 注销账户（中心侧：状态改 Deleted，账本只读保留） ----------

#[derive(Deserialize)]
pub struct CloseReq {
    pub uid: String,
    #[serde(rename = "type")]
    pub atype: String,
    pub close_sig: String, // 账户私钥对 "close:{uid}:{type}" 的 detached 签名
}

/// 注销：校验账户本人签名后，将状态改为 Deleted（账本与账户信息只读保留，供审计；不可再登录）。
async fn close_account(
    State(st): State<AppState>,
    Json(req): Json<CloseReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(&req.atype)
        .ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
    let msg = format!("close:{}:{}", req.uid, atype.as_str());
    let pubk = {
        let conn = st.db.lock().unwrap();
        account_pubkey(&conn, &req.uid, atype).ok_or_else(|| ApiErr::not_found("账户不存在"))?
    };
    if !st
        .gpg
        .verify_detached(&pubk, msg.as_bytes(), &req.close_sig)
        .map_err(ApiErr::from)?
    {
        return Err(ApiErr::forbidden("注销签名校验失败"));
    }
    let conn = st.db.lock().unwrap();
    account::set_status(&conn, &req.uid, atype, AccountStatus::Deleted).map_err(ApiErr::from)?;
    Ok(Json(json!({ "ok": true, "uid": req.uid, "type": atype.as_str(), "status": "Deleted" })))
}

/// 读取账户公钥（用于验签）。
fn account_pubkey(conn: &Connection, uid: &str, atype: AccountType) -> Option<String> {
    let table = atype.table_name();
    let sql = format!("SELECT pubkey FROM {table} WHERE uid=?1");
    conn.query_row(&sql, params![uid], |r| r.get(0)).ok()
}

// ---------- 开立账户 ----------

#[derive(Deserialize)]
pub struct OpenReq {
    pub uid: String,
    #[serde(rename = "type")]
    pub atype: String,
    pub email: String,
    pub pubkey: String,
    #[serde(default)]
    pub encrypted_seckey: String, // 密码加密私钥（登录取回用）
    #[serde(default)]
    pub password_hash: String,    // $salt$sha256（客户端注册时上传）
}

async fn open_account(
    State(st): State<AppState>,
    Json(req): Json<OpenReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(&req.atype)
        .ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
    if req.uid.trim().is_empty() || req.email.trim().is_empty() || req.pubkey.trim().is_empty() {
        return Err(ApiErr::bad_request("uid/email/pubkey 均不能为空"));
    }
    // 校验公钥是有效 armored（解析指纹），防止垃圾数据
    let fp = st
        .gpg
        .fingerprint_of_armored_pubkey(&req.pubkey)
        .map_err(|_| ApiErr::bad_request("公钥格式无效（需为 gpg armored）"))?;

    let conn = st.db.lock().unwrap();
    if account::account_exists(&conn, &req.uid, atype)? {
        return Err(ApiErr::bad_request("账户已存在"));
    }
    // Country/Bank 账户必须为 AEU 已认定成员（Active），否则拒绝开立
    match atype {
        AccountType::Country => {
            let ok = conn
                .query_row(
                    "SELECT COUNT(*) FROM member_countries WHERE name=?1 AND status='Active'",
                    params![req.uid.trim()],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
                > 0;
            if !ok {
                return Err(ApiErr::bad_request("该国家未经 AEU 理事会认定，无法开立国家账户"));
            }
        }
        AccountType::Bank => {
            let ok = conn
                .query_row(
                    "SELECT COUNT(*) FROM member_companies WHERE name=?1 AND status='Active'",
                    params![req.uid.trim()],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
                > 0;
            if !ok {
                return Err(ApiErr::bad_request("该银行未经 AEU 理事会认定，无法开立银行账户"));
            }
        }
        _ => {}
    }
    let now = Utc::now();
    let acc = Account {
        uid: req.uid.trim().to_string(),
        account_type: atype,
        email: req.email.trim().to_string(),
        pubkey: req.pubkey.trim().to_string(),
        encrypted_seckey: req.encrypted_seckey.trim().to_string(), // 密码加密私钥（登录取回用）
        balance: 0,
        status: AccountStatus::Active,
        last_tx_hash: Some(transaction::account_chain_seed(&req.uid.trim(), atype)),
        created_at: now,
        changed_at: now,
    };
    account::create_account(&conn, &acc)?;
    // 存登录凭证（客户端注册时提供密码哈希；供登录取回）
    if !req.password_hash.trim().is_empty() {
        conn.execute(
            "INSERT INTO account_credentials(uid, type, password_hash) VALUES (?1,?2,?3) \
             ON CONFLICT(uid,type) DO UPDATE SET password_hash=excluded.password_hash",
            params![acc.uid.clone(), atype.as_str(), req.password_hash.trim()],
        )
        .map_err(ApiErr::from_err)?;
    }
    Ok(Json(json!({ "ok": true, "uid": acc.uid, "type": atype.as_str(), "fingerprint": fp, "balance": 0 })))
}

// ---------- 登录取回加密私钥 ----------

#[derive(Deserialize)]
pub struct FetchKeyReq {
    pub uid: String,
    #[serde(rename = "type", default)]
    pub atype: String, // 可空：空则按 UID 在所有账户类型中自动匹配
    pub password: String,
}

/// 客户端登录：验证密码哈希，返回加密私钥与账户信息（供本机缓存或跨设备恢复）。
async fn fetch_key(
    State(st): State<AppState>,
    Json(req): Json<FetchKeyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    // 类型：指定则单类型；否则按 UID 在全部账户类型中自动匹配
    let atypes: Vec<AccountType> = if req.atype.trim().is_empty() {
        vec![
            AccountType::Individual,
            AccountType::Country,
            AccountType::Bank,
            AccountType::System,
        ]
    } else {
        let t = AccountType::from_str(&req.atype)
            .ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
        vec![t]
    };
    let conn = st.db.lock().unwrap();
    let mut last_err: Option<ApiErr> = None;
    for atype in atypes {
        let stored: Option<String> = conn
            .query_row(
                "SELECT password_hash FROM account_credentials WHERE uid=?1 AND type=?2",
                params![req.uid, atype.as_str()],
                |r| r.get(0),
            )
            .ok();
        let Some(stored) = stored else { continue };
        let (salt, hash) = stored.split_once('$').unwrap_or(("", &stored));
        if crate::auth::hash_password(&req.password, salt) != hash {
            last_err = Some(ApiErr::forbidden("密码错误"));
            continue;
        }
        let acc = account::get_account(&conn, &req.uid, atype)?
            .ok_or_else(|| ApiErr::not_found("账户不存在"))?;
        if acc.status != AccountStatus::Active {
            return Err(ApiErr::forbidden("该账户已注销/冻结，无法登录"));
        }
        return Ok(Json(json!({
            "ok": true,
            "uid": acc.uid,
            "type": atype.as_str(),
            "email": acc.email,
            "pubkey": acc.pubkey,
            "encrypted_seckey": acc.encrypted_seckey,
        })));
    }
    Err(last_err.unwrap_or_else(|| ApiErr::not_found("该账户未设置登录凭证（需客户端注册时开启）")))
}

// ---------- 提交交易 ----------

#[derive(Deserialize)]
pub struct SubmitReq {
    pub tx: Transaction,
}

async fn submit(
    State(st): State<AppState>,
    Json(req): Json<SubmitReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let tx = req.tx;

    // 1) 校验 tx_hash 一致（防篡改）
    let expect = transaction::compute_tx_hash(&tx);
    if tx.tx_hash != expect {
        return Err(ApiErr::bad_request("交易哈希不一致（tx 被篡改）"));
    }

    // 3) 校验发送方存在 + 公钥签名
    let (sender_pub, sender_atype) = {
        let conn = st.db.lock().unwrap();
        let pubk = account_pubkey(&conn, &tx.sender, tx.sender_type)
            .ok_or_else(|| ApiErr::not_found(format!("发送方账户不存在: {}", tx.sender)))?;
        (pubk, tx.sender_type)
    };
    if !st
        .gpg
        .verify_detached(&sender_pub, tx.tx_hash.as_bytes(), &tx.sender_sig)
        .map_err(ApiErr::from)?
    {
        return Err(ApiErr::forbidden("发送方签名校验失败"));
    }

    // 4) 校验接收方存在（Transfer/Issue/Redeem 均需目标账户）
    {
        let conn = st.db.lock().unwrap();
        if !account::account_exists(&conn, &tx.receiver, tx.receiver_type)? {
            return Err(ApiErr::not_found(format!("接收方账户不存在: {}", tx.receiver)));
        }
        // 防止伪造他人为发送方：sender_type 由签名绑定，但再校验发送方与 apikey 无绑定关系，
        // 因此依赖签名有效性（已校验）。
        let _ = sender_atype;
    }

    // 5) 提交（Pending + tx_confirmations）
    let mut conn = st.db.lock().unwrap();
    transaction::submit_tx(&mut conn, &tx)?;
    Ok(Json(json!({ "ok": true, "tx_id": tx.tx_id, "status": "Pending" })))
}

// ---------- 接收方确认 / 拒绝 ----------

#[derive(Deserialize)]
pub struct ConfirmReq {
    pub tx_id: String,
    pub receiver_sig: String, // 接收方用私钥对 tx_id 的 detached 签名
    pub reject_reason: Option<String>,
}

async fn confirm(
    State(st): State<AppState>,
    Json(req): Json<ConfirmReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let reason = req.reject_reason.clone();
    let (receiver, rtype, pubk) = {
        let conn = st.db.lock().unwrap();
        let tx = transaction::get_transaction(&conn, &req.tx_id)
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::not_found("交易不存在"))?;
        if tx.status != acs_core::models::TransactionStatus::Pending {
            return Err(ApiErr::bad_request("该交易已处理"));
        }
        let pubk = account_pubkey(&conn, &tx.receiver, tx.receiver_type)
            .ok_or_else(|| ApiErr::not_found("接收方账户不存在"))?;
        (tx.receiver, tx.receiver_type, pubk)
    };
    // 校验接收方签名（对 tx_id）
    if !st
        .gpg
        .verify_detached(&pubk, req.tx_id.as_bytes(), &req.receiver_sig)
        .map_err(ApiErr::from)?
    {
        return Err(ApiErr::forbidden("接收方签名校验失败"));
    }
    let mut conn = st.db.lock().unwrap();
    match reason.as_deref() {
        Some(r) if !r.trim().is_empty() => {
            transaction::reject_tx(&mut conn, &req.tx_id, &receiver, rtype, r)?;
            Ok(Json(json!({ "ok": true, "tx_id": req.tx_id, "status": "Rejected" })))
        }
        _ => {
            transaction::confirm_tx(&mut conn, &req.tx_id, &receiver, rtype)?;
            Ok(Json(json!({ "ok": true, "tx_id": req.tx_id, "status": "Confirmed" })))
        }
    }
}

// ---------- 待确认查询 ----------

#[derive(Deserialize)]
pub struct PendingQuery {
    pub uid: String,
    #[serde(rename = "type", default = "default_type")]
    pub atype: String,
}

fn default_type() -> String {
    "Individual".to_string()
}

async fn pending(
    State(st): State<AppState>,
    Query(q): Query<PendingQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(&q.atype)
        .ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
    let conn = st.db.lock().unwrap();
    let txs = transaction::list_pending_for(&conn, &q.uid, atype)?;
    let items: Vec<serde_json::Value> = txs
        .iter()
        .map(|t| {
            json!({
                "tx_id": t.tx_id,
                "tx_type": t.tx_type.as_str(),
                "sender": t.sender,
                "sender_type": t.sender_type.as_str(),
                "amount": t.amount,
                "timestamp": t.timestamp,
                "tx_hash": t.tx_hash,
            })
        })
        .collect();
    Ok(Json(json!({ "uid": q.uid, "items": items })))
}
