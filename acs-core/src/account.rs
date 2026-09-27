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
/// 语义（与余额口径配套，v3.2.1）：
/// - 交易一提交（Pending）就推进**发送方**链头（发送方事务严格串行，防重放/双花）；
/// - **接收方链头在确认时才推进**：待收（Pending 收款）不占接收方链头，否则接收方
///   在待收期间自己的转账会把链头顶走，导致点「接受」时判「链头不一致」而永远卡在 Pending；
/// - 被拒收/置错时发送方链头回退到 `sender_last_hash`。
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

/// 接收方确认收款时要写入的链头。
///
/// 待收（Pending 收款）**不占**接收方链头，因此确认时的链头推进必须容忍「链头已被
/// 接收方自己的转账（或另一笔先被接受的收款）顶走」这一正常情形：
/// - 链头仍等于本笔交易快照的 `receiver_last_hash` → 推进为本笔 `tx_hash`（常规路径）；
/// - 链头已是本笔 `tx_hash`（拒收回退路径）→ 保持不变（幂等）；
/// - 链头已被别的交易顶走 → 返回 `None`（**保持现状，绝不回退别人的链头**）。
///
/// 历史 bug 现场：这里以前硬性要求「链头 == receiver_last_hash」，于是接收方一旦在
/// 待收期间自己转过账，点「接受」就永久报「接收方链头不一致」，表现为
/// 「交易状态持续 Pending + 待收箱点了接受仍不清理」。
pub fn advance_head_for_receiver(
    stored: Option<&str>,
    receiver_last_hash: Option<&str>,
    new_head: Option<&str>,
) -> Option<String> {
    if stored == receiver_last_hash || stored == new_head {
        new_head.map(|s| s.to_string())
    } else {
        None
    }
}

/// 注销后的云端清理：**删除口令加密的私钥密文，只保留公钥**。
/// 账本与公钥保留（审计需要），但中心不再持有可解开的私钥材料。
/// 系统账户不在此列：其密钥材料属服务端自有资产（代管签名用）。
pub fn purge_secret(conn: &Connection, uid: &str, atype: AccountType) -> Result<()> {
    if atype == AccountType::System {
        return Ok(());
    }
    let table = atype.table_name();
    conn.execute(
        &format!("UPDATE {table} SET encrypted_seckey='', changed_at=?1 WHERE uid=?2"),
        params![Utc::now().timestamp(), uid],
    )?;
    Ok(())
}

/// 交易前置校验：只有 `Active` 账户可以参与交易。
///
/// `who` 为可读主语（"发送方" / "接收方"），用于给出「谁」不可交易的提示。
/// 注销 / 冻结 / 关闭一律拒绝，并带上具体原因（对外映射为 403）。
pub fn ensure_tradable(status: AccountStatus, who: &str) -> Result<()> {
    match status {
        AccountStatus::Active => Ok(()),
        AccountStatus::Deleted => Err(AcsError::AccountBlocked(format!(
            "{who}账户已注销，不可进行交易"
        ))),
        AccountStatus::Frozen => Err(AcsError::AccountBlocked(format!(
            "{who}账户已被冻结，不可进行交易"
        ))),
        AccountStatus::Closed => Err(AcsError::AccountBlocked(format!(
            "{who}账户已关闭，不可进行交易"
        ))),
    }
}

/// 全量重算并回写所有账户余额：balance = Σ(计入的收款) − Σ(计入的支出)。
///
/// **计入口径（v3.2.1 起，两侧不对称）**：
/// - **支出（发送方）**：`Pending` + `Confirmed` 均计入 —— 转单一提交就冻结发送方余额，
///   同一笔钱无法被重复花出（`Rejected` / `Error` 不计入，金额自动退回）。
/// - **收入（接收方）**：**仅 `Confirmed` 计入** —— 待收箱里的一笔在接收方点「接受」
///   之前不属于接收方；点了接受（状态转 Confirmed、接收方链头推进）才真正到账。
///
/// 为什么收支口径要不对称：`Pending` 是「已冻结、待接收方表态」的中间态。对付款方而言钱
/// 已经花出去了（必须立即扣，否则能重复花）；对收款方而言钱还没到手（不能记账，
/// 否则「还没点接受就到账」）。两侧都算或都不算都会产生错误语义。
///
/// 收支口径**必须与 `transaction` 里的结算/入账逻辑逐条对应，不可随意增删**：
/// - 收入：`Mint` / `Issue` / `Transfer` 的收款方
/// - 支出：`Redeem` / `Transfer` 的付款方
///
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
         WHERE status='Confirmed' AND tx_type IN ('Mint','Issue','Transfer') \
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
/// 计入口径与 `recompute_all_balances` 一致（v3.2.1）：
/// 支出 `Pending`+`Confirmed` 计入；收入**仅 `Confirmed`** 计入（待收不算）。
pub fn recompute_account(conn: &Connection, uid: &str, atype: AccountType) -> Result<i64> {
    let received = incoming_confirmed(conn, uid, atype)?;
    let sent = outgoing_all(conn, uid, atype)?;
    signed_balance(conn, uid, atype, received, sent)
}

/// 原始净额（收入 − 支出，**不把负值夹取为 0**）：用于「可支付性」判定。
/// 口径与 `recompute_account` 完全一致（支出 Pending+Confirmed 计入、收入仅 Confirmed 计入），
/// 仅不做负值兜底 —— 否则透支会被看成「余额 0」而无法识别。
pub fn raw_balance(conn: &Connection, uid: &str, atype: AccountType) -> Result<i64> {
    Ok(incoming_confirmed(conn, uid, atype)? - outgoing_all(conn, uid, atype)?)
}

/// 已确认收款合计：`Mint` / `Issue` / `Transfer` 中状态为 `Confirmed` 的部分。
/// **不含 `Pending`**：待收箱里的钱在接收方点「接受」前不算余额（v3.2.1）。
fn incoming_confirmed(conn: &Connection, uid: &str, atype: AccountType) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions \
             WHERE receiver=?1 AND receiver_type=?2 AND status='Confirmed' \
               AND tx_type IN ('Mint','Issue','Transfer')",
            params![uid, atype.as_str()],
            |r| r.get(0),
        )
        .unwrap_or(0))
}

/// 支出合计（含 `Pending`）：`Redeem` / `Transfer` 中状态为 `Pending` / `Confirmed` 的部分。
/// 含 `Pending` 是刻意的：转单一提交（Pending）就冻结发送方余额，防同一笔钱被重复花出。
fn outgoing_all(conn: &Connection, uid: &str, atype: AccountType) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(SUM(amount),0) FROM transactions \
             WHERE sender=?1 AND sender_type=?2 AND status IN ('Pending','Confirmed') \
               AND tx_type IN ('Redeem','Transfer')",
            params![uid, atype.as_str()],
            |r| r.get(0),
        )
        .unwrap_or(0))
}

/// 把「收入 − 支出」写成该账户余额并返回；负差额按 0 落库并记日志。
fn signed_balance(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
    received: i64,
    sent: i64,
) -> Result<i64> {
    let mut inc = HashMap::new();
    if received != 0 {
        inc.insert((uid.to_string(), atype.as_str().to_string()), received);
    }
    let mut out = HashMap::new();
    if sent != 0 {
        out.insert((uid.to_string(), atype.as_str().to_string()), sent);
    }
    let bal = settle_balance(&inc, &out, uid, atype.as_str());
    conn.execute(
        &format!("UPDATE {} SET balance=?1 WHERE uid=?2", atype.table_name()),
        params![bal, uid],
    )?;
    Ok(bal)
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
