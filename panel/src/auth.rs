//! 面板账户与登录（v4.0.0 Panel #1）。
//!
//! 设计要点（用户需求原文 → 实现）：
//! - **面板登录**：单一账户，默认 `root` / `ChangeMe123!`，**硬编码在代码里**（不写 `.env`）。
//! - **初次登录强制改用户名与密码**：密码哈希落盘在 `~/.alphacoinserver/panel-account.json`；
//!   默认口令登录后只能进入「强制改密」页，其它接口一律 403；改密后默认口令即失效。
//! - **登录态下的账户安全操作需再验证密码**：改凭据 / 增删 TOTP / 增删 Passkey 前调用
//!   `POST /api/auth/verify` 获得 5 分钟「已验证窗口」；改密本身要求输入当前密码。
//! - **TOTP 最多三个**：开启后「用户名+密码」登录强制 2FA；Passkey 仍一键登录（不走 TOTP）。
//! - **Passkey（WebAuthn）**：见 [`crate::passkey`]。
//!
//! 会话只存在内存里（面板重启 = 重新登录），不落盘、不写 `.env`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{FromRequestParts, State};
use axum::http::{header, request::Parts, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::PanelState;

/// 默认账户（硬编码，按需求**不写入 .env**）。
pub const DEFAULT_USERNAME: &str = "root";
pub const DEFAULT_PASSWORD: &str = "ChangeMe123!";

/// 会话待机有效期（秒）：登录后 2 小时无操作需重新登录。
const SESSION_TTL_SECS: i64 = 7200;
/// 「再次验证密码」的有效窗口（秒）：窗口内连续做账户安全操作不必反复输入。
const VERIFY_WINDOW_SECS: i64 = 300;
/// TOTP 最多允许绑定的验证器数量。
pub const MAX_TOTP: usize = 3;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 供同 crate 其他模块（passkey）取锁，避免各自处理中毒锁。
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

// ============================ 账户文件 ============================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Base32（RFC4648，无填充）密钥。
    pub secret: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyEntry {
    /// credential id（base64url，无填充）。
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// ES256 公钥（SEC1 未压缩点，base64 标准编码）。
    pub public_key: String,
    #[serde(default)]
    pub sign_count: u32,
    /// 注册时的 rpId（诊断用；校验时以当前访问 Host 为准）。
    #[serde(default)]
    pub rp_id: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelAccount {
    pub username: String,
    pub password_hash: String,
    /// 是否仍处于「必须修改初始用户名/密码」状态。
    pub must_change: bool,
    /// 初始密码是否已经用于登录过（记录用）。
    #[serde(default)]
    pub initial_used: bool,
    #[serde(default)]
    pub totp: Vec<TotpEntry>,
    #[serde(default)]
    pub passkeys: Vec<PasskeyEntry>,
    pub updated_at: i64,
}

impl PanelAccount {
    fn new_default() -> Self {
        PanelAccount {
            username: DEFAULT_USERNAME.to_string(),
            password_hash: acs_core::password::hash(DEFAULT_PASSWORD),
            must_change: true,
            initial_used: false,
            totp: Vec::new(),
            passkeys: Vec::new(),
            updated_at: now(),
        }
    }
}

pub fn account_path() -> std::path::PathBuf {
    crate::registry::server_root().join("panel-account.json")
}

/// 读取账户；不存在时**创建默认账户**（root / ChangeMe123!，必须改）。
pub fn ensure_account() -> std::io::Result<bool> {
    if account_path().exists() {
        return Ok(false);
    }
    let acc = PanelAccount::new_default();
    save_account(&acc)?;
    Ok(true)
}

pub fn load_account() -> Result<PanelAccount, String> {
    let p = account_path();
    let text = std::fs::read_to_string(&p).map_err(|e| format!("读取 {} 失败：{e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} 解析失败：{e}", p.display()))
}

pub fn save_account(acc: &PanelAccount) -> std::io::Result<()> {
    let p = account_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(acc).map_err(std::io::Error::other)?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

// ============================ 会话 ============================

#[derive(Debug, Clone)]
pub struct Session {
    pub username: String,
    pub must_change: bool,
    #[allow(dead_code)]
    pub created_at: i64,
    pub expires_at: i64,
    /// 再次验证密码的有效截止时间（<= now 表示未验证）。
    pub verified_until: i64,
}

impl Session {
    pub fn verified(&self) -> bool {
        self.verified_until > now()
    }
}

/// Passkey 登录/注册的一次性挑战。
#[derive(Debug, Clone)]
pub struct Challenge {
    pub kind: &'static str, // "register" | "login"
    pub rp_id: String,
    pub expires_at: i64,
}

/// 待确认的 TOTP 绑定（token -> (secret, 发起时间)）。
#[derive(Debug, Clone)]
pub struct PendingTotp {
    pub secret: String,
    pub created_at: i64,
}

/// 登录失败锁定（按 用户名 / 来源 IP 分别计数 + 指数退避）。
#[derive(Debug, Clone, Default)]
pub struct LoginLock {
    pub fails: u32,
    pub locked_until: i64,
}

#[derive(Clone)]
pub struct AuthState {
    pub sessions: Arc<Mutex<HashMap<String, Session>>>,
    pub challenges: Arc<Mutex<HashMap<String, Challenge>>>,
    pub pending_totp: Arc<Mutex<HashMap<String, PendingTotp>>>,
    /// 按「尝试的用户名」计数：不存在的用户名只锁自己，不影响真实账户（F10）。
    pub login_user_fails: Arc<Mutex<HashMap<String, LoginLock>>>,
    /// 按「来源 IP」计数：60s 内失败 10 次 → 429（防高速爆破）。
    pub login_ip_fails: Arc<Mutex<HashMap<String, LoginLock>>>,
    /// 口令哈希并发闸门：PBKDF2 是 CPU 密集操作，最多同时算 2 个，
    /// 其余请求快速返回 429（避免登录请求占满线程导致整个面板不可用）。
    pub hash_gate: Arc<tokio::sync::Semaphore>,
}

impl Default for AuthState {
    fn default() -> Self {
        AuthState {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            challenges: Arc::new(Mutex::new(HashMap::new())),
            pending_totp: Arc::new(Mutex::new(HashMap::new())),
            login_user_fails: Arc::new(Mutex::new(HashMap::new())),
            login_ip_fails: Arc::new(Mutex::new(HashMap::new())),
            hash_gate: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
}

/// 若 key 处于锁定期，返回剩余秒数。
fn lock_wait(m: &Mutex<HashMap<String, LoginLock>>, key: &str) -> Option<i64> {
    let ts = now();
    let map = lock(m);
    map.get(key)
        .filter(|l| l.locked_until > ts)
        .map(|l| l.locked_until - ts)
}

/// 记录一次失败；达到 `threshold` 后指数退避锁定（base 秒起，封顶 300s）。
fn bump_fail(m: &Mutex<HashMap<String, LoginLock>>, key: &str, threshold: u32, base: i64) {
    let ts = now();
    let mut map = lock(m);
    let e = map.entry(key.to_string()).or_default();
    e.fails = e.fails.saturating_add(1);
    if e.fails >= threshold {
        let over = (e.fails - threshold) as i64;
        let secs = (base * (1i64 << over.min(4))).min(300);
        e.locked_until = ts + secs;
        acs_core::log::err(format!("面板登录失败锁定：{key}（{secs} 秒）"));
    }
    if map.len() > 2000 {
        map.retain(|_, l| l.locked_until > ts || l.fails < threshold);
    }
}

fn clear_fails(m: &Mutex<HashMap<String, LoginLock>>, key: &str) {
    lock(m).remove(key);
}

/// 从请求头取来源 IP（面板默认仅本机，反向代理时取 X-Forwarded-For 首段）。
fn client_ip(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

/// 在阻塞线程池里校验口令；拿不到哈希闸门时返回 `Err(())`（调用方回 429）。
async fn verify_password(st: &PanelState, pw: String, stored: String) -> Result<bool, ()> {
    let permit = st
        .auth
        .hash_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| ())?;
    let ok = tokio::task::spawn_blocking(move || {
        let r = acs_core::password::verify(&pw, &stored);
        drop(permit);
        r
    })
    .await
    .unwrap_or(false);
    Ok(ok)
}

/// 在阻塞线程池里计算新口令哈希（等待闸门，允许排队）。
async fn hash_password(st: &PanelState, pw: String) -> Option<String> {
    let permit = st.auth.hash_gate.clone().acquire_owned().await.ok()?;
    tokio::task::spawn_blocking(move || {
        let h = acs_core::password::hash(&pw);
        drop(permit);
        h
    })
    .await
    .ok()
}

/// 新建会话，返回 token。
pub fn create_session(st: &PanelState, username: &str, must_change: bool) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let ts = now();
    lock(&st.auth.sessions).insert(
        token.clone(),
        Session {
            username: username.to_string(),
            must_change,
            created_at: ts,
            expires_at: ts + SESSION_TTL_SECS,
            verified_until: 0,
        },
    );
    token
}

/// 取会话（滑动续期 + 过期清理）。
pub fn touch_session(st: &PanelState, token: &str) -> Option<Session> {
    let mut m = lock(&st.auth.sessions);
    let ts = now();
    let s = m.get_mut(token)?;
    if s.expires_at < ts {
        m.remove(token);
        return None;
    }
    s.expires_at = ts + SESSION_TTL_SECS;
    Some(s.clone())
}

/// 让某账户的其他会话全部失效（改凭据后调用；保留当前 token）。
pub fn invalidate_others(st: &PanelState, keep: &str) {
    let mut m = lock(&st.auth.sessions);
    m.retain(|t, _| t == keep);
}

// ============================ 提取器 / 拒绝 ============================

pub struct AuthReject {
    pub status: StatusCode,
    pub message: String,
    pub must_change: bool,
    pub need_totp: bool,
}

impl AuthReject {
    fn unauthorized(msg: impl Into<String>) -> Self {
        AuthReject {
            status: StatusCode::UNAUTHORIZED,
            message: msg.into(),
            must_change: false,
            need_totp: false,
        }
    }
    fn forbidden(msg: impl Into<String>) -> Self {
        AuthReject {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
            must_change: false,
            need_totp: false,
        }
    }
}

impl IntoResponse for AuthReject {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
                "must_change": self.must_change,
                "need_totp": self.need_totp,
            })),
        )
            .into_response()
    }
}

fn token_of(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

fn require_session(parts: &Parts, st: &PanelState, allow_must_change: bool) -> Result<(String, Session), AuthReject> {
    let token = token_of(parts).ok_or_else(|| AuthReject::unauthorized("缺少登录令牌"))?;
    let s = touch_session(st, &token).ok_or_else(|| AuthReject::unauthorized("会话无效或已过期，请重新登录"))?;
    if !allow_must_change && s.must_change {
        return Err(AuthReject {
            status: StatusCode::FORBIDDEN,
            message: "首次登录须先修改用户名与密码".into(),
            must_change: true,
            need_totp: false,
        });
    }
    Ok((token, s))
}

/// 已登录且已完成强制改密的用户（业务接口用）。
pub struct PanelUser {
    pub token: String,
    pub session: Session,
}

impl PanelUser {
    pub fn require_verified(&self) -> Result<(), AuthReject> {
        if self.session.verified() {
            Ok(())
        } else {
            Err(AuthReject::forbidden("该操作需要先验证登录密码"))
        }
    }
}

impl FromRequestParts<PanelState> for PanelUser {
    type Rejection = AuthReject;

    async fn from_request_parts(parts: &mut Parts, st: &PanelState) -> Result<Self, Self::Rejection> {
        let (token, session) = require_session(parts, st, false)?;
        Ok(PanelUser { token, session })
    }
}

/// 允许处于「强制改密」状态的用户（改密 / 退出 / 状态查询用）。
pub struct PanelUserAny {
    pub token: String,
    pub session: Session,
}

impl FromRequestParts<PanelState> for PanelUserAny {
    type Rejection = AuthReject;

    async fn from_request_parts(parts: &mut Parts, st: &PanelState) -> Result<Self, Self::Rejection> {
        let (token, session) = require_session(parts, st, true)?;
        Ok(PanelUserAny { token, session })
    }
}

// ============================ 登录 ============================

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub totp_code: String,
}

fn ensure_ready(st: &PanelState) -> Result<(), String> {
    let _ = st;
    ensure_account().map(|_| ()).map_err(|e| e.to_string())
}

/// `GET /api/auth/account`：当前账户的安全设置明细（TOTP / Passkey 列表，不含密钥材料）。
pub async fn account_info(user: PanelUserAny) -> Response {
    let _ = user;
    let acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    Json(json!({
        "username": acc.username,
        "totp": acc.totp.iter().map(|t| json!({
            "id": t.id, "name": t.name, "created_at": t.created_at
        })).collect::<Vec<_>>(),
        "passkeys": acc.passkeys.iter().map(|p| json!({
            "id": p.id, "name": p.name, "created_at": p.created_at, "rp_id": p.rp_id
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// `GET /api/auth/status`：登录页/首页用来判断当前状态（可带 token）。
///
/// 安全（F1）：**未认证时只返回最小信息**，不泄露管理员用户名、是否默认口令、
/// 是否待强制改密、TOTP/Passkey 明细等；这些只在已登录会话中返回。
pub async fn status(State(st): State<PanelState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(e) = ensure_ready(&st) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("面板账户初始化失败：{e}")})),
        )
            .into_response();
    }
    let acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(sess) = token.and_then(|t| touch_session(&st, t)) else {
        // 未登录：仅告知「未登录」+ 是否可用 Passkey 一键登录（登录页按钮需要）
        return Json(json!({
            "logged_in": false,
            "passkey_available": !acc.passkeys.is_empty(),
        }))
        .into_response();
    };
    Json(json!({
        "logged_in": true,
        "username": sess.username,
        "must_change": sess.must_change,
        "verified": sess.verified(),
        "totp_count": acc.totp.len(),
        "passkey_count": acc.passkeys.len(),
        "totp_enabled": !acc.totp.is_empty(),
    }))
    .into_response()
}

/// `POST /api/auth/login`：用户名+密码（若已绑定 TOTP 则强制 2FA）。
///
/// 防 DoS（F3/F10）：
/// - 口令哈希放到 `spawn_blocking`，并发受限（最多 2 个），拿不到闸门立即 429；
/// - 失败计数**按尝试的用户名**分别统计（不存在的用户名只锁它自己，不会锁到真实账户），
///   该用户名连续失败 5 次 → 指数退避（30s→300s）；
/// - 另按来源 IP 统计：60 秒内失败 10 次 → 429（防单 IP 高速爆破）。
pub async fn login(
    State(st): State<PanelState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<LoginReq>,
) -> Response {
    if let Err(e) = ensure_ready(&st) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("面板账户初始化失败：{e}")}))).into_response();
    }
    // 输入上限：避免超长字段带来的解析/哈希放大
    if req.username.chars().count() > 64 || req.password.len() > 1024 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "请求参数不合法"}))).into_response();
    }
    let ip = client_ip(&headers);
    let user_key = req.username.trim().to_ascii_lowercase();
    if let Some(wait) = lock_wait(&st.auth.login_user_fails, &user_key)
        .or_else(|| lock_wait(&st.auth.login_ip_fails, &ip))
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": format!("尝试过于频繁，请 {wait} 秒后重试"), "retry_after": wait})),
        )
            .into_response();
    }
    let mut acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    let user_ok = acc.username == req.username.trim();
    // 无论用户名是否正确都做一次口令校验（消除用户名枚举的时间差）
    let pass_ok = match verify_password(&st, req.password.clone(), acc.password_hash.clone()).await {
        Ok(v) => v,
        Err(()) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "登录请求过多，请稍后再试", "retry_after": 3})),
            )
                .into_response();
        }
    };
    if !user_ok || !pass_ok {
        bump_fail(&st.auth.login_user_fails, &user_key, 5, 30);
        bump_fail(&st.auth.login_ip_fails, &ip, 10, 30);
        acs_core::log::err("面板登录失败：用户名或密码错误");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "用户名或密码错误"})),
        )
            .into_response();
    }
    // TOTP：已绑定则「用户名+密码」登录强制二次验证（Passkey 一键登录不受此限）
    if !acc.totp.is_empty() {
        let code = req.totp_code.trim();
        if code.is_empty() {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "请输入动态验证码", "need_totp": true})),
            )
                .into_response();
        }
        if !acc.totp.iter().any(|t| totp_verify(&t.secret, code)) {
            bump_fail(&st.auth.login_user_fails, &user_key, 5, 30);
            bump_fail(&st.auth.login_ip_fails, &ip, 10, 30);
            acs_core::log::err("面板登录失败：动态验证码错误");
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "动态验证码错误", "need_totp": true})),
            )
                .into_response();
        }
    }
    clear_fails(&st.auth.login_user_fails, &user_key);
    clear_fails(&st.auth.login_ip_fails, &ip);
    // 首次用初始口令登录：记录（此后初始口令只能用于完成强制改密）
    if acc.must_change && !acc.initial_used {
        acc.initial_used = true;
        let _ = save_account(&acc);
    }
    let token = create_session(&st, &acc.username, acc.must_change);
    acs_core::log::info(format!("面板登录成功：{}", acc.username));
    Json(json!({
        "ok": true,
        "token": token,
        "username": acc.username,
        "must_change": acc.must_change,
    }))
    .into_response()
}

/// `POST /api/auth/logout`
pub async fn logout(State(st): State<PanelState>, user: PanelUserAny) -> Response {
    lock(&st.auth.sessions).remove(&user.token);
    Json(json!({ "ok": true })).into_response()
}

/// `POST /api/auth/verify`：再次输入登录密码，获得 5 分钟「已验证窗口」。
#[derive(Deserialize)]
pub struct VerifyReq {
    pub password: String,
}

pub async fn verify(State(st): State<PanelState>, user: PanelUserAny, Json(req): Json<VerifyReq>) -> Response {
    if req.password.len() > 1024 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "请求参数不合法"}))).into_response();
    }
    let acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    let ok = match verify_password(&st, req.password.clone(), acc.password_hash.clone()).await {
        Ok(v) => v,
        Err(()) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "请求过多，请稍后再试", "retry_after": 3})),
            )
                .into_response();
        }
    };
    if !ok {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "密码错误"}))).into_response();
    }
    let until = now() + VERIFY_WINDOW_SECS;
    if let Some(s) = lock(&st.auth.sessions).get_mut(&user.token) {
        s.verified_until = until;
    }
    Json(json!({"ok": true, "verified": true, "verified_until": until})).into_response()
}

// ============================ 改凭据 ============================

#[derive(Deserialize)]
pub struct ChangeCredReq {
    #[serde(default)]
    pub current_password: String,
    pub new_username: String,
    pub new_password: String,
    pub confirm_password: String,
}

/// 校验用户名：3–32 位，字母/数字/`_`/`-`/`.`。
fn valid_username(u: &str) -> bool {
    let len = u.chars().count();
    (3..=32).contains(&len)
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `POST /api/auth/change-credentials`
/// - 强制改密状态：**不需要**旧密码（初始口令已只能用于改密，避免二次暴露）；
/// - 正常状态：必须提供 `current_password`（改密本身即密码验证）。
pub async fn change_credentials(
    State(st): State<PanelState>,
    user: PanelUserAny,
    Json(req): Json<ChangeCredReq>,
) -> Response {
    let mut acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    let new_username = req.new_username.trim().to_string();
    if !valid_username(&new_username) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "用户名需 3–32 位，仅允许字母、数字、下划线、短横线、点"})),
        )
            .into_response();
    }
    if req.new_password.len() > 1024 || req.current_password.len() > 1024 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "请求参数不合法"}))).into_response();
    }
    if user.session.must_change && new_username == acc.username {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "首次登录必须更换用户名（不能与初始用户名相同）"})),
        )
            .into_response();
    }
    if req.new_password.chars().count() < 8 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "新密码至少 8 位"}))).into_response();
    }
    if req.new_password != req.confirm_password {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "两次输入的新密码不一致"}))).into_response();
    }
    if !user.session.must_change {
        let ok = match verify_password(&st, req.current_password.clone(), acc.password_hash.clone()).await {
            Ok(v) => v,
            Err(()) => {
                return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "请求过多，请稍后再试"})))
                    .into_response();
            }
        };
        if !ok {
            return (StatusCode::FORBIDDEN, Json(json!({"error": "当前密码错误"}))).into_response();
        }
        if new_username == acc.username && req.new_password.is_empty() {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "没有需要修改的内容"}))).into_response();
        }
    }
    let new_hash = match hash_password(&st, req.new_password.clone()).await {
        Some(h) => h,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "口令哈希失败"}))).into_response(),
    };
    acc.username = new_username.clone();
    acc.password_hash = new_hash;
    acc.must_change = false;
    acc.updated_at = now();
    if let Err(e) = save_account(&acc) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("保存失败：{e}")}))).into_response();
    }
    // 当前会话同步新用户名/状态；其它会话全部失效
    invalidate_others(&st, &user.token);
    if let Some(s) = lock(&st.auth.sessions).get_mut(&user.token) {
        s.username = new_username.clone();
        s.must_change = false;
        s.verified_until = now() + VERIFY_WINDOW_SECS;
    }
    acs_core::log::info(format!("面板凭据已更新：{}", new_username));
    Json(json!({"ok": true, "username": new_username, "must_change": false, "message": "凭据已更新"}))
        .into_response()
}

// ============================ TOTP ============================

const B32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32_ALPHABET[((buf >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32_ALPHABET[((buf << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in s.chars() {
        let c = c.to_ascii_uppercase();
        if c == '=' || c == ' ' || c == '-' {
            continue;
        }
        let v = B32_ALPHABET.iter().position(|&x| x as char == c)? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// HMAC-SHA1（TOTP / RFC 6238 用）。
fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    use sha1::{Digest, Sha1};
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let mut h = Sha1::new();
        h.update(key);
        let d = h.finalize();
        k[..20].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha1::new();
    inner.update(ipad);
    inner.update(msg);
    let id = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(opad);
    outer.update(id);
    let out = outer.finalize();
    let mut r = [0u8; 20];
    r.copy_from_slice(&out);
    r
}

/// 计算某一时间步的 6 位 TOTP。
pub fn totp_at(secret_b32: &str, step: u64) -> Option<String> {
    let key = base32_decode(secret_b32)?;
    let h = hmac_sha1(&key, &step.to_be_bytes());
    let off = (h[19] & 0x0f) as usize;
    let bin = ((h[off] as u32 & 0x7f) << 24)
        | ((h[off + 1] as u32) << 16)
        | ((h[off + 2] as u32) << 8)
        | (h[off + 3] as u32);
    Some(format!("{:06}", bin % 1_000_000))
}

/// 校验用户输入的 6 位验证码（允许前后各 1 个 30 秒窗口的时钟偏差）。
pub fn totp_verify(secret_b32: &str, code: &str) -> bool {
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let step = (chrono::Utc::now().timestamp() as u64) / 30;
    [step.saturating_sub(1), step, step + 1]
        .into_iter()
        .filter_map(|s| totp_at(secret_b32, s))
        .any(|c| acs_core::password::ct_eq(&c, &code))
}

fn new_totp_secret() -> String {
    let mut b = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut b);
    base32_encode(&b)
}

/// `POST /api/auth/totp/begin`：生成待确认的 TOTP 密钥与 otpauth:// URI。
pub async fn totp_begin(State(st): State<PanelState>, user: PanelUser) -> Response {
    if let Err(e) = user.require_verified() {
        return e.into_response();
    }
    let acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    if acc.totp.len() >= MAX_TOTP {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("最多只能绑定 {MAX_TOTP} 个验证器，请先删除一个")})),
        )
            .into_response();
    }
    let secret = new_totp_secret();
    lock(&st.auth.pending_totp).insert(
        user.token.clone(),
        PendingTotp { secret: secret.clone(), created_at: now() },
    );
    let issuer = "AlphaCoin 服务中心面板";
    let label = format!("{issuer}:{}", acc.username);
    let uri = format!(
        "otpauth://totp/{label}?secret={secret}&issuer={}&algorithm=SHA1&digits=6&period=30",
        urlencode(issuer)
    );
    Json(json!({
        "ok": true,
        "secret": secret,
        "otpauth_uri": uri,
        "digits": 6,
        "period": 30,
        "current_count": acc.totp.len(),
        "max": MAX_TOTP,
    }))
    .into_response()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Deserialize)]
pub struct TotpConfirmReq {
    pub code: String,
    #[serde(default)]
    pub name: String,
}

/// `POST /api/auth/totp/confirm`：校验动态码后正式绑定（最多 3 个）。
pub async fn totp_confirm(
    State(st): State<PanelState>,
    user: PanelUser,
    Json(req): Json<TotpConfirmReq>,
) -> Response {
    if let Err(e) = user.require_verified() {
        return e.into_response();
    }
    let pending = lock(&st.auth.pending_totp).remove(&user.token);
    let Some(pending) = pending else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "请先获取验证器密钥（已过期）"}))).into_response();
    };
    if now() - pending.created_at > 600 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "验证器密钥已过期，请重新获取"}))).into_response();
    }
    if !totp_verify(&pending.secret, &req.code) {
        lock(&st.auth.pending_totp).insert(user.token.clone(), pending);
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "动态验证码不正确"}))).into_response();
    }
    let mut acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    if acc.totp.len() >= MAX_TOTP {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("最多 {MAX_TOTP} 个验证器")}))).into_response();
    }
    let name = if req.name.trim().is_empty() {
        format!("验证器 {}", acc.totp.len() + 1)
    } else {
        req.name.trim().chars().take(32).collect()
    };
    acc.totp.push(TotpEntry {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        secret: pending.secret,
        created_at: now(),
    });
    acc.updated_at = now();
    if let Err(e) = save_account(&acc) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("保存失败：{e}")}))).into_response();
    }
    acs_core::log::info(format!("面板已绑定 TOTP 验证器（共 {} 个）", acc.totp.len()));
    Json(json!({"ok": true, "totp_count": acc.totp.len(), "message": "已绑定：此后密码登录需要动态验证码"}))
        .into_response()
}

#[derive(Deserialize)]
pub struct TotpRemoveReq {
    pub id: String,
}

/// `POST /api/auth/totp/remove`
pub async fn totp_remove(State(st): State<PanelState>, user: PanelUser, Json(req): Json<TotpRemoveReq>) -> Response {
    if let Err(e) = user.require_verified() {
        return e.into_response();
    }
    let _ = &st;
    let mut acc = match load_account() {
        Ok(a) => a,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    };
    let before = acc.totp.len();
    acc.totp.retain(|t| t.id != req.id);
    if acc.totp.len() == before {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "验证器不存在"}))).into_response();
    }
    acc.updated_at = now();
    if let Err(e) = save_account(&acc) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("保存失败：{e}")}))).into_response();
    }
    Json(json!({"ok": true, "totp_count": acc.totp.len(), "message": "验证器已删除"}))
        .into_response()
}
