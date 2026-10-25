//! 后台管理员账户管理（仅 Root）。
//!
//! v3.3.0（D17/D18/D28）：
//! - **Root 账户首启后不可增减**（由 `.env` 的 `ACS_ROOT_ACCOUNTS` 在首次建库时创建），
//!   本模块的创建/删除/停用/启用**只对 Admin 生效**，对 Root 一律拒绝。
//! - Root 可创建 **Admin**（原「金融部」Finance）；Admin **不生成 gpg 密钥**（不签名）。
//! - Root 可停用/启用/删除 Admin，并可强制重置 Admin 密码（`/api/admin/change-password`，
//!   无需旧密码；见 `auth::change_password`）。
//! 密码：仅在服务端以哈希存储。

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;

use acs_core::models::AdminRole;

use crate::api::audit::{log_audit, with_audit};
use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::password;
use crate::state::{self, AppState};

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
    let conn = state::db_lock(&st.db);
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
}

/// 创建 **Admin** 账户（Root 专属；Root 不可再被创建 —— D17）。
///
/// 注：**不生成 gpg 密钥**（Admin 不签名，D28）；`key_passphrase_enc` 等列留空。
async fn create_admin(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateAdminReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可创建管理员账号"));
    }
    let uid = req.uid.trim().to_string();
    if uid.is_empty() || req.password.len() < 8 {
        return Err(ApiErr::bad_request("用户名不能为空，密码至少 8 位"));
    }
    // v3.3.0（D19）：创建 Admin 属「根管理员授权动作」，要求主密钥已解锁
    // （根管理员登录时已用自己口令解开 masterseckey 副本并载入内存；此校验用于
    //   防止副本损坏/未初始化时仍能越权建号）。未解锁 → 409 并提示重新登录。
    if state::master_lock(&st).is_none() {
        return Err(ApiErr::conflict(
            "主密钥未解锁：请由根管理员重新登录后再创建管理员（登录即解锁）",
        ));
    }

    let conn = state::db_lock(&st.db);
    let pw_hash = password::hash(&req.password);
    // 角色固定为 admin：Root 账户只能在首次建库时由 .env 定义（首启后不可增减）
    conn.execute(
        "INSERT INTO admins(uid, role, password_hash, must_change_password, pubkey, encrypted_seckey, fingerprint, key_passphrase_enc, status, created_at) \
         VALUES (?1,'admin',?2,1,'','','','','Active',?3)",
        params![uid, pw_hash, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| ApiErr::bad_request(format!("创建失败（用户名可能已存在）: {e}")))?;
    log_audit(&conn, &auth.username, "create_admin", &uid);
    Ok(Json(json!({ "ok": true, "uid": uid, "role": AdminRole::Admin.as_str() })))
}

/// 删除 **Admin**（Root 专属；Root 不可被删除 —— D17）。
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
    let mut conn = state::db_lock(&st.db);
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
        return Err(ApiErr::forbidden("Root 账户首启后不可删减（只能删除 Admin）"));
    }
    with_audit(&mut conn, &auth.username, "delete_admin", &format!("id={id}"), |c| {
        c.execute("DELETE FROM admins WHERE id=?1", params![id])
            .map_err(ApiErr::from_err)?;
        Ok(())
    })?;
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
    let mut conn = state::db_lock(&st.db);
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
        return Err(ApiErr::forbidden("Root 账户不可停用（首启后不可增减）"));
    }
    let status = if enable { "Active" } else { "Disabled" };
    let op = if enable { "enable_admin" } else { "disable_admin" };
    with_audit(&mut conn, &auth.username, op, &format!("id={id}"), |c| {
        c.execute("UPDATE admins SET status=?1 WHERE id=?2", params![status, id])
            .map_err(ApiErr::from_err)?;
        Ok(())
    })?;
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
