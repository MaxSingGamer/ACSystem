//! 只读 HTTP 服务：对外提供镜像账本快照查询（仅 GET，无任何写操作）。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::store::Store;

#[derive(Clone)]
pub struct HttpState {
    pub store: std::sync::Arc<std::sync::Mutex<Store>>,
}

pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/accounts", get(accounts))
        .route("/api/account/{uid}", get(account))
        .route("/api/txs", get(txs))
        .route("/api/tx/{tx_id}", get(tx))
        .route("/", get(root))
        .with_state(state)
}

async fn root() -> &'static str {
    "A€ Alpha Coin · 只读镜像\nGET /api/status · /api/accounts · /api/account/{uid} · /api/txs · /api/tx/{tx_id}"
}

async fn status(State(st): State<HttpState>) -> Json<Value> {
    let s = st.store.lock().unwrap();
    Json(json!({
        "ok": true,
        "last_sync": s.info.last_sync,
        "last_hash": s.info.last_hash,
        "accounts": s.account_count(),
        "txs": s.tx_count(),
        "server": s.info.server_url,
    }))
}

async fn accounts(State(st): State<HttpState>) -> Json<Value> {
    let s = st.store.lock().unwrap();
    let mut stmt = s
        .conn
        .prepare("SELECT uid, type, balance, status, last_tx_hash, changed_at FROM mirror_accounts ORDER BY uid")
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "uid": r.get::<_, String>(0)?,
                "type": r.get::<_, String>(1)?,
                "balance": r.get::<_, i64>(2)?,
                "status": r.get::<_, String>(3)?,
                "last_tx_hash": r.get::<_, Option<String>>(4)?,
                "changed_at": r.get::<_, i64>(5)?,
            }))
        })
        .unwrap();
    let items: Vec<Value> = rows.flatten().collect();
    Json(json!({ "accounts": items, "count": items.len() }))
}

async fn account(State(st): State<HttpState>, Path(uid): Path<String>) -> impl IntoResponse {
    let s = st.store.lock().unwrap();
    match s.account(&uid) {
        Some((uid, atype, balance, status, last_tx_hash)) => (
            StatusCode::OK,
            Json(json!({
                "uid": uid, "type": atype, "balance": balance,
                "status": status, "last_tx_hash": last_tx_hash,
            })),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "账户不存在" }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct TxsQuery {
    pub uid: Option<String>,
    pub limit: Option<i64>,
}

async fn txs(State(st): State<HttpState>, Query(q): Query<TxsQuery>) -> Json<Value> {
    let s = st.store.lock().unwrap();
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let mut sql = String::from(
        "SELECT tx_id, tx_type, peer, peer_type, amount, ts, tx_hash, central_sig, status FROM local_ledger",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(uid) = q.uid.filter(|u| !u.is_empty()) {
        sql.push_str(" WHERE peer=?1");
        params.push(Box::new(uid));
    }
    sql.push_str(" ORDER BY ts DESC LIMIT ?");
    params.push(Box::new(limit));
    let mut stmt = s.conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())), |r| {
            Ok(json!({
                "tx_id": r.get::<_, String>(0)?,
                "tx_type": r.get::<_, String>(1)?,
                "peer": r.get::<_, String>(2)?,
                "peer_type": r.get::<_, String>(3)?,
                "amount": r.get::<_, i64>(4)?,
                "timestamp": r.get::<_, i64>(5)?,
                "tx_hash": r.get::<_, String>(6)?,
                "central_sig": r.get::<_, Option<String>>(7)?,
                "status": r.get::<_, String>(8)?,
            }))
        })
        .unwrap();
    let items: Vec<Value> = rows.flatten().collect();
    Json(json!({ "txs": items, "count": items.len() }))
}

async fn tx(State(st): State<HttpState>, Path(tx_id): Path<String>) -> impl IntoResponse {
    let s = st.store.lock().unwrap();
    let r = s
        .conn
        .query_row(
            "SELECT tx_id, tx_type, peer, peer_type, amount, ts, tx_hash, central_sig, status FROM local_ledger WHERE tx_id=?1",
            params![tx_id],
            |r| {
                Ok(json!({
                    "tx_id": r.get::<_, String>(0)?,
                    "tx_type": r.get::<_, String>(1)?,
                    "peer": r.get::<_, String>(2)?,
                    "peer_type": r.get::<_, String>(3)?,
                    "amount": r.get::<_, i64>(4)?,
                    "timestamp": r.get::<_, i64>(5)?,
                    "tx_hash": r.get::<_, String>(6)?,
                    "central_sig": r.get::<_, Option<String>>(7)?,
                    "status": r.get::<_, String>(8)?,
                }))
            },
        )
        .ok();
    match r {
        Some(v) => (StatusCode::OK, Json(v)).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "交易不存在" }))).into_response(),
    }
}
