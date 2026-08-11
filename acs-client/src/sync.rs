//! 中心镜像同步：通过只读 apikey 拉取增量交易与账户快照，写入本地。
//!
//! 信任模型：中心 > 本地 > 镜像。镜像快照带 sha256 哈希，若本地存有中心公钥可校验签名。

use anyhow::{anyhow, Result};
use rusqlite::params;

use crate::wallet::Wallet;

/// 同步结果（自动选择最快镜像/中心）。
#[derive(Debug, Default)]
pub struct SyncResult {
    /// 实际使用的数据源（中心或镜像地址）。
    pub source: String,
    pub server_time: i64,
    pub txs: usize,
    pub accounts: usize,
    pub hash: String,
    pub central_sig: Option<String>,
}

/// 同步：向中心请求可用镜像列表，自动 ping 各候选（中心+镜像）延迟，选最快端点拉增量。
/// since 取本地已知的最大交易时间戳。client 免 apikey（镜像拉取服务才需要 apikey）。
pub fn pull(w: &Wallet) -> Result<SyncResult> {
    let server = w.info.server_url.trim().trim_end_matches('/');
    if server.is_empty() {
        return Err(anyhow!("尚未配置中心服务器地址（请在设置中填写 server_url）"));
    }

    // 1) 候选端点：中心自身 + 社区镜像列表
    let mut candidates: Vec<String> = vec![server.to_string()];
    if let Ok(resp) = ureq::get(&format!("{server}/api/mirror/list"))
        .timeout(std::time::Duration::from_secs(5))
        .call()
    {
        if let Ok(j) = resp.into_json::<serde_json::Value>() {
            if let Some(arr) = j.get("mirrors").and_then(|v| v.as_array()) {
                for m in arr {
                    if let Some(u) = m.get("url").and_then(|v| v.as_str()) {
                        let u = u.trim().trim_end_matches('/');
                        if !u.is_empty() && !candidates.contains(&u.to_string()) {
                            candidates.push(u.to_string());
                        }
                    }
                }
            }
        }
    }

    // 2) ping 各候选 /api/status，选延迟最低者
    let mut best: Option<(String, std::time::Duration)> = None;
    for base in &candidates {
        let t0 = std::time::Instant::now();
        let ok = ureq::get(&format!("{base}/api/status"))
            .timeout(std::time::Duration::from_secs(3))
            .call()
            .is_ok();
        let dt = t0.elapsed();
        if ok && best.as_ref().map_or(true, |(_, d)| dt < *d) {
            best = Some((base.clone(), dt));
        }
    }
    let (chosen, _lat) = best.ok_or_else(|| {
        anyhow!(
            "无法连接任何候选端点（中心 {}，社区镜像 {}）：{}。请检查中心地址是否带 https、frp 隧道是否启动并绑定该域名、网络是否可达。",
            server,
            candidates.len().saturating_sub(1),
            candidates.join("、")
        )
    })?;

    // 3) 从最快端点拉增量（GET /api/sync?since=X）
    let since: i64 = w
        .conn
        .query_row("SELECT COALESCE(MAX(ts),0) FROM local_ledger", [], |r| r.get(0))
        .unwrap_or(0);
    let resp = ureq::get(&format!("{chosen}/api/sync?since={since}"))
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| anyhow!("连接 {chosen} 失败：{e}"))?;
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
            // 兼容中心（sender/sender_type）与镜像（peer/peer_type）字段
            let sender = t.get("peer").or_else(|| t.get("sender")).and_then(|v| v.as_str()).unwrap_or_default();
            let sender_type = t.get("peer_type").or_else(|| t.get("sender_type")).and_then(|v| v.as_str()).unwrap_or_default();
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
        source: chosen,
        server_time,
        txs,
        accounts,
        hash,
        central_sig,
    })
}
