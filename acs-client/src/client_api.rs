//! 与中心的账户/交易交互：开立、提交 outbox、待确认查询、确认/拒绝。
//! 复用镜像 apikey 认证；开立上传公钥，提交携带签名交易，确认用接收方私钥签名。

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::wallet::Wallet;

fn base(w: &Wallet) -> Result<String> {
    let url = w.info.server_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err(anyhow!("尚未配置中心服务器地址"));
    }
    Ok(url.to_string())
}

/// 开立账户：导出钱包公钥并上传到中心。
pub fn open_account(w: &Wallet) -> Result<serde_json::Value> {
    let url = base(w)?;
    if w.info.mirror_apikey.is_empty() {
        return Err(anyhow!("尚未配置镜像 apikey"));
    }
    let fp = w
        .fingerprint(&w.info.uid)
        .ok_or_else(|| anyhow!("未找到钱包密钥"))?;
    let pubkey = w
        .gpg
        .export_public_key(&fp)
        .map_err(|e| anyhow!("导出公钥失败：{e}"))?;
    let body = json!({
        "apikey": w.info.mirror_apikey,
        "uid": w.info.uid,
        "type": w.info.atype.as_str(),
        "email": w.info.email,
        "pubkey": pubkey,
    });
    match ureq::post(&format!("{url}/api/client/open"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send_json(body)
    {
        Ok(resp) => Ok(resp.into_json().map_err(|e| anyhow!("响应解析失败：{e}"))?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(anyhow!("中心返回 HTTP {code}: {text}"))
        }
        Err(e) => Err(anyhow!("连接失败：{e}")),
    }
}

/// 提交 outbox 中指定（或全部 Pending）交易到中心。
/// 返回 (提交数, 各 tx_id 结果)。
pub fn submit_outbox(w: &Wallet, tx_id: Option<&str>) -> Result<Vec<(String, String)>> {
    let url = base(w)?;
    if w.info.mirror_apikey.is_empty() {
        return Err(anyhow!("尚未配置镜像 apikey"));
    }
    let mut stmt = w
        .conn
        .prepare("SELECT tx_id, tx_json FROM outbox WHERE state='Pending' ORDER BY created_at")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap();
    let mut picked = Vec::new();
    for r in rows.flatten() {
        if let Some(only) = tx_id {
            if only != r.0 {
                continue;
            }
        }
        picked.push(r);
    }
    if picked.is_empty() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for (id, tx_json) in &picked {
        let tx: acs_core::models::Transaction = match serde_json::from_str(tx_json) {
            Ok(t) => t,
            Err(e) => {
                results.push((id.clone(), format!("outbox 数据损坏: {e}")));
                continue;
            }
        };
        let body = json!({ "apikey": w.info.mirror_apikey, "tx": tx });
        match ureq::post(&format!("{url}/api/client/submit"))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send_json(body)
        {
            Ok(resp) => {
                let _: serde_json::Value = resp
                    .into_json()
                    .map_err(|e| anyhow!("响应解析失败：{e}"))?;
                w.conn
                    .execute(
                        "UPDATE outbox SET state='Submitted' WHERE tx_id=?1",
                        rusqlite::params![id],
                    )?;
                results.push((id.clone(), "Submitted".into()));
            }
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp
                    .into_string()
                    .unwrap_or_default();
                results.push((id.clone(), format!("中心拒绝(HTTP {code}): {text}")));
            }
            Err(e) => {
                results.push((id.clone(), format!("连接失败：{e}")));
            }
        }
    }
    Ok(results)
}

/// 查询待确认交易（作为接收方）。
#[derive(Debug)]
#[allow(dead_code)] // tx_type/timestamp 供 TUI 列表展示
pub struct PendingTx {
    pub tx_id: String,
    pub tx_type: String,
    pub sender: String,
    pub amount: i64,
    pub timestamp: i64,
}

pub fn list_pending(w: &Wallet) -> Result<Vec<PendingTx>> {
    let url = base(w)?;
    let resp = ureq::get(&format!(
        "{url}/api/client/pending?apikey={}&uid={}&type={}",
        w.info.mirror_apikey,
        w.info.uid,
        w.info.atype.as_str()
    ))
    .timeout(std::time::Duration::from_secs(15))
    .call()
    .map_err(|e| anyhow!("连接中心失败：{e}"))?;
    let j: serde_json::Value = resp
        .into_json()
        .map_err(|e| anyhow!("响应解析失败：{e}"))?;
    let mut out = Vec::new();
    if let Some(items) = j.get("items").and_then(|v| v.as_array()) {
        for it in items {
            out.push(PendingTx {
                tx_id: it.get("tx_id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                tx_type: it.get("tx_type").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                sender: it.get("sender").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                amount: it.get("amount").and_then(|v| v.as_i64()).unwrap_or(0),
                timestamp: it.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0),
            });
        }
    }
    Ok(out)
}

/// 接收方确认（或拒绝）交易：用钱包私钥对 tx_id 签名后提交。
pub fn confirm_tx(
    w: &Wallet,
    tx_id: &str,
    passphrase: &str,
    reject_reason: Option<&str>,
) -> Result<serde_json::Value> {
    let url = base(w)?;
    let fp = w
        .fingerprint(&w.info.uid)
        .ok_or_else(|| anyhow!("未找到钱包密钥"))?;
    let sig = w
        .gpg
        .sign_detached(&fp, passphrase, tx_id.as_bytes())
        .map_err(|e| anyhow!("签名失败（口令可能不正确）：{e}"))?;
    let mut body = serde_json::Map::new();
    body.insert("apikey".into(), json!(w.info.mirror_apikey));
    body.insert("tx_id".into(), json!(tx_id));
    body.insert("receiver_sig".into(), json!(sig));
    if let Some(r) = reject_reason {
        body.insert("reject_reason".into(), json!(r));
    }
    let path = if reject_reason.is_some() { "reject" } else { "confirm" };
    match ureq::post(&format!("{url}/api/client/{path}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send_json(serde_json::Value::Object(body))
    {
        Ok(resp) => Ok(resp.into_json().map_err(|e| anyhow!("响应解析失败：{e}"))?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(anyhow!("中心返回 HTTP {code}: {text}"))
        }
        Err(e) => Err(anyhow!("连接失败：{e}")),
    }
}
