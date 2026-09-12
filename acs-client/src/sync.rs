//! 中心同步：直接从中心服务器拉取增量交易与账户快照，写入本地。
//!
//! 信任模型：中心为唯一记账权威。快照带 sha256 哈希，若本地存有中心公钥可校验签名。

use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Result};
use rusqlite::params;

use crate::wallet::Wallet;

/// 共享 ureq Agent：显式启用 native-tls（ureq 的快捷调用不会自动用 native-tls，
/// 必须在 AgentBuilder 上配置 tls_connector，否则 HTTPS 请求报 "no TLS backend"）。
pub fn shared_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let connector = native_tls::TlsConnector::builder()
            .build()
            .expect("初始化 TLS 连接器失败");
        ureq::AgentBuilder::new()
            .tls_connector(Arc::new(connector))
            .build()
    })
}

/// 同步结果。
#[derive(Debug, Default)]
pub struct SyncResult {
    /// 实际使用的数据源（中心地址）。
    pub source: String,
    pub server_time: i64,
    pub txs: usize,
    pub accounts: usize,
    pub hash: String,
    pub central_sig: Option<String>,
}

/// 同步：直接从中心服务器拉取增量（v3.0.0 取消镜像，不再发现多个端点）。
/// since 取本地已知的最大交易时间戳。client 免 apikey。
pub fn pull(w: &Wallet) -> Result<SyncResult> {
    let server = w.info.server_url.trim().trim_end_matches('/');
    if server.is_empty() {
        return Err(anyhow!("尚未配置中心服务器地址（请在设置中填写 server_url）"));
    }

    // 1) 中心直接拉增量（GET /api/sync?since=X）
    let since: i64 = w
        .conn
        .query_row("SELECT COALESCE(MAX(ts),0) FROM local_ledger", [], |r| r.get(0))
        .unwrap_or(0);
    acs_core::log::net(&format!("GET {server}/api/sync?since={since}"));
    let resp = shared_agent().get(&format!("{server}/api/sync?since={since}"))
        .timeout(std::time::Duration::from_secs(15))
        .call()
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

    // 合并交易到 local_ledger（仅与本账户相关的交易；direction: +1 收 / -1 支 / 0 未知）
    let our = w.info.uid.as_str();
    let our_type = w.info.atype.as_str();
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

            // 仅记录与本账户相关的交易；据此确定方向与对方。
            // 必须同时比较“类型”：否则与本人同名的其他类型账户（如管理员系统身份）会串账。
            let (direction, peer, peer_type) = if sender == our && sender_type == our_type {
                (-1i64, receiver, receiver_type)
            } else if receiver == our && receiver_type == our_type {
                (1i64, sender, sender_type)
            } else {
                continue;
            };

            let n = w.conn.execute(
                "INSERT OR IGNORE INTO local_ledger(tx_id, tx_type, peer, peer_type, amount, ts, tx_hash, central_sig, status, sender, receiver, direction) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    tx_id, tx_type, peer, peer_type, amount, ts, tx_hash,
                    central_sig.unwrap_or_default(), status, sender, receiver, direction
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
                 ON CONFLICT(uid,type) DO UPDATE SET \
                   balance=excluded.balance, status=excluded.status, \
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
    acs_core::log::out(&format!(
        "同步完成：新增交易 {txs}、账户快照 {accounts}（源 {server}）"
    ));
    Ok(SyncResult {
        source: server.to_string(),
        server_time,
        txs,
        accounts,
        hash,
        central_sig,
    })
}
