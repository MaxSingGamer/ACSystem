//! 账户数据访问（按账户类型路由到不同中心表；UID 唯一识别，无 abbr）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};

use crate::errors::{AcsError, Result};
use crate::models::{Account, AccountStatus, AccountType};

const ACCOUNT_COLS: &str =
    "uid, fullname, email, pubkey, encrypted_seckey, balance, status, last_tx_hash, created_at, changed_at";

fn map_account(row: &Row, atype: AccountType) -> rusqlite::Result<Account> {
    let status: String = row.get("status")?;
    let created: i64 = row.get("created_at")?;
    let changed: i64 = row.get("changed_at")?;
    Ok(Account {
        uid: row.get("uid")?,
        account_type: atype,
        fullname: row.get("fullname").unwrap_or_default(),
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
    let sql = format!(
        "INSERT INTO {table}({ACCOUNT_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"
    );
    conn.execute(
        &sql,
        params![
            acc.uid,
            acc.fullname,
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

/// 账户「简称」规则（v3.3.0 #4/#5 / D4：uid 即简称）。
///
/// - `Country`：**恰好 2 位、仅大写字母 A–Z**
/// - `Company`：**恰好 4 位、仅大写字母 A–Z**
/// - `Individual` / `System`：不限（个人 UID 为游戏 ID；系统账户由 `.env` 定义）
pub fn validate_uid(atype: AccountType, uid: &str) -> Result<()> {
    let (want, label) = match atype {
        AccountType::Country => (2, "国家账户简称需为 2 位大写字母（A–Z）"),
        AccountType::Company => (4, "企业账户简称需为 4 位大写字母（A–Z）"),
        _ => return Ok(()),
    };
    if uid.chars().count() != want || !uid.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(AcsError::InvalidArgument(format!(
            "{label}：实际为 {}（{uid}）",
            uid.chars().count()
        )));
    }
    Ok(())
}

/// 开立新账户前的完整校验（v3.3.0 #4/#5）：
/// 1) `uid` 符合类型规则（见 [`validate_uid`]）；
/// 2) 国家 / 企业必须提供非空 `fullname`；
/// 3) `fullname` 在**同类型内唯一**（DB 另有部分唯一索引兜底）。
///
/// Individual / System 不校验 fullname（恒为空串）。
pub fn validate_new_account(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
    fullname: &str,
) -> Result<()> {
    validate_uid(atype, uid)?;
    let fullname = fullname.trim();
    match atype {
        AccountType::Country | AccountType::Company => {
            if fullname.is_empty() {
                return Err(AcsError::InvalidArgument(
                    "国家 / 企业账户必须填写全称（fullname）".into(),
                ));
            }
            let table = atype.table_name();
            let n: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE fullname=?1"),
                params![fullname],
                |r| r.get(0),
            )?;
            if n > 0 {
                return Err(AcsError::InvalidArgument(format!(
                    "该全称已被同类型账户占用：{fullname}"
                )));
            }
        }
        _ => {}
    }
    Ok(())
}

/// 按 (uid, type) 取全称（展示用；不存在或为空串返回空串）。
pub fn fullname_of(conn: &Connection, uid: &str, atype: AccountType) -> Result<String> {
    let table = atype.table_name();
    let v: Option<String> = conn
        .query_row(
            &format!("SELECT fullname FROM {table} WHERE uid=?1"),
            params![uid],
            |r| r.get(0),
        )
        .ok();
    Ok(v.unwrap_or_default())
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

/// 中心库「余额体检水位」键名（存于 `meta` 表，值为上次体检时的交易表最大 rowid）。
pub const BALANCE_SANITY_KEY: &str = "balance_sanity_rowid";

/// 交易表当前最大 rowid（无交易时 0）。
///
/// 用途：判断「自上次体检以来账本是否新增过交易」，从而**只在必要时**做全量余额重算。
/// 交易只增不删（`transactions` 无删除路径），故 rowid 单调可用作变更标记；
/// 若将来出现删除，rowid 会跳号，仍能正确触发一次重算（保守方向是安全的）。
pub fn latest_tx_rowid(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row("SELECT COALESCE(MAX(rowid),0) FROM transactions", [], |r| {
            r.get(0)
        })
        .unwrap_or(0))
}

/// 自上次「余额体检」以来账本是否有新增交易（首启无水位视为有变化 → 体检一次）。
pub fn balance_dirty(conn: &Connection) -> Result<bool> {
    let marker = conn
        .query_row(
            "SELECT v FROM meta WHERE k=?1",
            params![BALANCE_SANITY_KEY],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok());
    let current = latest_tx_rowid(conn)?;
    Ok(marker != Some(current))
}

/// 记录「已完成余额体检」的水位（与 `balance_dirty` 配对）。
pub fn mark_balance_clean(conn: &Connection) -> Result<()> {
    let current = latest_tx_rowid(conn)?;
    conn.execute(
        "INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
        params![BALANCE_SANITY_KEY, current.to_string()],
    )?;
    Ok(())
}

/// 需要时才做全量余额重算：有新增交易（或首启无水位）才重算并推进水位，否则直接返回。
///
/// 为什么需要：`/api/sync` 是**免认证**端点，客户端每 3 分钟拉一次。此前每次都要跑两条
/// 聚合 + 逐账户 `UPDATE`（4 张表全量），既放大写入又长时间占用全局库锁，多客户端线性叠加。
/// 余额在提交/确认/拒收时已增量维护，全量重算只是「纠偏体检」，无需每次请求都做。
/// 返回是否真的执行了重算。
pub fn recompute_all_if_dirty(conn: &Connection) -> Result<bool> {
    if !balance_dirty(conn)? {
        return Ok(false);
    }
    recompute_all_balances(conn)?;
    mark_balance_clean(conn)?;
    Ok(true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(db::CENTRAL_SCHEMA).unwrap();
        c
    }

    fn insert(conn: &Connection, uid: &str, atype: AccountType, fullname: &str) {
        let t = atype.table_name();
        conn.execute(
            &format!(
                "INSERT INTO {t}(uid, fullname, email, pubkey, encrypted_seckey, balance, status, created_at, changed_at)
                 VALUES (?1,?2,?3,'pub','sek',0,'Active',0,0)"
            ),
            params![uid, fullname, format!("{uid}@x.test")],
        )
        .unwrap();
    }

    /// v3.3.0（#4/#5 / D31）：国家 2 位、企业 4 位、仅大写字母。
    #[test]
    fn uid_rules() {
        assert!(validate_uid(AccountType::Country, "LE").is_ok());
        assert!(validate_uid(AccountType::Country, "LEO").is_err());
        assert!(validate_uid(AccountType::Country, "le").is_err());
        assert!(validate_uid(AccountType::Country, "L1").is_err());
        assert!(validate_uid(AccountType::Company, "ALPH").is_ok());
        assert!(validate_uid(AccountType::Company, "ALP").is_err());
        assert!(validate_uid(AccountType::Company, "ALPHX").is_err());
        assert!(validate_uid(AccountType::Company, "AL1H").is_err());
        assert!(validate_uid(AccountType::Company, "alph").is_err());
        // 个人 / 系统不限
        assert!(validate_uid(AccountType::Individual, "Steve-Ok").is_ok());
        assert!(validate_uid(AccountType::System, "AlphaEU").is_ok());
    }

    /// v3.3.0（#4/#5）：国家 / 企业全称必填且**同类型内唯一**；个人不需要全称。
    #[test]
    fn fullname_required_and_unique_within_type() {
        let c = mem();
        assert!(validate_new_account(&c, "ALPH", AccountType::Company, "").is_err());
        assert!(validate_new_account(&c, "ALPH", AccountType::Company, "   ").is_err());

        insert(&c, "ALPH", AccountType::Company, "Alpha Bank");
        assert!(validate_new_account(&c, "BETA", AccountType::Company, "Alpha Bank").is_err());
        assert!(validate_new_account(&c, "BETA", AccountType::Company, "Beta Bank").is_ok());

        // 跨类型可同名（只要求同类型唯一）
        insert(&c, "LE", AccountType::Country, "Alpha Bank");
        assert!(validate_new_account(&c, "KO", AccountType::Country, "Alpha Bank").is_err(), "国家类型内仍须唯一");

        // 个人账户不需要全称
        assert!(validate_new_account(&c, "steve", AccountType::Individual, "").is_ok());
    }

    /// fullname 可被读回（create/read 一致性）。
    #[test]
    fn fullname_roundtrip() {
        let c = mem();
        insert(&c, "ALPH", AccountType::Company, "Alpha Bank Co.,Ltd.");
        let a = get_account(&c, "ALPH", AccountType::Company).unwrap().unwrap();
        assert_eq!(a.fullname, "Alpha Bank Co.,Ltd.");
        assert_eq!(fullname_of(&c, "ALPH", AccountType::Company).unwrap(), "Alpha Bank Co.,Ltd.");
        // 个人账户 fullname 为空
        insert(&c, "steve", AccountType::Individual, "");
        assert_eq!(fullname_of(&c, "steve", AccountType::Individual).unwrap(), "");
    }
}
