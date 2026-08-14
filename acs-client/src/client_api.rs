//! 与中心的账户/交易交互：开立、提交 outbox、待确认查询、确认/拒绝。
//! 认证：client 凭 ed25519 签名与公钥，不依赖 apikey。

use anyhow::{anyhow, Result};
use serde_json::json;

use acs_core::models::AccountType;

use crate::sync::shared_agent;
use crate::wallet::Wallet;

fn base(w: &Wallet) -> Result<String> {
    let url = w.info.server_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err(anyhow!("尚未配置中心服务器地址"));
    }
    Ok(url.to_string())
}

/// 开立账户：导出钱包公钥，连同密码加密私钥与密码哈希上传到中心（支持多设备登录）。
pub fn open_account(
    w: &Wallet,
    encrypted_seckey: &str,
    password_hash: &str,
) -> Result<serde_json::Value> {
    let url = base(w)?;
    let fp = w
        .fingerprint(&w.info.uid)
        .ok_or_else(|| anyhow!("未找到钱包密钥"))?;
    let pubkey = w
        .gpg
        .export_public_key(&fp)
        .map_err(|e| anyhow!("导出公钥失败：{e}"))?;
    let body = json!({
        "uid": w.info.uid,
        "type": w.info.atype.as_str(),
        "email": w.info.email,
        "pubkey": pubkey,
        "encrypted_seckey": encrypted_seckey,
        "password_hash": password_hash,
    });
    match shared_agent().post(&format!("{url}/api/client/open"))
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

/// 登录：向中心请求取回加密私钥（服务端校验密码哈希后返回），供本机导入或跨设备恢复。
/// atype 为 None 时由中心按 UID 自动匹配账户类型。
pub fn fetch_key(
    w: &Wallet,
    uid: &str,
    atype: Option<AccountType>,
    password: &str,
) -> Result<serde_json::Value> {
    let url = base(w)?;
    let body = json!({
        "uid": uid,
        "type": atype.map(|t| t.as_str().to_string()).unwrap_or_default(),
        "password": password,
    });
    match shared_agent().post(&format!("{url}/api/client/fetch-key"))
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

/// 获取 AEU 已认定成员国家/银行（Active），供注册下拉选择。
pub fn fetch_members(w: &Wallet) -> Result<serde_json::Value> {
    let url = base(w)?;
    match shared_agent()
        .get(&format!("{url}/api/client/members"))
        .timeout(std::time::Duration::from_secs(15))
        .call()
    {
        Ok(resp) => Ok(resp.into_json().map_err(|e| anyhow!("响应解析失败：{e}"))?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(anyhow!("中心返回 HTTP {code}: {text}"))
        }
        Err(e) => Err(anyhow!("连接失败：{e}")),
    }
}

/// 注销账户：账户私钥签名后上传，中心将状态改为 Deleted（不可再登录，账本只读保留审计）。
pub fn close_account(w: &Wallet, passphrase: &str) -> Result<serde_json::Value> {
    let url = base(w)?;
    let fp = w
        .fingerprint(&w.info.uid)
        .ok_or_else(|| anyhow!("未找到钱包密钥"))?;
    let msg = format!("close:{}:{}", w.info.uid, w.info.atype.as_str());
    let sig = w
        .gpg
        .sign_detached(&fp, passphrase, msg.as_bytes())
        .map_err(|e| anyhow!("签名失败（口令可能不正确）：{e}"))?;
    let body = json!({
        "uid": w.info.uid,
        "type": w.info.atype.as_str(),
        "close_sig": sig,
    });
    match shared_agent().post(&format!("{url}/api/client/close"))
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
        let body = json!({ "tx": tx });
        match shared_agent().post(&format!("{url}/api/client/submit"))
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
#[allow(dead_code)] // tx_type/timestamp 供列表展示使用
pub struct PendingTx {
    pub tx_id: String,
    pub tx_type: String,
    pub sender: String,
    pub amount: i64,
    pub timestamp: i64,
}

pub fn list_pending(w: &Wallet) -> Result<Vec<PendingTx>> {
    let url = base(w)?;
    let resp = shared_agent().get(&format!(
        "{url}/api/client/pending?uid={}&type={}",
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
    body.insert("tx_id".into(), json!(tx_id));
    body.insert("receiver_sig".into(), json!(sig));
    if let Some(r) = reject_reason {
        body.insert("reject_reason".into(), json!(r));
    }
    let path = if reject_reason.is_some() { "reject" } else { "confirm" };
    match shared_agent().post(&format!("{url}/api/client/{path}"))
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
