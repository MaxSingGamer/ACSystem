//! REST API：统一错误类型 + 路由装配。

pub mod accounts;
pub mod admins;
pub mod audit;
pub mod client;
pub mod keys;
pub mod members;
pub mod stats;
pub mod sync;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

/// 全请求日志中间件：每个进入的 HTTP 请求写入 .alphalog（NET 记录方法与路径，OUT 记录状态码）。
/// 用于「全调用 / 全通讯 / 全错误」无死角记录；查询串与路径经 mask 脱敏（避免泄路径泄露敏感名）。
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
    crate::log::net(&format!("{line} (ip={})", crate::log::mask(&ip)));
    let started = std::time::Instant::now();
    let resp = next.run(req).await;
    let status = resp.status();
    crate::log::out(&format!("{method} {path} → HTTP {status}（{}ms）", started.elapsed().as_millis()));
    resp
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
            E::HashMismatch(m) => Self::bad_request(format!("哈希链不一致: {m}")),
            E::Unauthorized(m) => Self::forbidden(m),
            E::InvalidCode => Self::bad_request("验证码无效"),
            other => Self::internal(other.to_string()),
        }
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiErr>;

/// 管理侧路由（后台网页 + 管理 API；仅内网监听，不开放公网）。
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/login", post(crate::auth::login))
        .route("/api/admin/logout", post(crate::auth::logout))
        .route("/api/admin/me", get(crate::auth::me))
        .route("/api/admin/change-password", post(crate::auth::change_password))
        .merge(accounts::routes())
        .merge(stats::routes())
        .merge(admins::routes())
        .merge(members::routes())
        .merge(audit::routes())
        .merge(keys::routes())
}

/// 公开路由（client 调用；对外监听，无网页）。
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .merge(client::routes())
        .merge(sync::routes())
        .merge(crate::update::routes())
}
