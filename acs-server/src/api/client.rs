//! Client（Alpha Wallet）接口：账户开立、交易提交、接收方确认/拒绝、待确认查询。
//!
//! 认证：client 凭 ed25519 签名（开立上传公钥、提交/确认用私钥签名）。
//! 安全：提交前校验 tx_hash 一致性 + 发送方 ed25519 签名；确认前校验接收方签名。

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Response};
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
        .route("/api/legal/{doc}", get(legal_doc))
}

/// 公开：返回协议 / 隐私 HTML（client 注册前展示并获取同意）。doc: individual-terms / enterprise-terms / privacy
async fn legal_doc(Path(doc): Path<String>) -> Response {
    match crate::legal::doc_html(&doc) {
        Some(html) => Html(html).into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, Json(json!({ "error": "文档不存在" }))).into_response(),
    }
}

/// 公开：返回 AEU 已认定的成员国家与企业（Active），供 client 注册下拉选择。
async fn list_public_members(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let mut countries = Vec::new();
    let mut companies = Vec::new();
    let mut systems = Vec::new();
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
            companies.push(r.map_err(ApiErr::from_err)?);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT uid FROM accounts_system WHERE status='Active' ORDER BY uid")
            .map_err(ApiErr::from_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(ApiErr::from_err)?;
        for r in rows {
            systems.push(r.map_err(ApiErr::from_err)?);
        }
    }
    Ok(Json(json!({ "countries": countries, "companies": companies, "systems": systems })))
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
    // 云端只保留公钥：删掉口令加密的私钥密文（中心不再持有可解开的私钥材料）
    account::purge_secret(&conn, &req.uid, atype).map_err(ApiErr::from)?;
    Ok(Json(json!({
        "ok": true, "uid": req.uid, "type": atype.as_str(), "status": "Deleted",
        "message": "账户已注销：中心已删除加密私钥（仅保留公钥），不可再交易"
    })))
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
    pub encrypted_seckey: String, // 密码加密私钥（登录取回用；口令不落库）
    #[serde(default)]
    pub agree_terms: bool,        // 是否同意《使用协议》
    #[serde(default)]
    pub agree_privacy: bool,      // 是否同意《隐私政策》
}

async fn open_account(
    State(st): State<AppState>,
    Json(req): Json<OpenReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(&req.atype)
        .ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
    // 系统账本账户：不接受客户端自助开立（改由 Server 管理后台「系统账本账户登录」）
    if atype == AccountType::System {
        return Err(ApiErr::forbidden("系统账本账户不接受客户端自助开立，请通过管理后台操作"));
    }
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
    // Country/Company 账户必须为 AEU 已认定成员（Active），否则拒绝开立
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
        AccountType::Company => {
            let ok = conn
                .query_row(
                    "SELECT COUNT(*) FROM member_companies WHERE name=?1 AND status='Active'",
                    params![req.uid.trim()],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
                > 0;
            if !ok {
                return Err(ApiErr::bad_request("该企业未经 AEU 理事会认定，无法开立企业账户"));
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
    // 注册须先同意《使用协议》/《隐私政策》（服务端二次校验并记录同意标识）。
    // 注：不保存任何口令哈希——私钥本身由用户口令加密（GPG S2K），口令不落库、不传输。
    if !req.agree_terms || !req.agree_privacy {
        return Err(ApiErr::forbidden("注册须先同意《使用协议》与《隐私政策》"));
    }
    conn.execute(
        &format!(
            "UPDATE {} SET last_login=?2, agree_terms=?3, agree_privacy=?4 WHERE uid=?1",
            atype.table_name()
        ),
        params![
            acc.uid.clone(),
            now.timestamp(),
            i64::from(req.agree_terms),
            i64::from(req.agree_privacy)
        ],
    )
    .map_err(ApiErr::from_err)?;
    Ok(Json(json!({ "ok": true, "uid": acc.uid, "type": atype.as_str(), "fingerprint": fp, "balance": 0 })))
}

// ---------- 登录取回加密私钥 ----------

#[derive(Deserialize)]
pub struct FetchKeyReq {
    pub uid: String,
    #[serde(rename = "type", default)]
    pub atype: String, // 可空：空则按 UID 在所有账户类型中自动匹配
    #[serde(default)]
    pub agree_terms: bool,   // 是否已同意《使用协议》
    #[serde(default)]
    pub agree_privacy: bool, // 是否已同意《隐私政策》
}

/// `fetch-key` 限流：单账户每小时上限（`ACS_FETCH_KEY_MAX_PER_HOUR`，默认 10）。
fn fetch_key_max_per_account() -> usize {
    std::env::var("ACS_FETCH_KEY_MAX_PER_HOUR")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10)
}

/// `fetch-key` 限流：全局每小时上限（`ACS_FETCH_KEY_GLOBAL_MAX_PER_HOUR`，默认 2000）。
/// 用于拦住「换 uid 批量枚举」——单账户限额拦不住这种遍历。
fn fetch_key_max_global() -> usize {
    std::env::var("ACS_FETCH_KEY_GLOBAL_MAX_PER_HOUR")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2000)
}

/// 限流窗口（秒）。
const FETCH_WINDOW_SECS: i64 = 3600;

/// 全局限流计数器在 key_fetch_hits 中的保留键（uid 不可能为它）。
const FETCH_GLOBAL_KEY: &str = "__all__";

/// 检查并记录一次 `fetch-key` 请求；超限返回 429。
///
/// 注：窗口为内存态（重启清零）。因服务端通常在反向代理（frp）之后、真实客户端 IP
/// 不可得，故不按 IP 限流，而是「单账户 + 全局」双窗口。
fn check_fetch_rate(st: &AppState, uid: &str) -> Result<(), ApiErr> {
    let per_max = fetch_key_max_per_account();
    let global_max = fetch_key_max_global();
    let now = chrono::Utc::now().timestamp();
    let mut hits = st.key_fetch_hits.lock().unwrap();

    // 防内存膨胀：条目过多时清理已过期的 key（全局键始终保留）
    if hits.len() > 5_000 {
        hits.retain(|k, v| {
            v.retain(|t| now - *t < FETCH_WINDOW_SECS);
            k == FETCH_GLOBAL_KEY || !v.is_empty()
        });
    }

    {
        let g = hits.entry(FETCH_GLOBAL_KEY.to_string()).or_default();
        g.retain(|t| now - *t < FETCH_WINDOW_SECS);
        if g.len() >= global_max {
            return Err(ApiErr::too_many_requests(
                "服务端密钥取回请求过于频繁，请稍后再试",
            ));
        }
        g.push(now);
    }
    {
        let per = hits.entry(uid.to_string()).or_default();
        per.retain(|t| now - *t < FETCH_WINDOW_SECS);
        if per.len() > per_max {
            return Err(ApiErr::too_many_requests(format!(
                "该账户取回密钥过于频繁（每小时上限 {per_max} 次），请稍后再试"
            )));
        }
        per.push(now);
    }
    Ok(())
}

/// 客户端登录：按 uid/type 返回加密私钥与账户信息（供本机缓存或跨设备恢复）。
/// **服务端不保存、不接收、不校验任何口令材料**：私钥由用户口令加密（GPG S2K），
/// 客户端本地用口令解开即完成校验。正因如此，返回密文的唯一凭据就是 uid ——
/// 故此处必须限流防枚举（见 `check_fetch_rate`：单账户 + 全局双窗口，环境变量可调）。
/// v3.1.0：登录请求须携带协议同意标识；未同意则服务端拒绝（客户端也应前置拦截）。
async fn fetch_key(
    State(st): State<AppState>,
    Json(req): Json<FetchKeyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !req.agree_terms || !req.agree_privacy {
        return Err(ApiErr::forbidden("登录须先同意《使用协议》与《隐私政策》"));
    }
    check_fetch_rate(&st, &req.uid)?;
    // 类型：指定则单类型；否则按 UID 在全部账户类型中自动匹配
    let atypes: Vec<AccountType> = if req.atype.trim().is_empty() {
        vec![
            AccountType::Individual,
            AccountType::Country,
            AccountType::Company,
            AccountType::System,
        ]
    } else {
        let t = AccountType::from_str(&req.atype)
            .ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
        vec![t]
    };
    let conn = st.db.lock().unwrap();
    for atype in atypes {
        // 不再校验口令哈希：私钥本身由用户口令加密（GPG S2K），"能否解开"即口令是否正确的唯一判据。
        // 这里只确认该类型下账户存在且可用，然后返回其加密私钥。
        let Some(acc) = account::get_account(&conn, &req.uid, atype)? else {
            continue;
        };
        if acc.status != AccountStatus::Active {
            return Err(ApiErr::forbidden("该账户已注销/冻结，无法登录"));
        }
        // 系统账本账户：客户端不再支持登录（改由管理后台操作）
        if atype == AccountType::System {
            return Err(ApiErr::forbidden(
                "系统账本账户请通过 Server 管理后台「系统账本账户登录」操作，客户端已取消系统账户登录",
            ));
        }
        // 记录最近登录时间与协议同意标识（不涉及口令）
        let _ = conn.execute(
            &format!(
                "UPDATE {} SET last_login=?1, agree_terms=?2, agree_privacy=?3 WHERE uid=?4",
                atype.table_name()
            ),
            params![
                chrono::Utc::now().timestamp(),
                i64::from(req.agree_terms),
                i64::from(req.agree_privacy),
                req.uid
            ],
        );
        return Ok(Json(json!({
            "ok": true,
            "uid": acc.uid,
            "type": atype.as_str(),
            "email": acc.email,
            "encrypted_seckey": acc.encrypted_seckey,
        })));
    }
    Err(ApiErr::not_found("账户不存在"))
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
    let mut tx = req.tx;

    // 1) 校验 tx_hash 一致（防篡改）
    let expect = transaction::compute_tx_hash(&tx);
    if tx.tx_hash != expect {
        return Err(ApiErr::bad_request("交易哈希不一致（tx 被篡改）"));
    }

    // 2) 校验客户端时间戳。ts 由客户端自填且参与 tx_hash，不校验等于账本时间轴可被任意伪造；
    //    权威时间另存 received_at（服务端时钟，客户端无法影响）。
    //    容忍度：未来 5 分钟（时钟误差）、历史 7 天（客户端出件箱可能延后重试提交）。
    let now = chrono::Utc::now().timestamp();
    if tx.timestamp > now + 300 {
        return Err(ApiErr::bad_request("交易时间戳超出允许范围（客户端时钟可能不准）"));
    }
    if tx.timestamp < now - 7 * 86_400 {
        return Err(ApiErr::bad_request("交易时间戳过于陈旧（请校准客户端时钟后重试）"));
    }
    tx.received_at = now;

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

    // 5) 提交（Pending）；提交前只重算「发送方」余额，保证余额判断准确
    //    （全量重算代价高且会长时间持有全局库锁，交由 /api/sync 低频触发）
    let mut conn = st.db.lock().unwrap();
    acs_core::account::recompute_account(&conn, &tx.sender, tx.sender_type)?;
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
            transaction::confirm_tx(&mut conn, &req.tx_id, &receiver, rtype, &req.receiver_sig)?;
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
