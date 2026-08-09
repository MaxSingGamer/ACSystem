//! 中心镜像同步：通过只读 apikey 拉取增量交易与账户快照，写入本地。
//!
//! 信任模型：中心 > 本地 > 镜像。镜像快照带 sha256 哈希，若本地存有中心公钥可校验签名。

use anyhow::{anyhow, Result};
use rusqlite::params;

use crate::wallet::Wallet;

/// 镜像拉取结果。
#[derive(Debug, Default)]
pub struct SyncResult {
    pub server_time: i64,
    pub txs: usize,
    pub accounts: usize,
    pub hash: String,
    pub central_sig: Option<String>,
}

/// 单次拉取同步：POST {server_url}/api/mirror/pull {apikey, since}。
/// since 取本地已知的最大交易时间戳。
pub fn pull(w: &Wallet) -> Result<SyncResult> {
    let url = w.info.server_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err(anyhow!("尚未配置中心服务器地址（请在设置中填写 server_url）"));
    }
    if w.info.mirror_apikey.is_empty() {
        return Err(anyhow!("尚未配置镜像 apikey（请在设置中填写）"));
    }

    let since: i64 = w
        .conn
        .query_row("SELECT COALESCE(MAX(ts),0) FROM local_ledger", [], |r| r.get(0))
        .unwrap_or(0);

    let body = serde_json::json!({ "apikey": w.info.mirror_apikey, "since": since });
    let resp = ureq::post(&format!("{url}/api/mirror/pull"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send_json(body)
        .map_err(|e| anyhow!("连接中心失败：{e}"))?;
    let j: serde_json::Value = resp
        .into_json()
        .map_err(|e| anyhow!("响应解析失败：{e}"))?;

    let data = j
        .get("data")
        .ok_or_else(|| anyhow!("响应缺少 data"))?;
    let hash = j
        .get("hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let central_sig = j
        .get("central_sig")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // 可选：校验中心签名（本地有中心公钥时）
    // TODO: 若 known_pubkeys 存有中心公钥，用 gpg.verify_detached 校验 hash 签名。

    // 合并交易到 local_ledger
    let mut txs = 0usize;
    if let Some(arr) = data.get("transactions").and_then(|v| v.as_array()) {
        for t in arr {
            let tx_id = t.get("tx_id").and_then(|v| v.as_str()).unwrap_or_default();
            let tx_type = t.get("tx_type").and_then(|v| v.as_str()).unwrap_or_default();
            let sender = t.get("sender").and_then(|v| v.as_str()).unwrap_or_default();
            let sender_type = t.get("sender_type").and_then(|v| v.as_str()).unwrap_or_default();
            let receiver = t.get("receiver").and_then(|v| v.as_str()).unwrap_or_default();
            let receiver_type = t.get("receiver_type").and_then(|v| v.as_str()).unwrap_or_default();
            let amount = t.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            let ts = t.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
            let tx_hash = t.get("tx_hash").and_then(|v| v.as_str()).unwrap_or_default();
            let central_sig = t.get("central_sig").and_then(|v| v.as_str());
            let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("Pending");

            let n = w.conn.execute(
                "INSERT OR IGNORE INTO local_ledger(tx_id, tx_type, peer, peer_type, amount, ts, tx_hash, central_sig, status) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    tx_id, tx_type, sender, sender_type, amount, ts, tx_hash,
                    central_sig.unwrap_or_default(), status
                ],
            )?;
            let _ = receiver;
            let _ = receiver_type;
            txs += n;
        }
    }

    // 合并账户快照到 mirror_accounts
    let mut accounts = 0usize;
    if let Some(arr) = data.get("accounts").and_then(|v| v.as_array()) {
        for a in arr {
            let uid = a.get("uid").and_then(|v| v.as_str()).unwrap_or_default();
            if uid.is_empty() {
                continue;
            }
            let atype = a.get("type").and_then(|v| v.as_str()).unwrap_or_default();
            let balance = a.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
            let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("Active");
            let last_tx_hash = a.get("last_tx_hash").and_then(|v| v.as_str());
            let changed_at = a.get("changed_at").and_then(|v| v.as_i64()).unwrap_or(0);
            let now = chrono::Utc::now().timestamp();
            let n = w.conn.execute(
                "INSERT INTO mirror_accounts(uid,type,balance,status,last_tx_hash,changed_at,synced_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7) \
                 ON CONFLICT(uid) DO UPDATE SET \
                   type=excluded.type, balance=excluded.balance, status=excluded.status, \
                   last_tx_hash=excluded.last_tx_hash, changed_at=excluded.changed_at, synced_at=excluded.synced_at",
                params![uid, atype, balance, status, last_tx_hash, changed_at, now],
            )?;
            accounts += n;
        }
    }

    let server_time = data
        .get("server_time")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    Ok(SyncResult {
        server_time,
        txs,
        accounts,
        hash,
        central_sig,
    })
}
