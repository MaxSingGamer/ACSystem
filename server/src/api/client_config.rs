//! 客户端配置下发（v3.3.0 #7 / D39）。
//!
//! 客户端不再读取任何 `.env` / 编译期注入：品牌、中心识别、更新源与少量行为参数
//! 统一从本接口获取（离线时由客户端缓存上一份）。
//!
//! **隐私红线**：本接口只返回下面这份**显式白名单**，绝不透出 `.env` 内容或任何
//! 账号/口令/路径/绑定地址/安全参数（D39 已逐项确认）。新增字段必须先改白名单并补测试。

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/client/config", get(client_config))
}

/// 口令最短长度（与客户端注册/登录校验、后台改密校验保持一致）。
pub const MIN_PASSWORD_LEN: usize = 8;

async fn client_config(State(st): State<AppState>) -> Json<serde_json::Value> {
    let b = acs_core::brand::brand();
    let public_port: u16 = std::env::var("ACS_PUBLIC_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(9600);
    Json(json!({
        // ---------- A 档：客户端功能直接依赖 ----------
        "brand_name": b.name,
        "currency": b.currency,
        "union_name": b.union_name,
        "union_name_cn": b.union_name_cn,
        "union_abbr": b.union_abbr,
        "system_name": b.system_name,
        "client_app": b.client_app,
        "public_url": b.public_url,
        "update_repo": b.update_repo,
        // 服务名称：面板以 -name 启动参数传入（D26）；未由面板管理时为空串
        "service_name": st.service_name,
        // ---------- B 档（D39：只公开这两项） ----------
        "public_port": public_port,
        "min_password_len": MIN_PASSWORD_LEN,
        // ---------- 协议版本：便于将来客户端判断接口兼容性 ----------
        "config_version": 1,
    }))
}
