//! WebView 登录/注册辅助（Tauri command 与 CLI 共用）。

use acs_core::models::AccountType;

use crate::client_api;
use crate::wallet::Wallet;

/// 未配置中心时使用默认地址。
pub fn ensure_server(w: &mut Wallet) {
    if w.info.server_url.trim().is_empty() {
        let _ = w.set_server_url(crate::DEFAULT_SERVER);
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 登录：本地缓存取回，或向中心 fetch-key（服务端校验密码哈希）。
/// v3.1.0：须携带同意《使用协议》《隐私政策》标识（前端已强制勾选后才允许调用）。
pub fn login_account(
    w: &mut Wallet,
    uid: &str,
    pass: &str,
    agree_terms: bool,
    agree_privacy: bool,
) -> anyhow::Result<String> {
    ensure_server(w);
    if !agree_terms || !agree_privacy {
        anyhow::bail!("登录须先同意《使用协议》与《隐私政策》");
    }
    // 1) 本地缓存私钥：导入并校验口令
    if let Some(acc) = w.local_account(uid) {
        if !acc.encrypted_seckey.is_empty()
            && w.gpg.import_key(&acc.encrypted_seckey).is_ok()
            && w.fingerprint(uid)
                .and_then(|fp| w.gpg.verify_passphrase(&fp, pass).ok())
                .is_some()
        {
            w.switch_account(uid)?;
            return Ok(uid.to_string());
        }
    }
    // 2) 无缓存或口令不符：向中心取回（服务端校验密码哈希）
    let known_type = w.local_account(uid).map(|a| a.atype);
    let r = client_api::fetch_key(w, uid, known_type, pass, agree_terms, agree_privacy)?;
    let email = r.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sek = r
        .get("encrypted_seckey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if sek.is_empty() {
        anyhow::bail!("中心未存有该账户加密私钥（该账户非本客户端注册）");
    }
    // 中心返回的实际账户类型（未指定类型时中心自动匹配）
    let atype = r
        .get("type")
        .and_then(|v| v.as_str())
        .and_then(|s| AccountType::from_str(s))
        .or(known_type)
        .unwrap_or(AccountType::Individual);
    w.gpg.import_key(&sek).map_err(|e| anyhow::anyhow!("导入密钥失败：{e}"))?;
    w.save_local_account(uid, atype, &email, &sek)?;
    w.switch_account(uid)?;
    Ok(uid.to_string())
}

/// 注册新账户：本地建密钥 + 中心 open。
/// v3.1.0：须先同意《使用协议》《隐私政策》才允许注册（前端勾选后携带标识）。
pub fn register_account(
    w: &mut Wallet,
    server_url: &str,
    uid: &str,
    atype: AccountType,
    email: &str,
    pass: &str,
    agree_terms: bool,
    agree_privacy: bool,
) -> anyhow::Result<()> {
    if !agree_terms || !agree_privacy {
        anyhow::bail!("注册须先同意《使用协议》与《隐私政策》");
    }
    // 注册表单提供了地址则覆盖；否则沿用默认
    if !server_url.trim().is_empty() {
        w.set_server_url(server_url)?;
    }
    ensure_server(w);
    let gk = w.create_key(uid, email, pass)?;
    w.init_wallet(uid, atype, email)?;
    let salt = uuid::Uuid::new_v4().to_string();
    let password_hash = format!("{salt}${}", sha256_hex(format!("{salt}:{pass}").as_bytes()));
    w.save_local_account(uid, atype, email, &gk.encrypted_seckey)?;
    client_api::open_account(w, &gk.encrypted_seckey, &password_hash, agree_terms, agree_privacy)?;
    Ok(())
}
