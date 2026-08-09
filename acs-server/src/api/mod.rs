//! REST API：统一错误类型 + 路由装配。

pub mod accounts;
pub mod admins;
pub mod audit;
pub mod client;
pub mod keys;
pub mod members;
pub mod mirror;
pub mod stats;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

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
        .merge(mirror::admin_routes())
}

/// 公开路由（client / mirror 调用；对外监听，仅 apikey 认证，无网页）。
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .merge(client::routes())
        .merge(mirror::public_routes())
}
