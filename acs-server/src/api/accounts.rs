//! 账本账户管理端点（网页不开放注册；仅状态管理）。
//!
//! 权限：root 管理所有类型；finance 仅 Company。
//! 转账（Transfer/Issue/Redeem）只能由 client 发起，网页不提供。
//!
//! 设计不变量：**任何余额变动都必须来自账本交易**。
//! 原 `/api/admin/credit`（root 直接改余额、不产生交易）已移除：
//! 每次 `/api/sync` 都会按账本重算余额，这类直接写的余额会被静默抹掉，属于资金丢失隐患。
//! 如需管理员调账，应新增一种专用交易类型（带 central_sig 签名）而不是直接改 balance。

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use acs_core::account;
use acs_core::models::{AccountStatus, AccountType};
use acs_core::transaction;

use crate::api::audit::with_audit;
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
}

fn can_manage(auth: &AuthUser, atype: AccountType) -> bool {
    auth.is_root() || (auth.role == acs_core::models::AdminRole::Finance && atype == AccountType::Company)
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
    let mut conn = st.db.lock().unwrap();
    // 业务写入与审计写入同一事务：审计失败则状态变更一并回滚
    with_audit(
        &mut conn,
        &auth.username,
        "set_status",
        &format!("{op}: {} {uid}", atype.as_str()),
        |c| {
            account::set_status(c, uid, atype, status).map_err(ApiErr::from)?;
            Ok(())
        },
    )?;
    Ok(Json(json!({ "ok": true, "uid": uid, "status": status.as_str() })))
}

async fn delete_account(
    State(st): State<AppState>,
    auth: AuthUser,
    Path((atype_s, uid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth.is_root() {
        return Err(ApiErr::forbidden("仅根管理员可注销账户"));
    }
    let atype = AccountType::from_str(&atype_s).ok_or_else(|| ApiErr::bad_request("未知账户类型"))?;
    let mut conn = st.db.lock().unwrap();
    // 软删除：状态改为 Deleted（账户信息与账本只读保留，供审计），而非物理删除；
    // 同时删除口令加密的私钥密文（云端仅保留公钥）
    with_audit(
        &mut conn,
        &auth.username,
        "delete_account",
        &format!("注销账户: {} {uid}", atype.as_str()),
        |c| {
            account::set_status(c, &uid, atype, AccountStatus::Deleted).map_err(ApiErr::from)?;
            account::purge_secret(c, &uid, atype).map_err(ApiErr::from)?;
            Ok(())
        },
    )?;
    Ok(Json(json!({ "ok": true, "uid": uid, "status": "Deleted" })))
}

// 备注：原 `/api/admin/credit`（root 直接改余额）已**整体移除**，不再保留代码。
// 原因：它 `UPDATE ... SET balance=...` 而不产生交易，而 `account::recompute_all_balances`
// 只按账本（Confirmed 交易）推导余额，且该重算会在每次 `/api/sync` 触发 ——
// 管理员充值会在客户端下一次刷新时被静默抹掉（资金丢失隐患）。
// 若日后确需管理员调账，应新增一种**带 central_sig 签名的专用交易类型**，
// 让调账同样进入账本与余额重算口径，而不是直接写 balance。

