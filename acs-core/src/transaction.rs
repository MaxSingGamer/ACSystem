//! 交易数据访问与结算（双方确认机制）。
//!
//! - `Mint`（铸造）：根管理员发起，只打入 PreIssuedAccount，**自动确认**（需中心签名）。
//! - `Transfer / Issue / Redeem`：提交后为 Pending，**接收方确认**后才结算。
//! 确认信息（`receiver_sig` / `confirmed_at` / `reject_reason`）直接内联在 `transactions` 表中，
//! 不再另建 `tx_confirmations` 附属表（该表的 confirmed 列与 status 冗余，且从未被任何查询读取）。
//! 所有结算在单个 `BEGIN IMMEDIATE` 事务内完成，重查余额与链头，防双花。

use rusqlite::{params, Connection, Row, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::errors::{AcsError, Result};
use crate::models::{
    AccountStatus, AccountType, Transaction, TransactionStatus, TransactionType,
};

const TX_COLS: &str = "tx_id, tx_type, sender, sender_type, receiver, receiver_type, amount, ts, received_at, tx_hash, sender_sig, central_sig, receiver_sig, sender_last_hash, receiver_last_hash, status, confirmed_at, reject_reason";

/// 交易规范序列化（不含 tx_hash / 签名 / 状态），用于计算 tx_hash。
pub fn canonical_string(tx: &Transaction) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        tx.tx_id,
        tx.tx_type.as_str(),
        tx.sender,
        tx.sender_type.as_str(),
        tx.receiver,
        tx.receiver_type.as_str(),
        tx.amount,
        tx.timestamp,
        tx.sender_last_hash.as_deref().unwrap_or(""),
        tx.receiver_last_hash.as_deref().unwrap_or(""),
    )
}

/// 计算交易哈希：sha256(规范序列化)。
pub fn compute_tx_hash(tx: &Transaction) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_string(tx).as_bytes());
    hex::encode(hasher.finalize())
}

/// 账户账本链的创世种子。
pub fn account_chain_seed(uid: &str, atype: AccountType) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}|{}|genesis", uid, atype.as_str()).as_bytes());
    hex::encode(hasher.finalize())
}

fn map_tx(row: &Row) -> rusqlite::Result<Transaction> {
    let tx_type: String = row.get("tx_type")?;
    let sender_type: String = row.get("sender_type")?;
    let receiver_type: String = row.get("receiver_type")?;
    let status: String = row.get("status")?;
    Ok(Transaction {
        tx_id: row.get("tx_id")?,
        tx_type: TransactionType::from_str(&tx_type).unwrap_or(TransactionType::Transfer),
        sender: row.get("sender")?,
        sender_type: AccountType::from_str(&sender_type).unwrap_or(AccountType::Individual),
        receiver: row.get("receiver")?,
        receiver_type: AccountType::from_str(&receiver_type).unwrap_or(AccountType::Individual),
        amount: row.get("amount")?,
        timestamp: row.get("ts")?,
        received_at: row.get("received_at")?,
        tx_hash: row.get("tx_hash")?,
        sender_sig: row.get("sender_sig")?,
        central_sig: row.get("central_sig")?,
        receiver_sig: row.get("receiver_sig")?,
        sender_last_hash: row.get("sender_last_hash")?,
        receiver_last_hash: row.get("receiver_last_hash")?,
        status: TransactionStatus::from_str(&status).unwrap_or(TransactionStatus::Pending),
        confirmed_at: row.get("confirmed_at")?,
        reject_reason: row.get("reject_reason")?,
    })
}

fn insert_transaction(conn: &Connection, tx: &Transaction) -> Result<()> {
    let sql = format!(
        "INSERT INTO transactions({TX_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)"
    );
    conn.execute(
        &sql,
        params![
            tx.tx_id,
            tx.tx_type.as_str(),
            tx.sender,
            tx.sender_type.as_str(),
            tx.receiver,
            tx.receiver_type.as_str(),
            tx.amount,
            tx.timestamp,
            tx.received_at,
            tx.tx_hash,
            tx.sender_sig,
            tx.central_sig,
            tx.receiver_sig,
            tx.sender_last_hash,
            tx.receiver_last_hash,
            tx.status.as_str(),
            tx.confirmed_at,
            tx.reject_reason,
        ],
    )?;
    Ok(())
}

/// 按 tx_id 查询交易。
pub fn get_transaction(conn: &Connection, tx_id: &str) -> Result<Option<Transaction>> {
    let sql = format!("SELECT {TX_COLS} FROM transactions WHERE tx_id=?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![tx_id], map_tx)?;
    Ok(rows.next().transpose()?)
}

/// 列出某账户（发送方或接收方）的全部交易。
pub fn list_transactions_for(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
) -> Result<Vec<Transaction>> {
    let st = atype.as_str();
    let sql = format!(
        "SELECT {TX_COLS} FROM transactions \
         WHERE (sender=?1 AND sender_type=?2) OR (receiver=?1 AND receiver_type=?2) \
         ORDER BY ts DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![uid, st], map_tx)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 列出某账户的待确认账单（作为接收方且状态为 Pending）。
pub fn list_pending_for(
    conn: &Connection,
    uid: &str,
    atype: AccountType,
) -> Result<Vec<Transaction>> {
    let st = atype.as_str();
    let sql = format!(
        "SELECT {TX_COLS} FROM transactions \
         WHERE receiver=?1 AND receiver_type=?2 AND status='Pending' ORDER BY ts ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![uid, st], map_tx)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 提交交易。
/// - Mint：需中心签名，直接结算并 Confirmed（自动确认）。
/// - 其他：插入 Pending，等待接收方确认。
///
/// 核心不变量：**每笔交易必须有唯一哈希与有效签名**。
/// - `tx_hash` 由 `compute_tx_hash` 生成（含随机 tx_id）并由唯一索引保证不重复；
/// - Mint 的签名是 `central_sig`（根管理员，发送方非账本账户故无 sender_sig）；
/// - 其余类型的发送方签名 `sender_sig` 必须非空（服务端已先用公钥验签）。
pub fn submit_tx(conn: &mut Connection, tx: &Transaction) -> Result<()> {
    if tx.tx_hash.trim().is_empty() {
        return Err(AcsError::Message("交易缺少哈希".into()));
    }
    if tx.tx_type == TransactionType::Mint {
        require_sig(&tx.central_sig, "中心（根管理员）")?;
        let db = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        apply_settlement(&db, tx)?;
        // 自动确认：直接以 Confirmed 落库。central_sig 保留根管理员真签名，
        // 不再有「先 UPDATE 后 INSERT」的覆写陷阱（旧写法会往该列写入明文文本）。
        insert_transaction(&db, &confirmed_tx(tx, None))?;
        db.commit()?;
        return Ok(());
    }
    require_sig(&Some(tx.sender_sig.clone()), "发送方")?;
    // 非 Mint：提交为 Pending，校验发送方状态与余额
    let db = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let sender = crate::account::require_account(&db, &tx.sender, tx.sender_type)?;
    if sender.status != AccountStatus::Active {
        return Err(AcsError::AccountNotActive);
    }
    if tx.tx_type == TransactionType::Transfer || tx.tx_type == TransactionType::Redeem {
        if sender.balance < tx.amount {
            return Err(AcsError::InsufficientBalance);
        }
    }
    if sender.last_tx_hash.as_deref() != tx.sender_last_hash.as_deref() {
        return Err(AcsError::HashMismatch("发送方链头不一致".into()));
    }
    insert_transaction(&db, tx)?;
    db.commit()?;
    Ok(())
}

/// 接收方确认交易并结算。
/// `receiver_sig`：接收方对 `tx_id` 的分离签名（**必填**，服务端已用接收方公钥验签；
/// 后台代管系统账本账户时由服务端用该账户密钥现签）。
pub fn confirm_tx(
    conn: &mut Connection,
    tx_id: &str,
    actor_uid: &str,
    actor_type: AccountType,
    receiver_sig: &str,
) -> Result<()> {
    if receiver_sig.trim().is_empty() {
        return Err(AcsError::Unauthorized("接收方确认签名缺失".into()));
    }
    let db = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tx = require_tx(&db, tx_id)?;
    if tx.status != TransactionStatus::Pending {
        return Err(AcsError::Message("该交易已处理（不可再修改状态）".into()));
    }
    if tx.receiver != actor_uid || tx.receiver_type != actor_type {
        return Err(AcsError::Unauthorized("仅接收方可确认该交易".into()));
    }
    apply_settlement(&db, &tx)?;
    db.execute(
        "UPDATE transactions SET status='Confirmed', receiver_sig=?1, confirmed_at=?2 WHERE tx_id=?3",
        params![receiver_sig, chrono::Utc::now().timestamp(), tx_id],
    )?;
    db.commit()?;
    Ok(())
}
/// 接收方拒绝交易（理由写入主表，可供对账与审计查询）。
pub fn reject_tx(
    conn: &mut Connection,
    tx_id: &str,
    actor_uid: &str,
    actor_type: AccountType,
    reason: &str,
) -> Result<()> {
    let db = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tx = require_tx(&db, tx_id)?;
    if tx.status != TransactionStatus::Pending {
        return Err(AcsError::Message("该交易已处理（不可再修改状态）".into()));
    }
    if tx.receiver != actor_uid || tx.receiver_type != actor_type {
        return Err(AcsError::Unauthorized("仅接收方可拒绝该交易".into()));
    }
    db.execute(
        "UPDATE transactions SET status='Rejected', reject_reason=?1, confirmed_at=?2 WHERE tx_id=?3",
        params![reason, chrono::Utc::now().timestamp(), tx_id],
    )?;
    db.commit()?;
    Ok(())
}

/// 将交易标记为错误状态（金额不计入余额，此后不可再修改状态）。
/// 用于结算/校验失败的交易（如签名/链头不一致导致无法推进）。
pub fn mark_error(
    conn: &mut Connection,
    tx_id: &str,
) -> Result<()> {
    let db = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tx = require_tx(&db, tx_id)?;
    if tx.status != TransactionStatus::Pending {
        return Err(AcsError::Message("该交易已处理（不可再修改状态）".into()));
    }
    db.execute(
        "UPDATE transactions SET status='Error', reject_reason='error', confirmed_at=?1 WHERE tx_id=?2",
        params![chrono::Utc::now().timestamp(), tx_id],
    )?;
    db.commit()?;
    Ok(())
}

/// 终态（不可再修改状态）：Rejected / Error。
pub fn is_terminal(status: TransactionStatus) -> bool {
    matches!(
        status,
        TransactionStatus::Rejected | TransactionStatus::Error
    )
}

/// 按类型执行账本结算（内部，须在事务内调用）。
/// 注意：Mint 的发送方为根管理员（非账本账户），不校验发送方。
fn apply_settlement(conn: &Connection, tx: &Transaction) -> Result<()> {
    let receiver = crate::account::require_account(conn, &tx.receiver, tx.receiver_type)?;
    if receiver.status != AccountStatus::Active {
        return Err(AcsError::AccountNotActive);
    }
    if receiver.last_tx_hash.as_deref() != tx.receiver_last_hash.as_deref() {
        return Err(AcsError::HashMismatch("接收方链头不一致".into()));
    }
    let hash = Some(tx.tx_hash.as_str());
    match tx.tx_type {
        TransactionType::Mint => {
            // 铸造增发：仅接收方(PreIssuedAccount)增加
            crate::account::update_balance_and_hash(
                conn, &tx.receiver, tx.receiver_type, receiver.balance + tx.amount, hash,
            )?;
        }
        TransactionType::Issue => {
            // 商品篮子兑换 A€：PreIssuedAccount(发行源) 余额不变，接收方增加
            crate::account::update_balance_and_hash(
                conn, &tx.receiver, tx.receiver_type, receiver.balance + tx.amount, hash,
            )?;
        }
        TransactionType::Redeem => {
            let sender = crate::account::require_account(conn, &tx.sender, tx.sender_type)?;
            if sender.status != AccountStatus::Active {
                return Err(AcsError::AccountNotActive);
            }
            if sender.balance < tx.amount {
                return Err(AcsError::InsufficientBalance);
            }
            if sender.last_tx_hash.as_deref() != tx.sender_last_hash.as_deref() {
                return Err(AcsError::HashMismatch("发送方链头不一致".into()));
            }
            crate::account::update_balance_and_hash(
                conn, &tx.sender, tx.sender_type, sender.balance - tx.amount, hash,
            )?;
        }
        TransactionType::Transfer => {
            let sender = crate::account::require_account(conn, &tx.sender, tx.sender_type)?;
            if sender.status != AccountStatus::Active {
                return Err(AcsError::AccountNotActive);
            }
            if sender.balance < tx.amount {
                return Err(AcsError::InsufficientBalance);
            }
            if sender.last_tx_hash.as_deref() != tx.sender_last_hash.as_deref() {
                return Err(AcsError::HashMismatch("发送方链头不一致".into()));
            }
            crate::account::update_balance_and_hash(
                conn, &tx.sender, tx.sender_type, sender.balance - tx.amount, hash,
            )?;
            crate::account::update_balance_and_hash(
                conn, &tx.receiver, tx.receiver_type, receiver.balance + tx.amount, hash,
            )?;
        }
    }
    Ok(())
}

/// 断言签名存在且非空（核心不变量）。
fn require_sig(sig: &Option<String>, who: &str) -> Result<()> {
    match sig.as_deref() {
        Some(s) if !s.trim().is_empty() => Ok(()),
        _ => Err(AcsError::Unauthorized(format!("{who}签名缺失"))),
    }
}

/// 生成「已确认」副本：设置状态与确认时间（Mint 自动确认 / 接收方确认）。
fn confirmed_tx(tx: &Transaction, receiver_sig: Option<&str>) -> Transaction {
    let mut t = tx.clone();
    t.status = TransactionStatus::Confirmed;
    t.confirmed_at = Some(chrono::Utc::now().timestamp());
    if let Some(s) = receiver_sig {
        t.receiver_sig = Some(s.to_string());
    }
    t
}

fn require_tx(conn: &Connection, tx_id: &str) -> Result<Transaction> {
    get_transaction(conn, tx_id)?.ok_or_else(|| AcsError::Message(format!("交易不存在: {tx_id}")))
}
