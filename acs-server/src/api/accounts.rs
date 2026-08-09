//! 账本账户管理端点（网页不开放注册；仅状态管理）。
//!
//! 权限：root 管理所有类型；finance 仅 Bank。
//! 转账（Transfer/Issue/Redeem）只能由 client 发起，网页不提供。

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use acs_core::account;
use acs_core::models::{AccountStatus, AccountType};
use acs_core::transaction;

use crate::api::audit::log_audit;
use crate::api::{ApiErr, ApiResult};
use crate::auth::AuthUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/accounts", get(list_accounts))
        .route(
            "/api/accounts/{atype}/{uid}",
            get(get_account).delete(delete_account),
        )
        .route("/api/accounts/{atype}/{uid}/freeze", post(freeze))
        .route("/api/accounts/{atype}/{uid}/unfreeze", post(unfreeze))
        .route("/api/admin/credit", post(credit))
}

fn can_manage(auth: &AuthUser, atype: AccountType) -> bool {
    auth.is_root() || (auth.role == acs_core::models::AdminRole::Finance && atype == AccountType::Bank)
}

#[derive(Deserialize)]
pub struct ListQuery {
    atype: String,
    search: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_accounts(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(&q.atype).ok_or_else(|| ApiErr::bad_request("未知账户类型"))?;
    if !can_manage(&auth, atype) {
        return Err(ApiErr::forbidden("无权管理该账户类型"));
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let table = atype.table_name();

    let conn = st.db.lock().unwrap();
    let mut sql = format!(
        "SELECT uid, email, balance, status, last_tx_hash, created_at, changed_at FROM {table}"
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(s) = q.search.filter(|s| !s.trim().is_empty()) {
        sql.push_str(" WHERE uid LIKE ?1");
        params.push(Box::new(format!("%{}%", s.trim())));
    }
    sql.push_str(" ORDER BY changed_at DESC LIMIT ? OFFSET ?");
    params.push(Box::new(limit));
    params.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql).map_err(ApiErr::from_err)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())), |r| {
            Ok(json!({
                "uid": r.get::<_, String>(0)?,
                "email": r.get::<_, String>(1)?,
                "balance": r.get::<_, i64>(2)?,
                "status": r.get::<_, String>(3)?,
                "last_tx_hash": r.get::<_, Option<String>>(4)?,
                "created_at": r.get::<_, i64>(5)?,
                "changed_at": r.get::<_, i64>(6)?,
            }))
        })
        .map_err(ApiErr::from_err)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(ApiErr::from_err)?);
    }
    Ok(Json(json!({ "atype": atype.as_str(), "count": items.len(), "items": items })))
}

async fn get_account(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((atype_s, uid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(&atype_s).ok_or_else(|| ApiErr::bad_request("未知账户类型"))?;
    if !can_manage(&auth, atype) {
        return Err(ApiErr::forbidden("无权管理该账户类型"));
    }
    let conn = st.db.lock().unwrap();
    let acc = account::require_account(&conn, &uid, atype).map_err(ApiErr::from)?;
    let txs = transaction::list_transactions_for(&conn, &uid, atype).map_err(ApiErr::from)?;
    Ok(Json(json!({
        "uid": acc.uid,
        "atype": atype.as_str(),
        "email": acc.email,
        "pubkey": acc.pubkey,
        "balance": acc.balance,
        "status": acc.status.as_str(),
        "last_tx_hash": acc.last_tx_hash,
        "created_at": acc.created_at.timestamp(),
        "changed_at": acc.changed_at.timestamp(),
        "transactions": txs.iter().map(|t| json!({
            "tx_id": t.tx_id, "tx_type": t.tx_type.as_str(),
            "sender": t.sender, "sender_type": t.sender_type.as_str(),
            "receiver": t.receiver, "receiver_type": t.receiver_type.as_str(),
            "amount": t.amount, "timestamp": t.timestamp, "status": t.status.as_str(),
            "central_signed": t.central_sig.is_some(),
        })).collect::<Vec<_>>(),
    })))
}

async fn freeze(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((atype_s, uid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    set_status(&st, &auth, &atype_s, &uid, AccountStatus::Frozen, "冻结账户").await
}

async fn unfreeze(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((atype_s, uid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    set_status(&st, &auth, &atype_s, &uid, AccountStatus::Active, "解冻账户").await
}

async fn set_status(
    st: &AppState,
    auth: &AuthUser,
    atype_s: &str,
    uid: &str,
    status: AccountStatus,
    op: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let atype = AccountType::from_str(atype_s).ok_or_else(|| ApiErr::bad_request("未知账户类型"))?;
    if !can_manage(auth, atype) {
        return Err(ApiErr::forbidden("无权管理该账户类型"));
    }
    let conn = st.db.lock().unwrap();
    account::set_status(&conn, uid, atype, status).map_err(ApiErr::from)?;
    log_audit(&conn, &auth.username, "set_status", &format!("{op}: {} {}", atype.as_str(), uid));
    Ok(Json(json!({ "ok": true, "uid": uid, "status": status.as_str() })))
}

async fn delete_account(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((atype_s, uid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可删除账户"));
    }
    let atype = AccountType::from_str(&atype_s).ok_or_else(|| ApiErr::bad_request("未知账户类型"))?;
    let conn = st.db.lock().unwrap();
    let table = atype.table_name();
    let n = conn
        .execute(&format!("DELETE FROM {table} WHERE uid=?1"), params![uid])
        .map_err(ApiErr::from_err)?;
    if n == 0 {
        return Err(ApiErr::not_found("账户不存在"));
    }
    log_audit(&conn, &auth.username, "delete_account", &format!("{} {}", atype.as_str(), uid));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CreditReq {
    pub uid: String,
    #[serde(rename = "type")]
    pub atype: String,
    pub amount: i64,
}

/// 充值/调整余额（仅 root；供管理调节与测试）。
/// 说明：直接调整账户余额并记审计，不产生交易（账本对账时注意）。
async fn credit(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreditReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可充值"));
    }
    let atype = AccountType::from_str(&req.atype).ok_or_else(|| ApiErr::bad_request("未知账户类型"))?;
    if req.amount == 0 {
        return Err(ApiErr::bad_request("金额不能为 0"));
    }
    let conn = st.db.lock().unwrap();
    let acc = account::require_account(&conn, &req.uid, atype)?;
    let new_balance = acc.balance + req.amount;
    if new_balance < 0 {
        return Err(ApiErr::bad_request("余额不足，无法扣减"));
    }
    account::update_balance_and_hash(
        &conn,
        &req.uid,
        atype,
        new_balance,
        acc.last_tx_hash.as_deref(),
    )?;
    log_audit(
        &conn,
        &auth.username,
        "credit",
        &format!("{} {}{}", atype.as_str(), req.uid, if req.amount >= 0 { format!("+{}", req.amount) } else { req.amount.to_string() }),
    );
    Ok(Json(json!({ "ok": true, "uid": req.uid, "balance": new_balance })))
}

