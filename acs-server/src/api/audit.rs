//! 审计（交易总账单、管理日志）：查看需二次输入密码；支持按时间段导出 .log。
//! 管理日志中创建账户/改密等操作不记录密码明文/哈希等细节。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;

use crate::api::{ApiErr, ApiResult};
use crate::auth::{hash_password, AuthUser};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/audit/unlock", post(unlock_audit))
        .route("/api/audit", get(list_audit))
        .route("/api/audit/export", get(export_audit))
}

/// 二次鉴权：输入当前登录账号密码，换取 10 分钟审计查看权。
#[derive(Deserialize)]
pub struct UnlockReq {
    pub password: String,
}

async fn unlock_audit(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<UnlockReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let pw_hash: String = conn
        .query_row(
            "SELECT password_hash FROM admins WHERE id=?1",
            params![auth.admin_id],
            |r| r.get(0),
        )
        .map_err(ApiErr::from_err)?;
    let (salt, stored) = pw_hash.split_once('$').unwrap_or(("", &pw_hash));
    if hash_password(&req.password, salt) != stored {
        return Err(ApiErr::forbidden("密码错误"));
    }
    st.audit_unlocked
        .lock()
        .unwrap()
        .insert(auth.token.clone(), Utc::now().timestamp() + 600);
    Ok(Json(json!({ "ok": true, "expires_in": 600 })))
}

/// 校验审计二次鉴权（账单/审计/导出共用）。
pub fn require_audit(st: &AppState, auth: &AuthUser) -> ApiResult<()> {
    let map = st.audit_unlocked.lock().unwrap();
    match map.get(&auth.token) {
        Some(exp) if *exp >= Utc::now().timestamp() => Ok(()),
        _ => Err(ApiErr::forbidden("查看审计/账单需先输入密码解锁")),
    }
}

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
}

async fn list_audit(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_audit(&st, &auth)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let conn = st.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, actor, op, detail, ts FROM audit_log ORDER BY id DESC LIMIT ?1")
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "actor": r.get::<_, String>(1)?,
                "op": r.get::<_, String>(2)?,
                "detail": r.get::<_, String>(3)?,
                "ts": r.get::<_, i64>(4)?,
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
pub struct ExportQuery {
    pub from: Option<String>, // YYYY-MM-DD
    pub to: Option<String>,
}

/// 导出管理日志 .log（时间段筛选），直接以文本下载。
async fn export_audit(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    require_audit(&st, &auth)?;
    let conn = st.db.lock().unwrap();
    let mut sql = String::from("SELECT id, actor, op, detail, ts FROM audit_log WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(f) = q.from.as_deref().filter(|f| !f.is_empty()) {
        sql.push_str(" AND date(ts,'unixepoch')>=?");
        params.push(Box::new(f.to_string()));
    }
    if let Some(t) = q.to.as_deref().filter(|t| !t.is_empty()) {
        sql.push_str(" AND date(ts,'unixepoch')<=?");
        params.push(Box::new(t.to_string()));
    }
    sql.push_str(" ORDER BY id ASC");

    let mut stmt = conn.prepare(&sql).map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(ApiErr::from_err)?;
    let mut lines = vec![
        "# Alpha Coin System - 管理日志导出".to_string(),
        format!("# 导出时间: {}", Utc::now().to_rfc3339()),
        "# 操作者 | 操作 | 详情 | 时间戳(UTC)".to_string(),
    ];
    for r in rows {
        let (id, actor, op, detail, ts) = r.map_err(ApiErr::from_err)?;
        lines.push(format!("{id}\t{actor}\t{op}\t{detail}\t{ts}"));
    }
    let body = lines.join("\n");
    let fname = format!("audit_{}_{}.log", q.from.as_deref().unwrap_or("all"), q.to.as_deref().unwrap_or("now"));
    Ok((
        StatusCode::OK,
        [("Content-Type", "text/plain; charset=utf-8"), ("Content-Disposition", &format!("attachment; filename=\"{fname}\""))],
        body,
    )
        .into_response())
}

/// 记录审计日志（敏感操作；改密/建号不记录任何密码细节）。
pub fn log_audit(conn: &Connection, actor: &str, op: &str, detail: &str) {
    let _ = conn.execute(
        "INSERT INTO audit_log(actor, op, detail, ts) VALUES (?1,?2,?3,?4)",
        params![actor, op, detail, Utc::now().timestamp()],
    );
}
