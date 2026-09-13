//! WebView 登录/注册辅助（Tauri command 与 CLI 共用）。

use acs_core::models::AccountType;

use crate::client_api;
use crate::wallet::Wallet;

/// 未配置中心时使用默认地址（可由 .env 的 `ACS_PUBLIC_URL` 定制）。
pub fn ensure_server(w: &mut Wallet) {
    if w.info.server_url.trim().is_empty() {
        let _ = w.set_server_url(&crate::default_server());
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

/// 取本机 gpg 钥环中该账户的公钥（armored）。失败返回空串（不阻断登录）。
fn local_pubkey(w: &Wallet, uid: &str) -> String {
    w.fingerprint(uid)
        .and_then(|fp| w.gpg.export_public_key(&fp).ok())
        .unwrap_or_default()
}

/// 登录：本地历史账户缓存取回（免输 UID），或向中心 fetch-key。
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
    // 1) 本地历史账户缓存私钥：导入并校验口令（口令即私钥解密口令）
    if let Some(acc) = w.local_account(uid) {
        if !acc.encrypted_seckey.is_empty()
            && w.gpg.import_key(&acc.encrypted_seckey).is_ok()
            && w.fingerprint(uid)
                .and_then(|fp| w.gpg.verify_passphrase(&fp, pass).ok())
                .is_some()
        {
            // 首次登录时补写公钥（登录界面展示历史账户用）
            if acc.pubkey.is_empty() {
                let pk = local_pubkey(w, uid);
                if !pk.is_empty() {
                    let _ = w.save_local_account(
                        uid,
                        acc.atype,
                        &acc.email,
                        &pk,
                        &acc.encrypted_seckey,
                    );
                }
            }
            w.switch_account(uid)?;
            return Ok(uid.to_string());
        }
    }
    // 2) 无缓存或口令不符：向中心取回（仅取回加密私钥，服务端不校验口令）
    let known_type = w.local_account(uid).map(|a| a.atype);
    let r = client_api::fetch_key(w, uid, known_type, agree_terms, agree_privacy)?;
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
    // 校验口令：口令错则不入账（否则历史账户列表会留下无法解密的条目）
    let fp = w
        .fingerprint(uid)
        .ok_or_else(|| anyhow::anyhow!("导入后未找到账户 {uid} 的密钥"))?;
    w.gpg
        .verify_passphrase(&fp, pass)
        .map_err(|_| anyhow::anyhow!("密码错误：无法解开该账户私钥"))?;
    let pk = w.gpg.export_public_key(&fp).unwrap_or_default();
    w.save_local_account(uid, atype, &email, &pk, &sek)?;
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
    // 只上传“口令加密后的私钥”；不上传口令，也不上传任何口令哈希
    w.save_local_account(uid, atype, email, &gk.pubkey, &gk.encrypted_seckey)?;
    client_api::open_account(w, &gk.encrypted_seckey, agree_terms, agree_privacy)?;
    Ok(())
}
