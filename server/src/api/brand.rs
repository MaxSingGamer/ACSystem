//! 品牌元信息（公开端点）：把服务端 `.env` 里的 `ACS_BRAND_*` 暴露给前端，
//! 让管理后台与客户端界面不必硬编码品牌名/货币符号（多部署复用同一份代码）。

use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::AppState;

/// 公开：品牌与部署元信息（不含任何敏感信息）。
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/brand", get(brand))
}

async fn brand() -> Json<serde_json::Value> {
    let b = acs_core::brand::brand();
    Json(json!({
        "ok": true,
        "name": b.name,
        "currency": b.currency,
        "system_name": b.system_name,
        "union_name": b.union_name,
        "union_name_cn": b.union_name_cn,
        "union_abbr": b.union_abbr,
        "server_app": b.server_app,
        "client_app": b.client_app,
        "public_url": b.public_url,
        "admin_url": b.admin_url,
    }))
}
