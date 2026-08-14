//! 镜像同步（只读）与镜像 apikey 管理（仅 root）。

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use acs_core::models::AccountType;

use crate::api::audit::log_audit;
use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::AppState;

/// 公开：mirror 拉取（需 apikey）+ 镜像列表 / 状态 / 增量同步（client 免 apikey）。
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/mirror/pull", post(pull))
        .route("/api/mirror/list", get(list_mirrors))
        .route("/api/status", get(status))
        .route("/api/sync", get(sync_public))
}

/// 管理（镜像 apikey 管理 + 社区镜像注册，仅 root）。
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/mirror-keys", get(list_keys).post(create_key))
        .route("/api/admin/mirror-keys/{apikey}", delete(delete_key))
        .route("/api/admin/mirror-registry", get(list_registry).post(add_registry))
        .route("/api/admin/mirror-registry/{url}", delete(delete_registry))
}

#[derive(Deserialize)]
pub struct PullReq {
    pub apikey: String,
    pub since: Option<i64>,
}

async fn pull(
    State(st): State<AppState>,
    Json(req): Json<PullReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mirror_keys WHERE apikey=?1 AND status='Active'",
            params![req.apikey],
            |r| r.get(0),
        )
        .map_err(ApiErr::from_err)?;
    if n == 0 {
        return Err(ApiErr::forbidden("apikey 无效或已停用"));
    }
    let since = req.since.unwrap_or(0);

    let (snapshot, hash) = build_snapshot(&conn, since)?;

    conn.execute(
        "UPDATE mirror_keys SET last_pull_at=?1 WHERE apikey=?2",
        params![Utc::now().timestamp(), req.apikey],
    )
    .map_err(ApiErr::from_err)?;

    let central_sig = crate::api::keys::try_sign_hash(&st, &hash);
    Ok(Json(json!({ "ok": true, "hash": hash, "central_sig": central_sig, "data": snapshot })))
}

/// 构建增量账本快照（Confirmed 交易 + 各表账户 + sha256），供 pull 与公开 /api/sync 共用。
fn build_snapshot(conn: &Connection, since: i64) -> Result<(serde_json::Value, String), ApiErr> {
    let mut tstmt = conn
        .prepare(
            "SELECT tx_id, tx_type, sender, sender_type, receiver, receiver_type, amount, ts, tx_hash, central_sig, status \
             FROM transactions WHERE status='Confirmed' AND ts>?1 ORDER BY ts ASC",
        )
        .map_err(ApiErr::from_err)?;
    let trows = tstmt
        .query_map(params![since], |r| {
            Ok(json!({
                "tx_id": r.get::<_, String>(0)?,
                "tx_type": r.get::<_, String>(1)?,
                "sender": r.get::<_, String>(2)?,
                "sender_type": r.get::<_, String>(3)?,
                "receiver": r.get::<_, String>(4)?,
                "receiver_type": r.get::<_, String>(5)?,
                "amount": r.get::<_, i64>(6)?,
                "timestamp": r.get::<_, i64>(7)?,
                "tx_hash": r.get::<_, String>(8)?,
                "central_sig": r.get::<_, Option<String>>(9)?,
                "status": r.get::<_, String>(10)?,
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut txs = Vec::new();
    for r in trows {
        txs.push(r.map_err(ApiErr::from_err)?);
    }

    let mut accounts = Vec::new();
    for at in [AccountType::Country, AccountType::Company, AccountType::Individual, AccountType::System] {
        let table = at.table_name();
        let mut astmt = conn
            .prepare(&format!("SELECT uid, balance, status, last_tx_hash, changed_at FROM {table}"))
            .map_err(ApiErr::from_err)?;
        let arows = astmt
            .query_map([], |r| {
                Ok(json!({
                    "uid": r.get::<_, String>(0)?,
                    "type": at.as_str(),
                    "balance": r.get::<_, i64>(1)?,
                    "status": r.get::<_, String>(2)?,
                    "last_tx_hash": r.get::<_, Option<String>>(3)?,
                    "changed_at": r.get::<_, i64>(4)?,
                }))
            })
            .map_err(ApiErr::from_err)?;
        for r in arows {
            accounts.push(r.map_err(ApiErr::from_err)?);
        }
    }

    let snapshot = json!({
        "since": since,
        "server_time": Utc::now().timestamp(),
        "transactions": txs,
        "accounts": accounts,
    });
    let snap_str = serde_json::to_string(&snapshot).map_err(ApiErr::from_err)?;
    let mut h = Sha256::new();
    h.update(snap_str.as_bytes());
    let hash = hex::encode(h.finalize());
    Ok((snapshot, hash))
}

/// 公开：可用镜像列表（client 拉取镜像地址，无需 apikey）。
async fn list_mirrors(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT url, name, note FROM mirror_registry WHERE status='Active' ORDER BY name")
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "url": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "note": r.get::<_, String>(2)?,
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(ApiErr::from_err)?);
    }
    Ok(Json(json!({ "mirrors": items })))
}

/// 公开：健康/延迟探测。
async fn status(State(_st): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "name": "acs-server",
        "version": env!("CARGO_PKG_VERSION"),
        "server_time": Utc::now().timestamp(),
    }))
}

#[derive(Deserialize)]
pub struct SyncQuery {
    pub since: Option<i64>,
}

/// 公开：增量同步（client 免 apikey 从中心直拉）。
async fn sync_public(
    State(st): State<AppState>,
    Query(q): Query<SyncQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let since = q.since.unwrap_or(0);
    let conn = st.db.lock().unwrap();
    let (snapshot, hash) = build_snapshot(&conn, since)?;
    let central_sig = crate::api::keys::try_sign_hash(&st, &hash);
    Ok(Json(json!({ "ok": true, "hash": hash, "central_sig": central_sig, "data": snapshot })))
}

async fn list_keys(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可管理镜像 apikey"));
    }
    let conn = st.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT apikey, name, status, last_pull_at FROM mirror_keys ORDER BY name")
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "apikey": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
                "last_pull_at": r.get::<_, Option<i64>>(3)?,
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
pub struct CreateKeyReq {
    pub name: String,
}

async fn create_key(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateKeyReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可管理镜像 apikey"));
    }
    let apikey = format!("mir-{}", uuid::Uuid::new_v4().simple());
    let conn = st.db.lock().unwrap();
    conn.execute(
        "INSERT INTO mirror_keys(apikey, name, status, last_pull_at) VALUES (?1,?2,'Active',NULL)",
        params![apikey, req.name.trim()],
    )
    .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "create_mirror_key", &req.name);
    Ok(Json(json!({ "ok": true, "apikey": apikey })))
}

async fn delete_key(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(apikey): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可管理镜像 apikey"));
    }
    let conn = st.db.lock().unwrap();
    conn.execute("DELETE FROM mirror_keys WHERE apikey=?1", params![apikey])
        .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "delete_mirror_key", &apikey);
    Ok(Json(json!({ "ok": true })))
}

// ---------- 社区镜像注册（client 从此列表发现镜像） ----------

async fn list_registry(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可管理镜像注册"));
    }
    let conn = st.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT url, name, note, status, created_at FROM mirror_registry ORDER BY name")
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "url": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "note": r.get::<_, String>(2)?,
                "status": r.get::<_, String>(3)?,
                "created_at": r.get::<_, i64>(4)?,
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
pub struct AddRegistryReq {
    pub url: String,
    pub name: Option<String>,
    pub note: Option<String>,
}

async fn add_registry(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<AddRegistryReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可管理镜像注册"));
    }
    let url = req.url.trim().trim_end_matches('/').to_string();
    if url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiErr::bad_request("镜像地址需以 http(s):// 开头"));
    }
    let conn = st.db.lock().unwrap();
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT INTO mirror_registry(url, name, note, status, created_at) VALUES (?1,?2,?3,'Active',?4) \
         ON CONFLICT(url) DO UPDATE SET name=excluded.name, note=excluded.note, status='Active'",
        params![
            url,
            req.name.clone().unwrap_or_default().trim(),
            req.note.clone().unwrap_or_default().trim(),
            now
        ],
    )
    .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "add_mirror_registry", &url);
    Ok(Json(json!({ "ok": true, "url": url })))
}

async fn delete_registry(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(url): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可管理镜像注册"));
    }
    let url = url.replace("%2F", "/").replace("%3A", ":");
    let conn = st.db.lock().unwrap();
    conn.execute("DELETE FROM mirror_registry WHERE url=?1", params![url])
        .map_err(ApiErr::from_err)?;
    log_audit(&conn, &auth.username, "delete_mirror_registry", &url);
    Ok(Json(json!({ "ok": true })))
}
