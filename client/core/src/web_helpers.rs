//! WebView 登录/注册辅助（Tauri command 与 CLI 共用）。

use acs_core::models::AccountType;

use crate::client_api;
use crate::wallet::Wallet;

/// 校验中心地址已配置（v3.3.0 #7：**不再有编译期/环境默认地址**，
/// 中心必须由用户在启动流程里选择，见 #8/#11）。
pub fn ensure_server(w: &Wallet) -> anyhow::Result<()> {
    if w.info.server_url.trim().is_empty() {
        anyhow::bail!("尚未配置中心服务地址：请先在「中心地址」中选择或输入服务域名");
    }
    Ok(())
}

/// 拉取并按 `#7 / D39` 缓存服务端下发的客户端配置（离线/失败时保留旧缓存，不视为错误）。
pub fn refresh_client_config(w: &mut Wallet) {
    if w.info.server_url.trim().is_empty() {
        return;
    }
    if let Ok(cfg) = client_api::fetch_config(w) {
        // #15：把服务名称回填到中心历史，启动页优先按名称展示
        if let Some(name) = cfg.get("service_name").and_then(|v| v.as_str()) {
            crate::wallet::update_center_name(&w.domain, name).ok();
        }
        let _ = w.set_client_config(&cfg);
    }
}

/// 界面品牌信息：优先用服务端下发的配置，其次中性默认值。
/// `source` 字段供 UI 提示「配置来自中心 / 使用默认」。
pub fn brand_payload(w: &Wallet) -> serde_json::Value {
    let cfg = w.client_config();
    let get = |key: &str, fallback: &str| {
        cfg.as_ref()
            .and_then(|c| c.get(key))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| fallback.to_string())
    };
    serde_json::json!({
        "ok": true,
        "name": get("brand_name", "Alpha Coin"),
        "currency": get("currency", "A€"),
        "system_name": get("system_name", "Alpha Coin System"),
        "client_app": get("client_app", "Alpha Client"),
        "union_name": get("union_name", "Alpha Economy Union"),
        "union_name_cn": get("union_name_cn", "阿尔法经济联盟"),
        "union_abbr": get("union_abbr", "AEU"),
        // 中心地址以本地为准（用户选择），配置里的 public_url 仅作参考
        "public_url": w.info.server_url,
        "service_name": cfg.as_ref().and_then(|c| c.get("service_name")).cloned().unwrap_or(serde_json::json!("")),
        "min_password_len": cfg.as_ref().and_then(|c| c.get("min_password_len")).and_then(|v| v.as_u64()).unwrap_or(8),
        "update_repo": get("update_repo", ""),
        "source": if cfg.is_some() { "server" } else { "default" },
    })
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
    ensure_server(w)?;
    refresh_client_config(w);
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
/// v3.3.0（#4/#5）：国家 / 企业需提供 `fullname`（全称，同类型唯一），且 `uid`（简称）需符合
/// 「国家 2 位大写字母 / 企业 4 位大写字母」规则；个人账户 `fullname` 传空串。
pub fn register_account(
    w: &mut Wallet,
    server_url: &str,
    uid: &str,
    atype: AccountType,
    fullname: &str,
    email: &str,
    pass: &str,
    agree_terms: bool,
    agree_privacy: bool,
) -> anyhow::Result<()> {
    if !agree_terms || !agree_privacy {
        anyhow::bail!("注册须先同意《使用协议》与《隐私政策》");
    }
    // 本地先校验一次（中心端还会再校验，此处为更快反馈）
    acs_core::account::validate_uid(atype, uid)?;
    if matches!(atype, AccountType::Country | AccountType::Company) && fullname.trim().is_empty() {
        anyhow::bail!("国家 / 企业账户必须填写全称");
    }
    // 注册表单提供了地址则覆盖；否则沿用已选中的中心（#7：不再有编译期默认地址）
    if !server_url.trim().is_empty() {
        w.set_server_url(server_url)?;
    }
    ensure_server(w)?;
    refresh_client_config(w);
    let gk = w.create_key(uid, email, pass)?;
    w.init_wallet(uid, atype, email)?;
    // 只上传“口令加密后的私钥”；不上传口令，也不上传任何口令哈希
    w.save_local_account(uid, atype, email, &gk.pubkey, &gk.encrypted_seckey)?;
    client_api::open_account(w, &gk.encrypted_seckey, fullname, agree_terms, agree_privacy)?;
    Ok(())
}
