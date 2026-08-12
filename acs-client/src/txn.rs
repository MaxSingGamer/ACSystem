//! 交易构建与签名：本地构造 Transfer，用钱包密钥对交易哈希做 detached 签名，
//! 写入 outbox 待提交（后续由中心端点接收并进入双方确认流程）。

use anyhow::{anyhow, Result};
use rusqlite::params;

use acs_core::models::{AccountType, Transaction, TransactionStatus, TransactionType};
use acs_core::transaction;

use crate::wallet::Wallet;

/// 构建并签名一笔转账（本地 outbox）。
/// 返回 (tx_id, tx_hash)。
pub fn build_and_sign_transfer(
    w: &Wallet,
    receiver: &str,
    receiver_type: AccountType,
    amount: i64,
    passphrase: &str,
) -> Result<(String, String)> {
    if amount <= 0 {
        return Err(anyhow!("金额须大于 0"));
    }
    if w.info.uid.is_empty() {
        return Err(anyhow!("钱包尚未初始化"));
    }
    if receiver.trim().is_empty() {
        return Err(anyhow!("接收方 UID 不能为空"));
    }
    let uid = w.info.uid.clone();
    let atype = w.info.atype;

    // 本账户链头：镜像快照中的 last_tx_hash（若无则用 genesis）
    let last_hash: Option<String> = w
        .conn
        .query_row(
            "SELECT last_tx_hash FROM mirror_accounts WHERE uid=?1",
            params![uid],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let sender_last_hash = last_hash
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| transaction::account_chain_seed(&uid, atype));

    // 接收方链头：同样取镜像快照或 genesis（tx_hash 与签名都依赖它，须与中心一致）
    let recv_last: Option<String> = w
        .conn
        .query_row(
            "SELECT last_tx_hash FROM mirror_accounts WHERE uid=?1",
            params![receiver.trim()],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let receiver_last_hash = recv_last
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| transaction::account_chain_seed(receiver.trim(), receiver_type));

    let mut tx = Transaction::new(
        TransactionType::Transfer,
        uid.clone(),
        atype,
        receiver.trim().to_string(),
        receiver_type,
        amount,
    );
    tx.sender_last_hash = Some(sender_last_hash);
    tx.receiver_last_hash = Some(receiver_last_hash);
    tx.timestamp = chrono::Utc::now().timestamp();
    tx.tx_hash = transaction::compute_tx_hash(&tx);

    // 用钱包密钥签名交易哈希（校验口令）
    let fp = w
        .fingerprint(&uid)
        .ok_or_else(|| anyhow!("未找到钱包密钥（指纹），请检查密钥生成"))?;
    let sig = w
        .gpg
        .sign_detached(&fp, passphrase, tx.tx_hash.as_bytes())
        .map_err(|e| anyhow!("签名失败（口令可能不正确）：{e}"))?;
    tx.sender_sig = sig;

    let tx_json = serde_json::to_string(&tx)?;
    w.conn.execute(
        "INSERT INTO outbox(tx_id, tx_json, created_at, state) VALUES (?1,?2,?3,'Pending')",
        params![tx.tx_id, tx_json, chrono::Utc::now().timestamp()],
    )?;
    Ok((tx.tx_id.clone(), tx.tx_hash.clone()))
}

/// 列出本地交易历史（本地账本，含我方相关的交易）。
pub fn list_local_tx(w: &Wallet, limit: usize) -> Vec<(String, String, String, String, i64, i64, String)> {
    let uid = w.info.uid.clone();
    let mut stmt = w
        .conn
        .prepare(
            "SELECT tx_id, tx_type, peer, peer_type, amount, ts, status \
             FROM local_ledger \
             WHERE peer=?1 OR peer=?1 \
             ORDER BY ts DESC LIMIT ?2",
        )
        .unwrap();
    let rows = stmt
        .query_map(params![uid, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .unwrap();
    let mut out = Vec::new();
    for r in rows.flatten() {
        out.push(r);
    }
    out
}

/// 列出 outbox 待提交。
pub fn list_outbox(w: &Wallet) -> Vec<(String, String, i64)> {
    let mut stmt = w
        .conn
        .prepare("SELECT tx_id, state, created_at FROM outbox ORDER BY created_at DESC")
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .unwrap();
    let mut out = Vec::new();
    for r in rows.flatten() {
        out.push(r);
    }
    out
}

/// 供 UI 展示用的本地余额口径：镜像快照余额。
#[allow(dead_code)] // 旧 TUI 使用，Web 版用 wallet::mirror_balance
pub fn balance(w: &Wallet) -> i64 {
    w.mirror_balance()
}

// 让 TransactionStatus / Transaction 类型被引用（防止未使用告警）的辅助。
pub fn _touch(_t: &Transaction, _s: TransactionStatus) {}
