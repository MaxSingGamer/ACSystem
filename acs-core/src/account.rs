//! 账户数据访问（按账户类型路由到不同中心表；UID 唯一识别，无 abbr）。

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};

use crate::errors::{AcsError, Result};
use crate::models::{Account, AccountStatus, AccountType};

const ACCOUNT_COLS: &str =
    "uid, email, pubkey, encrypted_seckey, balance, status, last_tx_hash, created_at, changed_at";

fn map_account(row: &Row, atype: AccountType) -> rusqlite::Result<Account> {
    let status: String = row.get("status")?;
    let created: i64 = row.get("created_at")?;
    let changed: i64 = row.get("changed_at")?;
    Ok(Account {
        uid: row.get("uid")?,
        account_type: atype,
        email: row.get("email")?,
        pubkey: row.get("pubkey")?,
        encrypted_seckey: row.get("encrypted_seckey")?,
        balance: row.get("balance")?,
        status: AccountStatus::from_str(&status).unwrap_or(AccountStatus::Active),
        last_tx_hash: row.get("last_tx_hash")?,
        created_at: ts_to_dt(created),
        changed_at: ts_to_dt(changed),
    })
}

fn ts_to_dt(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now)
}

/// 新建账户（写入对应类型表）。
pub fn create_account(conn: &Connection, acc: &Account) -> Result<()> {
    let table = acc.account_type.table_name();
    let sql = format!("INSERT INTO {table}({ACCOUNT_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)");
    conn.execute(
        &sql,
        params![
            acc.uid,
            acc.email,
            acc.pubkey,
            acc.encrypted_seckey,
            acc.balance,
            acc.status.as_str(),
            acc.last_tx_hash,
            acc.created_at.timestamp(),
            acc.changed_at.timestamp(),
        ],
    )?;
    Ok(())
}

/// 按 uid + 类型查询账户。
pub fn get_account(conn: &Connection, uid: &str, atype: AccountType) -> Result<Option<Account>> {
    let table = atype.table_name();
    let sql = format!("SELECT {ACCOUNT_COLS} FROM {table} WHERE uid=?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![uid], |r| map_account(r, atype))?;
    Ok(rows.next().transpose()?)
}

/// 查询账户，不存在则报错。
pub fn require_account(conn: &Connection, uid: &str, atype: AccountType) -> Result<Account> {
    get_account(conn, uid, atype)?.ok_or_else(|| AcsError::AccountNotFound(uid.to_string()))
}

/// 账户是否存在。
pub fn account_exists(conn: &Connection, uid: &str, atype: AccountType) -> Result<bool> {
    Ok(get_account(conn, uid, atype)?.is_some())
}

/// 修改账户状态（冻结/解冻/关闭）。
pub fn set_status(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
    status: AccountStatus,
) -> Result<()> {
    let table = atype.table_name();
    let sql = format!("UPDATE {table} SET status=?1, changed_at=?2 WHERE uid=?3");
    conn.execute(&sql, params![status.as_str(), Utc::now().timestamp(), uid])?;
    Ok(())
}

/// 更新余额与账本链头哈希（结算时使用）。
pub fn update_balance_and_hash(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
    balance: i64,
    last_tx_hash: Option<&str>,
) -> Result<()> {
    let table = atype.table_name();
    let sql = format!(
        "UPDATE {table} SET balance=?1, last_tx_hash=?2, changed_at=?3 WHERE uid=?4"
    );
    conn.execute(&sql, params![balance, last_tx_hash, Utc::now().timestamp(), uid])?;
    Ok(())
}
