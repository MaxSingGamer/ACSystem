//! 账户数据访问（按账户类型路由到不同中心表；UID 唯一识别，无 abbr）。

use std::collections::HashMap;

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

/// 修改账户状态（冻结/解冻/关闭/注销）。
///
/// **终态保护**：`Deleted`（注销）是终态，不可再迁出。
/// 注销意味着账户与账本只读保留供审计（法律文档已作承诺），
/// 若允许把 Deleted 改回 Active，等于注销可被撤销、承诺失效。
/// `Deleted → Deleted` 视为幂等（重复注销不报错）。
pub fn set_status(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
    status: AccountStatus,
) -> Result<()> {
    let cur = get_account(conn, uid, atype)?
        .ok_or_else(|| AcsError::AccountNotFound(uid.to_string()))?;
    if cur.status == AccountStatus::Deleted && status != AccountStatus::Deleted {
        return Err(AcsError::Message(format!(
            "账户 {uid} 已注销（Deleted，终态），不可再变更为 {}",
            status.as_str()
        )));
    }
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

/// 仅更新账本链头（余额统一交给 `recompute_account` / `recompute_all_balances`）。
/// 语义（与余额口径配套）：交易一提交（Pending）就推进**发送方**链头，
/// 接收方链头在确认时推进；被拒收/置错时发送方链头回退到 `sender_last_hash`。
pub fn set_last_hash(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
    last_tx_hash: Option<&str>,
) -> Result<()> {
    let table = atype.table_name();
    let sql = format!("UPDATE {table} SET last_tx_hash=?1, changed_at=?2 WHERE uid=?3");
    conn.execute(&sql, params![last_tx_hash, Utc::now().timestamp(), uid])?;
    Ok(())
}

/// 全量重算并回写所有账户余额：balance = Σ(计入的收款) − Σ(计入的支出)。
///
/// **计入口径（v3.1.0 起）**：`Pending` 与 `Confirmed` 均计入，`Rejected` / `Error` 不计入。
/// 即：转出一提交（Pending）就立即扣减发送方余额（防止同一笔钱被重复花出），
/// 收款方在待确认阶段即计入余额（若对方拒收，则该笔交易状态转为 Rejected，双方余额自动回退）。
///
/// 收支口径**必须与 `transaction` 里的结算/入账逻辑逐条对应，不可随意增删**：
/// - 收入：`Mint` / `Issue` / `Transfer` 的收款方
/// - 支出：`Redeem` / `Transfer` 的付款方
/// 之所以不对称：`Issue`（发行）是「商品篮子 → A€」的**增发**，`Redeem`（赎回）是
/// 「A€ → 商品篮子」的**销毁**；发行账户（PreIssuedAccount）只是记账对手方，
/// 余额不随二者变动。
///
/// 性能：用两条 `GROUP BY` 汇总替代原先「逐账户两条 SUM」，
/// 复杂度由 O(账户数 × 交易数) 降为 O(交易数 + 账户数)（走 idx_tx_sender/idx_tx_receiver）。
pub fn recompute_all_balances(conn: &Connection) -> Result<()> {
    let inc = group_sums(
        conn,
        "SELECT receiver, receiver_type, COALESCE(SUM(amount),0) FROM transactions \
         WHERE status IN ('Pending','Confirmed') AND tx_type IN ('Mint','Issue','Transfer') \
         GROUP BY receiver, receiver_type",
    )?;
    let out = group_sums(
        conn,
        "SELECT sender, sender_type, COALESCE(SUM(amount),0) FROM transactions \
         WHERE status IN ('Pending','Confirmed') AND tx_type IN ('Redeem','Transfer') \
         GROUP BY sender, sender_type",
    )?;
    for at in [
        AccountType::Country,
        AccountType::Company,
        AccountType::Individual,
        AccountType::System,
    ] {
        let table = at.table_name();
        let st = at.as_str();
        let mut stmt = conn.prepare(&format!("SELECT uid FROM {table}"))?;
        let uids: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        for uid in uids {
            let bal = settle_balance(&inc, &out, &uid, st);
            conn.execute(
                &format!("UPDATE {table} SET balance=?1 WHERE uid=?2"),
                params![bal, uid],
            )?;
        }
    }
    Ok(())
}

/// 重算并回写**单个账户**余额并返回结果（提交交易前刷新发送方余额用）。
/// 只做两条走索引的 SUM，不触发全库扫描。
/// 计入口径与 `recompute_all_balances` 一致：`Pending` + `Confirmed` 计入，`Rejected`/`Error` 不计入。
pub fn recompute_account(conn: &Connection, uid: &str, atype: AccountType) -> Result<i64> {
    let table = atype.table_name();
    let st = atype.as_str();
    let inc: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions \
             WHERE receiver=?1 AND receiver_type=?2 AND status IN ('Pending','Confirmed') \
               AND tx_type IN ('Mint','Issue','Transfer')",
            params![uid, st],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let out: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions \
             WHERE sender=?1 AND sender_type=?2 AND status IN ('Pending','Confirmed') \
               AND tx_type IN ('Redeem','Transfer')",
            params![uid, st],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mut m = HashMap::new();
    if inc != 0 {
        m.insert((uid.to_string(), st.to_string()), inc);
    }
    let mut m2 = HashMap::new();
    if out != 0 {
        m2.insert((uid.to_string(), st.to_string()), out);
    }
    let bal = settle_balance(&m, &m2, uid, st);
    conn.execute(
        &format!("UPDATE {table} SET balance=?1 WHERE uid=?2"),
        params![bal, uid],
    )?;
    Ok(bal)
}

/// 原始净额（收入 − 支出，**不把负值夹取为 0**）：用于「可支付性」判定。
/// 口径与 `recompute_account` 完全一致（Pending + Confirmed 计入，Rejected/Error 不计入），
/// 仅不做负值兜底 —— 否则透支会被看成「余额 0」而无法识别。
pub fn raw_balance(conn: &Connection, uid: &str, atype: AccountType) -> Result<i64> {
    let st = atype.as_str();
    let inc: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions \
             WHERE receiver=?1 AND receiver_type=?2 AND status IN ('Pending','Confirmed') \
               AND tx_type IN ('Mint','Issue','Transfer')",
            params![uid, st],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let out: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions \
             WHERE sender=?1 AND sender_type=?2 AND status IN ('Pending','Confirmed') \
               AND tx_type IN ('Redeem','Transfer')",
            params![uid, st],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(inc - out)
}

/// 按 (uid, 类型) 取汇总值。
fn group_sums(conn: &Connection, sql: &str) -> Result<HashMap<(String, String), i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut m = HashMap::new();
    for r in rows {
        let (k, v) = r?;
        m.insert(k, v);
    }
    Ok(m)
}

/// 收入 − 支出；负数只可能来自账目异常，按 0 处理并写日志（不再静默掩盖）。
fn settle_balance(
    inc: &HashMap<(String, String), i64>,
    out: &HashMap<(String, String), i64>,
    uid: &str,
    atype_str: &str,
) -> i64 {
    let key = (uid.to_string(), atype_str.to_string());
    let i = inc.get(&key).copied().unwrap_or(0);
    let o = out.get(&key).copied().unwrap_or(0);
    let diff = i - o;
    if diff < 0 {
        crate::log::err(format!(
            "余额重算异常：{atype_str} 账户 {uid} 支出 {o} 大于收入 {i}（差额 {diff}），已按 0 写入"
        ));
    }
    diff.max(0)
}
