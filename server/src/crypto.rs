//! AES-256-GCM：用登录密码加密管理员 gpg 密钥 passphrase（服务端不存任何明文）。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// 用 key 派生 256 位密钥并加密明文，返回 `nonce||ciphertext`（hex）。
pub fn encrypt_secret(key: &str, plain: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    let k = h.finalize();
    let cipher = Aes256Gcm::new_from_slice(&k).expect("valid key");
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
        .expect("encrypt");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    hex::encode(out)
}

/// 解密 `nonce||ciphertext`（hex），失败返回错误串。
pub fn decrypt_secret(data: &str, key: &str) -> Result<String, String> {
    let raw = hex::decode(data).map_err(|e| format!("hex 解码失败: {e}"))?;
    if raw.len() < 13 {
        return Err("密文过短".into());
    }
    let (nonce, ct) = raw.split_at(12);
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    let k = h.finalize();
    let cipher = Aes256Gcm::new_from_slice(&k).map_err(|e| e.to_string())?;
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| "解密失败（密码错误）".to_string())?;
    String::from_utf8(pt).map_err(|e| e.to_string())
}
