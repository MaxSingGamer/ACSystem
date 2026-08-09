//! 统计端点：总览 + 每日流水折线图（近周/月/年）。账单需二次鉴权（审计密码）。

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use acs_core::models::AccountType;

use crate::api::audit::require_audit;
use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/stats/overview", get(overview))
        .route("/api/stats/daily", get(daily))
        .route("/api/stats/bills", get(bills))
}

fn day_start_utc() -> i64 {
    chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

async fn overview(
    State(st): State<AppState>,
    _auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = st.db.lock().unwrap();
    let today = day_start_utc();

    let today_flow: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions WHERE status='Confirmed' AND ts>=?1",
            params![today],
            |r| r.get(0),
        )
        .map_err(ApiErr::from_err)?;
    let today_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transactions WHERE status='Confirmed' AND ts>=?1",
            params![today],
            |r| r.get(0),
        )
        .map_err(ApiErr::from_err)?;
    let total_flow: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions WHERE status='Confirmed'",
            [],
            |r| r.get(0),
        )
        .map_err(ApiErr::from_err)?;
    let total_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0))
        .map_err(ApiErr::from_err)?;

    let mut accounts = serde_json::Map::new();
    for at in [AccountType::Country, AccountType::Bank, AccountType::Individual, AccountType::System] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {}", at.table_name()), [], |r| r.get(0))
            .map_err(ApiErr::from_err)?;
        accounts.insert(at.as_str().to_string(), json!(n));
    }
    Ok(Json(json!({
        "today_flow": today_flow, "today_count": today_count,
        "total_flow": total_flow, "total_count": total_count,
        "accounts": accounts, "as_of": chrono::Utc::now().timestamp(),
    })))
}

/// 每日累计流水（折线图）：days=7 近一周 / 30 近一月 / 365 近一年。
async fn daily(
    State(st): State<AppState>,
    _auth: AuthUser,
    Query(q): Query<DailyQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let days = q.days.unwrap_or(7).clamp(1, 365);
    let start = day_start_utc() - (days - 1) * 86400;
    let conn = st.db.lock().unwrap();

    // 每日总量（Confirmed）
    let mut stmt = conn
        .prepare(
            "SELECT date(ts,'unixepoch') d, COALESCE(SUM(amount),0) \
             FROM transactions WHERE status='Confirmed' AND ts>=?1 GROUP BY d ORDER BY d",
        )
        .map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map(params![start], |r| {
            Ok(json!({
                "date": r.get::<_, String>(0)?,
                "flow": r.get::<_, i64>(1)?,
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(ApiErr::from_err)?);
    }
    Ok(Json(json!({ "days": days, "items": items })))
}

#[derive(Deserialize)]
pub struct DailyQuery {
    pub days: Option<i64>,
}

#[derive(Deserialize)]
pub struct BillsQuery {
    pub date: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

/// 交易总账单（需先通过审计二次鉴权）。
async fn bills(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(q): Query<BillsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_audit(&st, &auth)?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(100).clamp(1, 500);
    let offset = (page - 1) * page_size;

    let conn = st.db.lock().unwrap();
    let mut sql = String::from(
        "SELECT tx_id, tx_type, sender, sender_type, receiver, receiver_type, amount, ts, status, central_sig \
         FROM transactions WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(d) = q.date.filter(|d| !d.is_empty()) {
        sql.push_str(" AND date(ts,'unixepoch')=?");
        params.push(Box::new(d));
    }
    sql.push_str(" ORDER BY ts DESC LIMIT ? OFFSET ?");
    params.push(Box::new(page_size));
    params.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql).map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())), |r| {
            Ok(json!({
                "tx_id": r.get::<_, String>(0)?,
                "tx_type": r.get::<_, String>(1)?,
                "sender": r.get::<_, String>(2)?,
                "sender_type": r.get::<_, String>(3)?,
                "receiver": r.get::<_, String>(4)?,
                "receiver_type": r.get::<_, String>(5)?,
                "amount": r.get::<_, i64>(6)?,
                "timestamp": r.get::<_, i64>(7)?,
                "status": r.get::<_, String>(8)?,
                "central_signed": r.get::<_, Option<String>>(9)?.is_some(),
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(ApiErr::from_err)?);
    }
    Ok(Json(json!({ "page": page, "page_size": page_size, "items": items })))
}
