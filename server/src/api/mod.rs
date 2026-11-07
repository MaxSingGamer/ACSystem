//! REST API：统一错误类型 + 路由装配。

pub mod accounts;
pub mod admins;
pub mod audit;
pub mod brand;
pub mod client;
pub mod client_config;
pub mod keys;
pub mod stats;
pub mod sync;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::state::AppState;

// ===================== DDoS 防护（第二回合后加固） =====================

/// 请求日志每秒最多记录多少条（超出的请求日志被折叠，仅保留错误响应）。
fn log_lines_per_sec() -> u32 {
    std::env::var("ACS_LOG_MAX_LINES_PER_SEC")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(80)
}

#[derive(Default)]
struct LogQuota {
    sec: i64,
    used: u32,
    suppressed: u64,
}

fn log_quota() -> &'static Mutex<LogQuota> {
    static Q: OnceLock<Mutex<LogQuota>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(LogQuota::default()))
}

#[derive(Default)]
struct TokenBucket {
    tokens: f64,
    last: Option<Instant>,
    dropped: u64,
    dropped_sec: i64,
}

fn rate_limits() -> (f64, f64, f64, f64) {
    static V: OnceLock<(f64, f64, f64, f64)> = OnceLock::new();
    *V.get_or_init(|| {
        let num = |k: &str, d: f64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|n| *n > 0.0)
                .unwrap_or(d)
        };
        (
            num("ACS_RATE_LIMIT_RPS", 50.0),
            num("ACS_RATE_LIMIT_BURST", 100.0),
            num("ACS_ADMIN_RATE_LIMIT_RPS", 30.0),
            num("ACS_ADMIN_RATE_LIMIT_BURST", 60.0),
        )
    })
}

fn rate_buckets() -> &'static Mutex<HashMap<String, TokenBucket>> {
    static B: OnceLock<Mutex<HashMap<String, TokenBucket>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 令牌桶：返回 `Ok(())` 放行；`Err(剩余秒)` 表示应回 429。
fn take_token(key: &str, rps: f64, burst: f64) -> Result<(), i64> {
    let now_i = chrono::Utc::now().timestamp();
    let mut map = crate::state::lock(rate_buckets());
    let b = map.entry(key.to_string()).or_default();
    let now = Instant::now();
    if b.last.is_none() {
        // 首次使用：桶初始为满（否则第一个请求会被误判 429）
        b.tokens = burst;
    }
    let elapsed = b.last.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
    b.last = Some(now);
    b.tokens = (b.tokens + elapsed * rps).min(burst);
    if b.tokens >= 1.0 {
        b.tokens -= 1.0;
        Ok(())
    } else {
        b.dropped = b.dropped.saturating_add(1);
        if b.dropped_sec != now_i {
            // 每秒最多记一条限流汇总，避免「拒绝日志」本身放大磁盘写入
            let dropped = b.dropped;
            b.dropped = 0;
            b.dropped_sec = now_i;
            crate::log::err(format!("限流拦截 {dropped} 个请求（{key}，{rps:.0} rps）"));
        }
        let wait = ((1.0 - b.tokens) / rps).ceil() as i64;
        Err(wait.max(1))
    }
    // 防止 map 无限增长
    // 说明：正常只有少量来源 IP；这里在 map 过大时清掉长期未活跃的桶。
}

pub async fn rate_limit_public(req: Request, next: Next) -> Response {
    rate_limit(req, next, false).await
}

pub async fn rate_limit_admin(req: Request, next: Next) -> Response {
    rate_limit(req, next, true).await
}

async fn rate_limit(req: Request, next: Next, admin: bool) -> Response {
    let (pr, pb, ar, ab) = rate_limits();
    let (rps, burst) = if admin { (ar, ab) } else { (pr, pb) };
    let ip = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string());
    if let Err(wait) = take_token(&format!("{}:{}", ip, if admin { "a" } else { "p" }), rps, burst) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, wait.to_string())],
            Json(serde_json::json!({ "error": "请求过于频繁，请稍后重试", "retry_after": wait })),
        )
            .into_response();
    }
    next.run(req).await
}

/// 同时在处理的请求上限（默认 256）；超限立即 503，避免排队无限增长。
fn inflight_limit() -> usize {
    std::env::var("ACS_MAX_INFLIGHT")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(256)
}

fn inflight_sem() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(inflight_limit()))
}

pub async fn inflight_cap(req: Request, next: Next) -> Response {
    let Ok(permit) = inflight_sem().try_acquire() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "1".to_string())],
            Json(serde_json::json!({ "error": "服务繁忙，请稍后重试" })),
        )
            .into_response();
    };
    let resp = next.run(req).await;
    drop(permit);
    resp
}

/// 全请求日志中间件：每个进入的 HTTP 请求写入 .alphalog（NET 记录方法与路径，OUT 记录状态码）。
/// v4.0.0（DDoS 加固）：非错误请求的日志按 `ACS_LOG_MAX_LINES_PER_SEC`（默认 80 行/秒）限流，
/// 超出的请求不再逐条落盘（避免「日志写入放大」成为 DoS 放大器）；4xx/5xx 响应始终记录。
pub async fn log_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let ip = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".to_string());
    let line = if query.is_empty() {
        format!("{method} {path}")
    } else {
        format!("{method} {path}?{query}")
    };
    // 日志配额（1 秒窗口）
    let (allow, summary) = {
        let now_i = chrono::Utc::now().timestamp();
        let mut q = crate::state::lock(log_quota());
        let mut summary = None;
        if q.sec != now_i {
            if q.suppressed > 0 {
                summary = Some(format!(
                    "请求日志限流：上 1 秒折叠 {} 条（ACS_LOG_MAX_LINES_PER_SEC={}）",
                    q.suppressed,
                    log_lines_per_sec()
                ));
            }
            q.sec = now_i;
            q.used = 0;
            q.suppressed = 0;
        }
        if q.used < log_lines_per_sec() {
            q.used += 1;
            (true, summary)
        } else {
            q.suppressed += 1;
            (false, summary)
        }
    };
    if let Some(s) = summary {
        crate::log::info(s);
    }
    if allow {
        crate::log::net(&format!("{line} (ip={})", crate::log::mask(&ip)));
    }
    let started = Instant::now();
    let resp = next.run(req).await;
    let status = resp.status();
    // 错误响应始终记录（审计需要）；成功响应受配额约束。
    if allow || status.is_client_error() || status.is_server_error() {
        crate::log::out(&format!(
            "{method} {path} → HTTP {status}（{}ms）",
            started.elapsed().as_millis()
        ));
    }
    resp
}

/// 把 axum/serde 的原始提取错误（JSON body / Query 反序列化失败）替换成统一文案。
///
/// F7/F9：框架默认返回 `text/plain` 的 “Failed to deserialize ...” 文本，
/// 会泄露结构体/字段名与实现细节；这里统一改为 JSON `{"error":"请求格式错误"}`。
pub async fn sanitize_errors(req: Request, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api");
    let res = next.run(req).await;
    if !is_api {
        return res;
    }
    let status = res.status();
    if status != StatusCode::BAD_REQUEST && status != StatusCode::UNPROCESSABLE_ENTITY {
        return res;
    }
    let is_plain = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/plain"))
        .unwrap_or(false);
    if !is_plain {
        return res;
    }
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": "请求格式错误" })),
    )
        .into_response()
}

#[derive(Serialize)]
pub struct ApiErrorBody {
    pub error: String,
}

pub struct ApiErr {
    pub status: StatusCode,
    pub message: String,
}

impl ApiErr {
    pub fn new(status: StatusCode, m: impl Into<String>) -> Self {
        ApiErr { status, message: m.into() }
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
    pub fn too_many_requests(m: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, m)
    }
    /// 409：请求本身合法，但服务端当前状态不允许（例如主密钥尚未解锁）。
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, m)
    }
    pub fn from_err(e: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        crate::log::err(&format!("API {}：{}", self.status.as_u16(), self.message));
        (self.status, Json(ApiErrorBody { error: self.message })).into_response()
    }
}

impl From<acs_core::errors::AcsError> for ApiErr {
    fn from(e: acs_core::errors::AcsError) -> Self {
        use acs_core::errors::AcsError as E;
        match e {
            E::AccountNotFound(m) => Self::not_found(m),
            E::AccountExists(m) => Self::bad_request(format!("账户已存在: {m}")),
            E::InsufficientBalance => Self::bad_request("余额不足"),
            E::AccountNotActive => Self::bad_request("账户未激活（冻结或关闭）"),
            // 注销 / 冻结 / 关闭账户的交易拦截：403 + 可读原因
            E::AccountBlocked(m) => Self::forbidden(m),
            E::HashMismatch(m) => Self::bad_request(format!("哈希链不一致: {m}")),
            E::Unauthorized(m) => Self::forbidden(m),
            E::InvalidCode => Self::bad_request("验证码无效"),
            // 以下均为「调用方可修正」的业务错误：必须回 4xx，
            // 否则客户端会把「状态终态不可变更」「交易已处理」这类正常的规则拒绝当成服务端故障。
            E::Message(m) => Self::bad_request(m),
            E::InvalidArgument(m) => Self::bad_request(format!("非法参数: {m}")),
            E::SignatureInvalid => Self::forbidden("签名无效"),
            E::KeyLocked => Self::forbidden("中心密钥未解锁"),
            // 其余（Db / Io / Gpg / Config）属服务端自身故障 → 500
            other => Self::internal(other.to_string()),
        }
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiErr>;

/// 管理侧路由（后台网页 + 管理 API）。
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/login", post(crate::auth::login))
        .route("/api/admin/logout", post(crate::auth::logout))
        .route("/api/admin/me", get(crate::auth::me))
        .route("/api/admin/change-password", post(crate::auth::change_password))
        .merge(crate::sysw::routes())
        .merge(accounts::routes())
        .merge(stats::routes())
        .merge(admins::routes())
        .merge(audit::routes())
        .merge(keys::routes())
        .merge(sync::admin_routes())
}

/// 公开路由（client 调用；对外监听，无网页）。
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .merge(client::routes())
        .merge(client_config::routes())
        .merge(sync::routes())
        .merge(brand::routes())
        .merge(crate::update::routes())
}
