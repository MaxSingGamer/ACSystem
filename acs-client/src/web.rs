//! acs-client 本地 Web GUI 后端：内嵌网页 + 本地 API。
//!
//! - 启动后绑定 127.0.0.1（默认 9580），自动打开浏览器访问网页。
//! - 网页定期发送心跳；网页关闭后心跳停止，超时自动退出进程。
//! - 所有账户/密钥/交易逻辑复用 wallet / client_api / txn / sync。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use acs_core::models::AccountType;

use crate::client_api;
use crate::sync;
use crate::txn;
use crate::wallet::Wallet;

/// 共享后端状态。
pub struct AppState {
    pub wallet: Mutex<Wallet>,
    pub last_heartbeat: AtomicI64,
}

pub type SharedState = Arc<AppState>;

/// 默认中心服务器地址（未配置时使用）。
pub const DEFAULT_SERVER: &str = "https://acsystem.maxshin.top";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 构建路由与共享状态。
pub fn build(wallet: Wallet) -> (Router, SharedState) {
    let state = Arc::new(AppState {
        wallet: Mutex::new(wallet),
        last_heartbeat: AtomicI64::new(now()),
    });
    let app = Router::new()
        .route("/", get(index))
        .route("/api/state", get(state_ep))
        .route("/api/logins", get(logins))
        .route("/api/login", post(login))
        .route("/api/register", post(register))
        .route("/api/logout", post(logout))
        .route("/api/sync", post(sync_ep))
        .route("/api/transfer", post(transfer))
        .route("/api/pending", get(pending_ep))
        .route("/api/confirm", post(confirm))
        .route("/api/reject", post(reject))
        .route("/api/submit", post(submit))
        .route("/api/set-server", post(set_server))
        .route("/api/delete-account", post(delete_account))
        .route("/api/members", get(members_ep))
        .route("/api/heartbeat", post(heartbeat))
        .route("/api/quit", post(quit))
        .with_state(state.clone());
    (app, state)
}

/// 心跳退出监控：网页关闭后心跳停止，超时自动退出进程。
pub fn heartbeat_watch(state: SharedState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if now() - state.last_heartbeat.load(Ordering::Relaxed) > 45 {
                std::process::exit(0);
            }
        }
    });
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

// ---------- 工具 ----------

fn lock(st: &SharedState) -> std::sync::MutexGuard<'_, Wallet> {
    st.wallet.lock().unwrap()
}

/// 未配置中心时使用默认地址。
fn ensure_server(w: &mut Wallet) {
    if w.info.server_url.trim().is_empty() {
        let _ = w.set_server_url(DEFAULT_SERVER);
    }
}

fn ok(msg: &str) -> Json<Value> {
    Json(json!({ "ok": true, "message": msg }))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "message": msg })),
    )
}

fn default_type() -> String {
    "Individual".into()
}

// ---------- 请求体 ----------

#[derive(Deserialize)]
struct LoginReq {
    uid: String,
    password: String,
}

#[derive(Deserialize)]
struct RegisterReq {
    server_url: String,
    uid: String,
    #[serde(rename = "type", default = "default_type")]
    atype: String,
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct PassReq {
    password: String,
}

#[derive(Deserialize)]
struct ConfirmReq {
    #[serde(default)]
    tx_id: Option<String>, // 可空：默认处理第一笔待确认
    password: String,
}

#[derive(Deserialize)]
struct RejectReq {
    tx_id: String,
    password: String,
    #[serde(default)]
    reason: String, // 拒收原因（可空）
}

#[derive(Deserialize)]
struct TransferReq {
    to: String,
    amount: i64,
    password: String,
}

#[derive(Deserialize)]
struct UrlReq {
    url: String,
}

// ---------- 处理器 ----------

async fn state_ep(State(st): State<SharedState>) -> Json<Value> {
    let w = lock(&st);
    let logged_in = w.info.initialized();
    // 不向客户端暴露全部账本账户快照（仅返回本账户相关数据）
    let txs = txn::list_local_tx(&w, 200);
    let outbox = txn::list_outbox(&w);
    let logins: Vec<Value> = w
        .list_local_accounts()
        .into_iter()
        .map(|a| {
            json!({
                "uid": a.uid,
                "type": a.atype.as_str(),
                "has_key": !a.encrypted_seckey.is_empty(),
                "last_login": a.last_login,
            })
        })
        .collect();
    Json(json!({
        "logged_in": logged_in,
        "uid": w.info.uid,
        "atype": w.info.atype.as_str(),
        "email": w.info.email,
        "server_url": w.info.server_url,
        "synced_at": w.info.synced_at,
        "balance": w.mirror_balance(),
        "txs": txs,
        "outbox": outbox,
        "logins": logins,
    }))
}

async fn logins(State(st): State<SharedState>) -> Json<Value> {
    let w = lock(&st);
    Json(json!({ "logins": w.list_local_accounts().into_iter().map(|a| json!({
        "uid": a.uid, "type": a.atype.as_str(), "has_key": !a.encrypted_seckey.is_empty(), "last_login": a.last_login
    })).collect::<Vec<_>>() }))
}

async fn login(
    State(st): State<SharedState>,
    Json(req): Json<LoginReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    match login_account(&mut w, &req.uid, &req.password) {
        Ok(uid) => Ok(ok(&format!("欢迎回来，{uid}"))),
        Err(e) => Err(err(&e.to_string())),
    }
}

fn login_account(w: &mut Wallet, uid: &str, pass: &str) -> anyhow::Result<String> {
    // 1) 本地缓存私钥：导入并校验口令
    if let Some(acc) = w.local_account(uid) {
        if !acc.encrypted_seckey.is_empty()
            && w.gpg.import_key(&acc.encrypted_seckey).is_ok()
            && w.fingerprint(uid)
                .and_then(|fp| w.gpg.verify_passphrase(&fp, pass).ok())
                .is_some()
        {
            w.switch_account(uid)?;
            return Ok(uid.to_string());
        }
    }
    // 2) 无缓存或口令不符：向中心取回（服务端校验密码哈希）
    let known_type = w.local_account(uid).map(|a| a.atype);
    let r = client_api::fetch_key(w, uid, known_type, pass)?;
    let email = r.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sek = r
        .get("encrypted_seckey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if sek.is_empty() {
        anyhow::bail!("中心未存有该账户加密私钥（该账户非本客户端注册）");
    }
    // 中心返回的实际账户类型（未指定类型时中心自动匹配）
    let atype = r
        .get("type")
        .and_then(|v| v.as_str())
        .and_then(|s| AccountType::from_str(s))
        .or(known_type)
        .unwrap_or(AccountType::Individual);
    w.gpg.import_key(&sek).map_err(|e| anyhow::anyhow!("导入密钥失败：{e}"))?;
    w.save_local_account(uid, atype, &email, &sek)?;
    w.switch_account(uid)?;
    Ok(uid.to_string())
}

async fn register(
    State(st): State<SharedState>,
    Json(req): Json<RegisterReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let atype = match AccountType::from_str(&req.atype) {
        Some(t) => t,
        None => return Err(err("无效账户类型")),
    };
    if req.uid.trim().is_empty() || req.email.is_empty() || req.password.len() < 8 {
        return Err(err("请填写 UID 与邮箱，口令至少 8 位"));
    }
    let mut w = lock(&st);
    ensure_server(&mut w);
    match register_account(&mut w, req.server_url.trim(), &req.uid, atype, &req.email, &req.password)
    {
        Ok(_) => Ok(ok("注册成功，账户已在中心登记")),
        Err(e) => Err(err(&e.to_string())),
    }
}

fn register_account(
    w: &mut Wallet,
    server_url: &str,
    uid: &str,
    atype: AccountType,
    email: &str,
    pass: &str,
) -> anyhow::Result<()> {
    // 注册表单提供了地址则覆盖；否则沿用默认
    if !server_url.trim().is_empty() {
        w.set_server_url(server_url)?;
    }
    let gk = w.create_key(uid, email, pass)?;
    w.init_wallet(uid, atype, email)?;
    let salt = uuid::Uuid::new_v4().to_string();
    let password_hash = format!("{salt}${}", sha256_hex(format!("{salt}:{pass}").as_bytes()));
    w.save_local_account(uid, atype, email, &gk.encrypted_seckey)?;
    client_api::open_account(w, &gk.encrypted_seckey, &password_hash)?;
    Ok(())
}

async fn logout(State(st): State<SharedState>) -> Json<Value> {
    let mut w = lock(&st);
    let _ = w.clear_current();
    Json(json!({ "ok": true }))
}

async fn sync_ep(
    State(st): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    match sync::pull(&w) {
        Ok(r) => {
            let _ = w.mark_synced(r.server_time, None);
            Ok(Json(json!({
                "ok": true,
                "message": format!("已同步（{}）：新增交易 {}，账户快照 {}", r.source, r.txs, r.accounts),
                "txs": r.txs,
                "accounts": r.accounts,
            })))
        }
        Err(e) => Err(err(&format!("同步失败：{e}"))),
    }
}

async fn transfer(
    State(st): State<SharedState>,
    Json(req): Json<TransferReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    let mut r = req.to.trim().to_string();
    let mut rtype = AccountType::Individual;
    if let Some(i) = r.find('@') {
        rtype = AccountType::from_str(&r[i + 1..]).unwrap_or(AccountType::Individual);
        r = r[..i].to_string();
    }
    if r.is_empty() {
        return Err(err("请输入接收方 UID"));
    }
    if req.amount <= 0 {
        return Err(err("金额须大于 0"));
    }
    match txn::build_and_sign_transfer(&w, &r, rtype, req.amount, &req.password) {
        Ok((tid, hash)) => Ok(Json(json!({
            "ok": true,
            "message": format!("已签名入待提交：{tid}"),
            "tx_id": tid,
            "hash": hash,
        }))),
        Err(e) => Err(err(&format!("转账失败：{e}"))),
    }
}

async fn confirm(
    State(st): State<SharedState>,
    Json(req): Json<ConfirmReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    let tid = match req.tx_id {
        Some(t) if !t.trim().is_empty() => t,
        _ => match client_api::list_pending(&w).ok().and_then(|l| l.into_iter().next().map(|p| p.tx_id)) {
            Some(t) => t,
            None => return Ok(Json(json!({ "ok": true, "message": "当前没有待确认交易" }))),
        },
    };
    match client_api::confirm_tx(&w, &tid, &req.password, None) {
        Ok(r) => {
            let s = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
            Ok(Json(json!({ "ok": true, "message": format!("交易 {:.8}… 状态 → {s}", tid) })))
        }
        Err(e) => Err(err(&e.to_string())),
    }
}

/// 待确认交易列表（作为接收方），供前端"确认收款/拒收"弹窗展示。
async fn pending_ep(
    State(st): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    match client_api::list_pending(&w) {
        Ok(list) => Ok(Json(json!({
            "items": list.into_iter().map(|p| json!({
                "tx_id": p.tx_id, "tx_type": p.tx_type, "sender": p.sender,
                "amount": p.amount, "timestamp": p.timestamp,
            })).collect::<Vec<_>>(),
        }))),
        Err(e) => Err(err(&format!("获取待确认列表失败：{e}"))),
    }
}

/// 拒收交易（作为接收方）：签名后提交中心，附拒收原因。
async fn reject(
    State(st): State<SharedState>,
    Json(req): Json<RejectReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    let reason = req.reason.trim();
    match client_api::confirm_tx(&w, &req.tx_id, &req.password, if reason.is_empty() { None } else { Some(reason) }) {
        Ok(r) => {
            let s = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
            Ok(Json(json!({ "ok": true, "message": format!("交易 {:.8}… 状态 → {s}", req.tx_id) })))
        }
        Err(e) => Err(err(&e.to_string())),
    }
}

async fn submit(
    State(st): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    match client_api::submit_outbox(&w, None) {
        Ok(res) if res.is_empty() => Ok(Json(json!({ "ok": true, "message": "outbox 中没有待提交交易" }))),
        Ok(res) => {
            let s: Vec<String> = res.iter().map(|(_, x)| x.clone()).collect();
            Ok(Json(json!({
                "ok": true,
                "message": format!("提交 {} 笔：{}", res.len(), s.join("，")),
            })))
        }
        Err(e) => Err(err(&e.to_string())),
    }
}

async fn set_server(
    State(st): State<SharedState>,
    Json(req): Json<UrlReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    let raw = req.url.trim().to_string();
    if raw.is_empty() {
        return Err(err("请输入中心地址"));
    }
    let u = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw
    } else {
        format!("http://{raw}")
    };
    w.set_server_url(&u).map_err(|e| err(&e.to_string()))?;
    Ok(ok(&format!("中心地址已保存：{u}")))
}

async fn members_ep(State(st): State<SharedState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    ensure_server(&mut w);
    match client_api::fetch_members(&w) {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(err(&e.to_string())),
    }
}

async fn delete_account(
    State(st): State<SharedState>,
    Json(req): Json<PassReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut w = lock(&st);
    let uid = w.info.uid.clone();
    let atype = w.info.atype;
    // 中心注销：状态改为 Deleted（账本只读保留，供审计；不可再登录）
    match client_api::close_account(&w, &req.password) {
        Ok(_) => {
            let _ = w.delete_local_account(&uid, atype);
            let _ = w.clear_current();
            Ok(Json(json!({
                "ok": true,
                "message": format!("账户 {uid} 已注销：中心状态已改 Deleted（账本只读保留供审计），本机记录已删除"),
            })))
        }
        Err(e) => Err(err(&e.to_string())),
    }
}

async fn heartbeat(State(st): State<SharedState>) -> Json<Value> {
    st.last_heartbeat.store(now(), Ordering::Relaxed);
    Json(json!({ "ok": true }))
}

async fn quit() -> Json<Value> {
    std::process::exit(0);
}
