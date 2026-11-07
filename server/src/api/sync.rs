//! 同步（v3.0.0 取消镜像；v4.0.0 安全加固）：
//! - `/api/status`：健康/延迟探测（公开）
//! - `/api/sync`：**需账户签名的增量同步**（只返回该账户相关交易与自身账户快照）
//! - `/api/admin/sync`：后台管理员全量快照（需 Bearer token，供运维/回归测试）
//!
//! 签名协议（F4 修复）：
//! ```text
//! X-ACS-Uid:  <uid>
//! X-ACS-Type: <Individual|Country|Company|System>
//! X-ACS-Ts:   <unix 秒>
//! X-ACS-Sig:  <对 "sync:{uid}:{type}:{since}:{ts}" 的分离签名（armored）>
//! ```
//! 签名公钥从账户表取；时间戳允许 ±300 秒偏差。

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use acs_core::models::AccountType;

use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::{self, AppState};

/// 公开：状态 + 需签名的增量同步。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/sync", get(sync_public))
}

/// 管理侧：全量账本快照（需登录）。
pub fn admin_routes() -> Router<AppState> {
    Router::new().route("/api/admin/sync", get(sync_admin))
}

/// 公开：健康/延迟探测。
///
/// v3.3.0（D26）：额外返回 `service_name`（由面板以 `-name` 启动参数传入），
/// 供客户端把「中心服务」以**服务名称**展示与区分（#15）；未传时为空串。
/// v4.0.0（F5）：不再返回 `version`，减少指纹信息。
async fn status(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "name": "AlphaCoinServer",
        "service_name": st.service_name,
        "server_time": Utc::now().timestamp(),
    }))
}

#[derive(Deserialize)]
pub struct SyncQuery {
    pub since: Option<i64>,
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// HTTP 头不允许换行，客户端传的是「单行 base64 的 armored 签名 body」；
/// 这里校验字符集并重新包装成标准 armored 文本，交给 gpg 验签。
fn rearmor_signature(payload: &str) -> Option<String> {
    let payload = payload.trim();
    if payload.is_empty() || payload.len() > 8192 {
        return None;
    }
    if !payload
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
    {
        return None;
    }
    let mut out = String::with_capacity(payload.len() + 64);
    out.push_str("-----BEGIN PGP SIGNATURE-----\n\n");
    for chunk in payload.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).ok()?);
        out.push('\n');
    }
    out.push_str("-----END PGP SIGNATURE-----\n");
    Some(out)
}

/// 公开增量同步：必须由账户私钥对 `sync:{uid}:{type}:{since}:{ts}` 签名。
/// 返回数据按 (uid, type) 过滤：只含该账户的自身快照与相关交易。
async fn sync_public(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SyncQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let since = q.since.unwrap_or(0);
    let uid = header_str(&headers, "x-acs-uid")
        .ok_or_else(|| ApiErr::unauthorized("需要客户端签名（缺少 X-ACS-Uid）"))?;
    let type_s = header_str(&headers, "x-acs-type")
        .ok_or_else(|| ApiErr::unauthorized("需要客户端签名（缺少 X-ACS-Type）"))?;
    let ts: i64 = header_str(&headers, "x-acs-ts")
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| ApiErr::unauthorized("需要客户端签名（缺少 X-ACS-Ts）"))?;
    let sig = header_str(&headers, "x-acs-sig")
        .ok_or_else(|| ApiErr::unauthorized("需要客户端签名（缺少 X-ACS-Sig）"))?;
    let sig = rearmor_signature(&sig)
        .ok_or_else(|| ApiErr::unauthorized("同步签名格式无效"))?;
    if (Utc::now().timestamp() - ts).abs() > 300 {
        return Err(ApiErr::unauthorized("签名时间戳超出允许范围"));
    }
    let atype = AccountType::from_str(&type_s).ok_or_else(|| ApiErr::bad_request("无效账户类型"))?;
    let msg = format!("sync:{uid}:{}:{since}:{ts}", atype.as_str());
    let pubk = {
        let conn = state::db_lock(&st.db);
        account_pubkey(&conn, &uid, atype).ok_or_else(|| {
            ApiErr::unauthorized("需要有效的账户签名（账户不存在或无公钥）")
        })?
    };
    if !st
        .gpg
        .verify_detached(&pubk, msg.as_bytes(), &sig)
        .map_err(ApiErr::from)?
    {
        return Err(ApiErr::unauthorized("同步签名校验失败"));
    }
    let conn = state::db_lock(&st.db);
    match acs_core::account::recompute_all_if_dirty(&conn) {
        Ok(true) => acs_core::log::info("同步前完成余额纠偏体检（账本有新增交易）"),
        Ok(false) => {}
        Err(e) => acs_core::log::err(format!("余额纠偏体检失败（已跳过，不影响同步）：{e}")),
    }
    let (snapshot, hash) = build_snapshot_filtered(&conn, since, Some((&uid, atype)))?;
    let central_sig = crate::api::keys::try_sign_hash(&st, &hash);
    Ok(Json(json!({ "ok": true, "hash": hash, "central_sig": central_sig, "data": snapshot })))
}

/// 管理后台：全量快照（需 Bearer token）。供运维排查与回归测试。
async fn sync_admin(
    State(st): State<AppState>,
    _auth: AuthUser,
    Query(q): Query<SyncQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let since = q.since.unwrap_or(0);
    let conn = state::db_lock(&st.db);
    let (snapshot, hash) = build_snapshot(&conn, since)?;
    let central_sig = crate::api::keys::try_sign_hash(&st, &hash);
    Ok(Json(json!({ "ok": true, "hash": hash, "central_sig": central_sig, "data": snapshot })))
}

fn account_pubkey(conn: &Connection, uid: &str, atype: AccountType) -> Option<String> {
    let sql = format!("SELECT pubkey FROM {} WHERE uid=?1", atype.table_name());
    conn.query_row(&sql, params![uid], |r| r.get::<_, String>(0))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// 构建增量账本快照（Confirmed 交易 + 各表账户 + sha256）。全量（管理端）。
pub fn build_snapshot(conn: &Connection, since: i64) -> Result<(serde_json::Value, String), ApiErr> {
    build_snapshot_filtered(conn, since, None)
}

/// 按 `owner=(uid, type)` 过滤的快照：
/// - 交易：只保留该账户作为发送方或接收方（且类型匹配）的交易；
/// - 账户：只保留该账户自身一行（客户端同步只需自身链头与余额）。
pub fn build_snapshot_filtered(
    conn: &Connection,
    since: i64,
    owner: Option<(&str, AccountType)>,
) -> Result<(serde_json::Value, String), ApiErr> {
    let mut txs = Vec::new();
    match owner {
        Some((uid, atype)) => {
            let mut tstmt = conn
                .prepare(
                    "SELECT tx_id, tx_type, sender, sender_type, receiver, receiver_type, amount, ts, tx_hash, central_sig, status \
                     FROM transactions WHERE status='Confirmed' AND ts>?1 \
                     AND ((sender=?2 AND sender_type=?3) OR (receiver=?2 AND receiver_type=?3)) \
                     ORDER BY ts ASC",
                )
                .map_err(ApiErr::from_err)?;
            let rows = tstmt
                .query_map(params![since, uid, atype.as_str()], tx_row)
                .map_err(ApiErr::from_err)?;
            for r in rows {
                txs.push(r.map_err(ApiErr::from_err)?);
            }
        }
        None => {
            let mut tstmt = conn
                .prepare(
                    "SELECT tx_id, tx_type, sender, sender_type, receiver, receiver_type, amount, ts, tx_hash, central_sig, status \
                     FROM transactions WHERE status='Confirmed' AND ts>?1 ORDER BY ts ASC",
                )
                .map_err(ApiErr::from_err)?;
            let rows = tstmt
                .query_map(params![since], tx_row)
                .map_err(ApiErr::from_err)?;
            for r in rows {
                txs.push(r.map_err(ApiErr::from_err)?);
            }
        }
    }

    let mut accounts = Vec::new();
    match owner {
        Some((uid, atype)) => {
            let table = atype.table_name();
            let mut astmt = conn
                .prepare(&format!(
                    "SELECT uid, fullname, balance, status, last_tx_hash, changed_at FROM {table} WHERE uid=?1"
                ))
                .map_err(ApiErr::from_err)?;
            let rows = astmt
                .query_map(params![uid], |r| account_row(r, atype))
                .map_err(ApiErr::from_err)?;
            for r in rows {
                accounts.push(r.map_err(ApiErr::from_err)?);
            }
        }
        None => {
            for at in [
                AccountType::Country,
                AccountType::Company,
                AccountType::Individual,
                AccountType::System,
            ] {
                let table = at.table_name();
                let mut astmt = conn
                    .prepare(&format!(
                        "SELECT uid, fullname, balance, status, last_tx_hash, changed_at FROM {table}"
                    ))
                    .map_err(ApiErr::from_err)?;
                let rows = astmt
                    .query_map([], |r| account_row(r, at))
                    .map_err(ApiErr::from_err)?;
                for r in rows {
                    accounts.push(r.map_err(ApiErr::from_err)?);
                }
            }
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

fn tx_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<serde_json::Value> {
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
}

fn account_row(r: &rusqlite::Row<'_>, at: AccountType) -> rusqlite::Result<serde_json::Value> {
    Ok(json!({
        "uid": r.get::<_, String>(0)?,
        "type": at.as_str(),
        "fullname": r.get::<_, String>(1)?,
        "balance": r.get::<_, i64>(2)?,
        "status": r.get::<_, String>(3)?,
        "last_tx_hash": r.get::<_, Option<String>>(4)?,
        "changed_at": r.get::<_, i64>(5)?,
    }))
}
