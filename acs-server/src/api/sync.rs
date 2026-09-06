//! 公开同步（v3.0.0 取消镜像）：中心作为唯一记账权威，提供增量同步与健康探测。
//! - `/api/status`：健康/延迟探测
//! - `/api/sync`：client 免 apikey 增量同步（Confirmed 交易 + 全部账户快照 + sha256 哈希 + 可选中心签名）

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use acs_core::models::AccountType;

use crate::api::{ApiErr, ApiResult};
use crate::state::AppState;

/// 公开：状态 / 增量同步（client 免 apikey）。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/sync", get(sync_public))
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

/// 构建增量账本快照（Confirmed 交易 + 各表账户 + sha256）。
pub fn build_snapshot(conn: &Connection, since: i64) -> Result<(serde_json::Value, String), ApiErr> {
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
