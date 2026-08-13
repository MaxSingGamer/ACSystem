//! AEU 成员注册表管理（countries / companies）。
//! countries 由 root（理事会）维护；companies 由 root/finance（金融部）维护。

use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use acs_core::models::AdminRole;

use crate::api::audit::log_audit;
use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/members/{kind}", get(list_members).post(create_member))
        .route("/api/members/{kind}/{id}", put(update_member).delete(delete_member))
}

fn table_of(kind: &str) -> Option<&'static str> {
    match kind {
        "countries" => Some("member_countries"),
        "companies" => Some("member_companies"),
        _ => None,
    }
}

fn can_manage(auth: &AuthUser, kind: &str) -> bool {
    match kind {
        "countries" => auth.is_root(),
        "companies" => auth.is_root() || auth.role == AdminRole::Finance,
        _ => false,
    }
}

async fn list_members(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(kind): Path<String>,
) -> ApiResult<Json<Value>> {
    let table = table_of(&kind).ok_or_else(|| ApiErr::bad_request("未知成员类型"))?;
    if !can_manage(&auth, &kind) {
        return Err(ApiErr::forbidden("无权管理该成员注册表"));
    }
    let conn = st.db.lock().unwrap();
    let mut stmt = conn
        .prepare(&format!("SELECT id, name, status FROM {table} ORDER BY id"))
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "name": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(ApiErr::from_err)?);
    }
    Ok(Json(json!({ "kind": kind, "items": items })))
}

async fn create_member(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(kind): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let table = table_of(&kind).ok_or_else(|| ApiErr::bad_request("未知成员类型"))?;
    if !can_manage(&auth, &kind) {
        return Err(ApiErr::forbidden("无权管理该成员注册表"));
    }
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiErr::bad_request("缺少 name"))?;
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("Active");
    let conn = st.db.lock().unwrap();
    conn.execute(
        &format!("INSERT INTO {table}(name, status) VALUES (?1,?2)"),
        params![name, status],
    )
    .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "create_member", &format!("{kind} {name}"));
    Ok(Json(json!({ "ok": true })))
}

async fn update_member(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((kind, id)): Path<(String, i64)>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let table = table_of(&kind).ok_or_else(|| ApiErr::bad_request("未知成员类型"))?;
    if !can_manage(&auth, &kind) {
        return Err(ApiErr::forbidden("无权管理该成员注册表"));
    }
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("Active");
    let conn = st.db.lock().unwrap();
    // name 留空表示仅改状态（保留原名）
    let n = if name.trim().is_empty() {
        conn.execute(
            &format!("UPDATE {table} SET status=?1 WHERE id=?2"),
            params![status, id],
        )
        .map_err(ApiErr::from_err)?
    } else {
        conn.execute(
            &format!("UPDATE {table} SET name=?1, status=?2 WHERE id=?3"),
            params![name, status, id],
        )
        .map_err(ApiErr::from_err)?
    };
    if n == 0 {
        return Err(ApiErr::not_found("成员不存在"));
    }
    log_audit(&conn, &auth.username, "update_member", &format!("{kind} id={id}"));
    Ok(Json(json!({ "ok": true })))
}

async fn delete_member(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((kind, id)): Path<(String, i64)>,
) -> ApiResult<Json<Value>> {
    let table = table_of(&kind).ok_or_else(|| ApiErr::bad_request("未知成员类型"))?;
    if !can_manage(&auth, &kind) {
        return Err(ApiErr::forbidden("无权管理该成员注册表"));
    }
    let conn = st.db.lock().unwrap();
    conn.execute(&format!("DELETE FROM {table} WHERE id=?1"), params![id])
        .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "delete_member", &format!("{kind} id={id}"));
    Ok(Json(json!({ "ok": true })))
}
