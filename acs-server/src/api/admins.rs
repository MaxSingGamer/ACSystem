//! 后台管理员账户管理（仅 root）。
//!
//! - root 可创建 finance / root 管理员（创建时自动生成 gpg 密钥对）
//! - root 不可修改/删除另一个 root（同级互不可改）
//! - root 可停用/启用 finance
//! 密码：仅在服务端以哈希存储；密钥 passphrase 以 AES 加密存储。

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;

use acs_core::models::AdminRole;

use crate::api::audit::log_audit;
use crate::api::{ApiErr, ApiResult};
use crate::auth::{hash_password, AuthUser};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admins", get(list_admins).post(create_admin))
        .route("/api/admins/{id}", delete(delete_admin))
        .route("/api/admins/{id}/disable", post(disable_admin))
        .route("/api/admins/{id}/enable", post(enable_admin))
}

async fn list_admins(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可查看后台账号"));
    }
    let conn = st.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, uid, role, status, must_change_password, created_at FROM admins ORDER BY id")
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "uid": r.get::<_, String>(1)?,
                "role": r.get::<_, String>(2)?,
                "status": r.get::<_, String>(3)?,
                "must_change_password": r.get::<_, bool>(4)?,
                "created_at": r.get::<_, i64>(5)?,
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
pub struct CreateAdminReq {
    pub uid: String,
    pub password: String,
    pub role: Option<String>,
}

async fn create_admin(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateAdminReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可创建后台账号"));
    }
    let uid = req.uid.trim().to_string();
    if uid.is_empty() || req.password.len() < 8 {
        return Err(ApiErr::bad_request("用户名不能为空，密码至少 8 位"));
    }
    let role = AdminRole::from_str(req.role.as_deref().unwrap_or("finance"))
        .ok_or_else(|| ApiErr::bad_request("角色无效"))?;

    let conn = st.db.lock().unwrap();
    let salt = uuid::Uuid::new_v4().to_string();
    let pw_hash = format!("{salt}${}", hash_password(&req.password, &salt));
    conn.execute(
        "INSERT INTO admins(uid, role, password_hash, must_change_password, pubkey, encrypted_seckey, fingerprint, key_passphrase_enc, status, created_at) \
         VALUES (?1,?2,?3,1,'','','','','Active',?4)",
        params![uid, role.as_str(), pw_hash, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| ApiErr::bad_request(format!("创建失败（用户名可能已存在）: {e}")))?;

    let id: i64 = conn
        .query_row("SELECT id FROM admins WHERE uid=?1", params![uid], |r| r.get(0))
        .map_err(ApiErr::from_err)?;
    // 自动生成 gpg 密钥对
    crate::auth::ensure_admin_keys(&conn, &st.gpg, id, &uid, &req.password).map_err(ApiErr::from)?;
    log_audit(&conn, &auth.username, "create_admin", &uid);
    Ok(Json(json!({ "ok": true, "uid": uid, "role": role.as_str() })))
}

async fn delete_admin(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可删除后台账号"));
    }
    if id == auth.admin_id {
        return Err(ApiErr::bad_request("不能删除自己"));
    }
    let conn = st.db.lock().unwrap();
    let row: Option<(i64, String)> = conn
        .query_row("SELECT id, role FROM admins WHERE id=?1", params![id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(ApiErr::from_err)?;
    let Some((_, role)) = row else {
        return Err(ApiErr::not_found("后台账号不存在"));
    };
    if AdminRole::from_str(&role) == Some(AdminRole::Root) {
        return Err(ApiErr::forbidden("同级别（根管理员）不可互相删除"));
    }
    conn.execute("DELETE FROM admins WHERE id=?1", params![id]).map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "delete_admin", &format!("id={id}"));
    Ok(Json(json!({ "ok": true })))
}

async fn set_enabled(
    st: &AppState,
    auth: &AuthUser,
    id: i64,
    enable: bool,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可停用/启用后台账号"));
    }
    if id == auth.admin_id {
        return Err(ApiErr::bad_request("不能停用自己"));
    }
    let conn = st.db.lock().unwrap();
    let row: Option<(i64, String)> = conn
        .query_row("SELECT id, role FROM admins WHERE id=?1", params![id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(ApiErr::from_err)?;
    let Some((_, role)) = row else {
        return Err(ApiErr::not_found("后台账号不存在"));
    };
    if AdminRole::from_str(&role) == Some(AdminRole::Root) {
        return Err(ApiErr::forbidden("同级别（根管理员）不可停用"));
    }
    let status = if enable { "Active" } else { "Disabled" };
    conn.execute("UPDATE admins SET status=?1 WHERE id=?2", params![status, id])
        .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, if enable { "enable_admin" } else { "disable_admin" }, &format!("id={id}"));
    Ok(Json(json!({ "ok": true, "status": status })))
}

async fn disable_admin(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    set_enabled(&st, &auth, id, false).await
}

async fn enable_admin(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    set_enabled(&st, &auth, id, true).await
}
