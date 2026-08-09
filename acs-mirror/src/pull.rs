//! 从中心拉取增量账本与账户快照，写入本地镜像库。
//! 可选：若配置了中心公钥，用 gpg 校验快照哈希的中心签名（防篡改）。

use anyhow::{anyhow, Result};
use rusqlite::params;

use crate::store::Store;

#[derive(Debug)]
#[allow(dead_code)] // server_time 供后续状态接口使用
pub struct PullResult {
    pub txs: usize,
    pub accounts: usize,
    pub hash: String,
    pub central_sig: Option<String>,
    pub server_time: i64,
}

/// 执行一次拉取并合并。
pub fn pull(store: &mut Store) -> Result<PullResult> {
    let url = store.info.server_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err(anyhow!("尚未配置中心地址（用 `acs-mirror config --server <url> --apikey <key>`）"));
    }
    if store.info.apikey.is_empty() {
        return Err(anyhow!("尚未配置镜像 apikey"));
    }
    let since = store.since();
    let body = serde_json::json!({ "apikey": store.info.apikey, "since": since });
    let resp = ureq::post(&format!("{url}/api/mirror/pull"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(body)
        .map_err(|e| anyhow!("连接中心失败：{e}"))?;
    let j: serde_json::Value = resp
        .into_json()
        .map_err(|e| anyhow!("响应解析失败：{e}"))?;

    let data = j.get("data").ok_or_else(|| anyhow!("响应缺少 data"))?;
    let hash = j
        .get("hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let central_sig = j
        .get("central_sig")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // 可选校验中心签名（配置了中心公钥时）
    if !store.info.central_pubkey.is_empty() {
        let sig = central_sig
            .as_deref()
            .ok_or_else(|| anyhow!("中心未返回签名，无法校验"))?;
        let gpg_bin = acs_core::gpg_detect::find_gpg()
            .unwrap_or_else(|| std::path::PathBuf::from("gpg.exe"));
        let gpg = acs_core::gpg::GpgUtil::new(gpg_bin, std::env::temp_dir().join("acs-mirror-gnupg"));
        let ok = gpg
            .verify_detached(&store.info.central_pubkey, hash.as_bytes(), sig)
            .map_err(|e| anyhow!("验签失败：{e}"))?;
        if !ok {
            return Err(anyhow!("中心签名校验失败（快照可能被篡改）"));
        }
    }

    let mut txs = 0usize;
    if let Some(arr) = data.get("transactions").and_then(|v| v.as_array()) {
        for t in arr {
            let tx_id = t.get("tx_id").and_then(|v| v.as_str()).unwrap_or_default();
            let tx_type = t.get("tx_type").and_then(|v| v.as_str()).unwrap_or_default();
            let sender = t.get("sender").and_then(|v| v.as_str()).unwrap_or_default();
            let sender_type = t.get("sender_type").and_then(|v| v.as_str()).unwrap_or_default();
            let amount = t.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            let ts = t.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
            let tx_hash = t.get("tx_hash").and_then(|v| v.as_str()).unwrap_or_default();
            let central_sig = t.get("central_sig").and_then(|v| v.as_str());
            let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("Pending");
            let n = store.conn.execute(
                "INSERT OR IGNORE INTO local_ledger(tx_id,tx_type,peer,peer_type,amount,ts,tx_hash,central_sig,status) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    tx_id, tx_type, sender, sender_type, amount, ts, tx_hash,
                    central_sig.unwrap_or_default(), status
                ],
            )?;
            txs += n;
        }
    }

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
            let n = store.conn.execute(
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
    store.mark_synced(server_time, &hash)?;
    Ok(PullResult {
        txs,
        accounts,
        hash,
        central_sig,
        server_time,
    })
}
