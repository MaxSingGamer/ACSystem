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
    // since 取“上次同步到的中心时间”——客户端不再保存账本副本，故不再从本地账本取 MAX(ts)
    let since: i64 = w.info.synced_at;
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

    // 客户端不再保存账本副本（local_ledger 已取消）：这里只统计本次同步中属于本账户的交易条数。
    // 账本（流水图 / 月度总账）改为按需从中心拉取，见本文件 fetch_ledger()。
    let our = w.info.uid.as_str();
    let our_type = w.info.atype.as_str();
    let mut txs = 0usize;
    if let Some(arr) = data.get("transactions").and_then(|v| v.as_array()) {
        for t in arr {
            let sender = t.get("sender").and_then(|v| v.as_str()).unwrap_or_default();
            let sender_type = t.get("sender_type").and_then(|v| v.as_str()).unwrap_or_default();
            let receiver = t.get("receiver").and_then(|v| v.as_str()).unwrap_or_default();
            let receiver_type = t.get("receiver_type").and_then(|v| v.as_str()).unwrap_or_default();
            // 归属判定必须 uid + 类型同时匹配（否则同名不同类的账户会串账）
            if (sender == our && sender_type == our_type)
                || (receiver == our && receiver_type == our_type)
            {
                txs += 1;
            }
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

/// 判断交易是否属于本账户（必须 **uid + 类型** 同时匹配）：返回方向 +1 收 / -1 支。
///
/// 为什么要带类型：管理员登录名可能与个人账户 UID 完全同名（历史 bug 现场）。
/// 若只按 uid 归属，系统侧的同名交易（如铸造、系统账本操作）会被错记到个人账户流水里。
fn own_direction(
    w: &Wallet,
    sender: &str,
    sender_type: &str,
    receiver: &str,
    receiver_type: &str,
) -> Option<i64> {
    own_direction_of(w.info.uid.as_str(), w.info.atype.as_str(), sender, sender_type, receiver, receiver_type)
}

/// 归属判定的纯函数版本（便于单元测试同名场景）。
pub fn own_direction_of(
    our_uid: &str,
    our_type: &str,
    sender: &str,
    sender_type: &str,
    receiver: &str,
    receiver_type: &str,
) -> Option<i64> {
    if sender == our_uid && sender_type == our_type {
        Some(-1)
    } else if receiver == our_uid && receiver_type == our_type {
        Some(1)
    } else {
        None
    }
}

/// 按需从中心拉取“本账户”的账本流水（**不落盘**，仅供 UI 展示）。
///
/// 返回数组元素（与前端 `state.txs` 各下标一致）：
/// `[tx_id, tx_type, peer, peer_type, amount, ts, status, direction]`
pub fn fetch_ledger(w: &Wallet) -> Result<Vec<serde_json::Value>> {
    let server = w.info.server_url.trim().trim_end_matches('/');
    if server.is_empty() {
        return Err(anyhow!("尚未配置中心服务器地址（请在设置中填写 server_url）"));
    }
    acs_core::log::net(&format!("GET {server}/api/sync?since=0 (ledger)"));
    let resp = shared_agent()
        .get(&format!("{server}/api/sync?since=0"))
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| anyhow!("连接中心失败：{e}"))?;
    let j: serde_json::Value = resp.into_json().map_err(|e| anyhow!("响应解析失败：{e}"))?;
    let mut out: Vec<(i64, serde_json::Value)> = Vec::new();
    if let Some(arr) = j
        .get("data")
        .and_then(|d| d.get("transactions"))
        .and_then(|v| v.as_array())
    {
        for t in arr {
            let tx_id = t.get("tx_id").and_then(|v| v.as_str()).unwrap_or_default();
            let tx_type = t.get("tx_type").and_then(|v| v.as_str()).unwrap_or_default();
            let sender = t.get("sender").and_then(|v| v.as_str()).unwrap_or_default();
            let sender_type = t.get("sender_type").and_then(|v| v.as_str()).unwrap_or_default();
            let receiver = t.get("receiver").and_then(|v| v.as_str()).unwrap_or_default();
            let receiver_type = t.get("receiver_type").and_then(|v| v.as_str()).unwrap_or_default();
            let amount = t.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            let ts = t.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
            let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let Some(dir) = own_direction(w, sender, sender_type, receiver, receiver_type) else {
                continue;
            };
            let (peer, peer_type) = if dir >= 0 {
                (sender, sender_type)
            } else {
                (receiver, receiver_type)
            };
            out.push((
                ts,
                serde_json::json!([tx_id, tx_type, peer, peer_type, amount, ts, status, dir]),
            ));
        }
    }
    out.sort_by_key(|(ts, _)| *ts);
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

#[cfg(test)]
mod tests {
    use super::own_direction_of;

    /// 同名账户（管理员登录名 == 个人账户 UID）下的归属隔离：
    /// 系统侧同名交易不得记入个人账户流水，反之亦然。
    #[test]
    fn samename_uid_is_not_attributed_across_types() {
        // 我们的账户：uid=testroot，类型=Individual
        let ours = ("testroot", "Individual");

        // 1) 系统侧铸造：sender=testroot 但 sender_type=System → 不属于个人账户
        assert_eq!(
            own_direction_of(ours.0, ours.1, "testroot", "System", "TestSystem", "System"),
            None
        );
        // 2) 系统侧收款：receiver=testroot 但 receiver_type=System → 也不属于
        assert_eq!(
            own_direction_of(ours.0, ours.1, "TestMinter", "System", "testroot", "System"),
            None
        );
        // 3) 真正发给个人账户的发行：类型匹配 → 收入
        assert_eq!(
            own_direction_of(ours.0, ours.1, "TestSystem", "System", "testroot", "Individual"),
            Some(1)
        );
        // 4) 个人账户转出：类型匹配 → 支出
        assert_eq!(
            own_direction_of(ours.0, ours.1, "testroot", "Individual", "BobT", "Individual"),
            Some(-1)
        );
        // 5) 同名不同类的第三方账户（同样 uid）不算我们的交易
        assert_eq!(
            own_direction_of(ours.0, ours.1, "testroot", "Company", "others", "Individual"),
            None
        );
        // 6) 同名且同类型才算
        assert_eq!(
            own_direction_of(ours.0, ours.1, "testroot", "Individual", "others", "Individual"),
            Some(-1)
        );
    }
}
