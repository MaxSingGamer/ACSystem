//! AlphaTar 服务包（v3.3.0 #13 / D36）。
//!
//! 文件：`<域名>.gz.alphatar`
//! 结构：目录 → tar → gzip →（可选）整包 AES-256-GCM 加密。
//!
//! 两种形态（导入时按文件头自动识别）：
//! - **未加密**：gzip 数据（魔数 `1f 8b`）
//! - **已加密**：`b"ALPHATAR"` + 版本字节(1) + 12 字节 nonce + AES-256-GCM 密文（含 tag）
//!   密钥 = SHA-256(导出时设置的密码)；密码不落盘，导入时需输入同一密码。
//!
//! 注：包内**不含服务名称**（D20/第 5 条：名称由导入方另设），也不含面板注册表信息；
//! 域名 / 端口等识别信息一律从包内 `<域名>/.env` 读取。

use std::io::{Read, Write};
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::tarx;

const MAGIC: &[u8; 8] = b"ALPHATAR";
const VERSION: u8 = 1;

/// 把服务目录打成 AlphaTar 字节（先 tar 再 gzip；`password` 非空则再整体加密）。
///
/// `top` = 包内顶层目录名（服务域名）；导入方据此识别服务并落到 `<services_root>/<top>/`。
pub fn export_package(dir: &Path, top: &str, password: Option<&str>) -> Result<Vec<u8>> {
    let tar = tarx::pack_dir(dir, top).with_context(|| format!("打包失败：{}", dir.display()))?;
    // gzip
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar)?;
    let plain = gz.finish()?;
    match password.filter(|p| !p.is_empty()) {
        None => Ok(plain),
        Some(pwd) => Ok(encrypt(&plain, pwd)?),
    }
}

/// 从 AlphaTar 字节解出 tar 条目；`password` 仅在文件为加密形态时使用。
pub fn import_package(data: &[u8], password: Option<&str>) -> Result<Vec<tarx::Entry>> {
    let plain = if data.starts_with(MAGIC) {
        let pwd = password.filter(|p| !p.is_empty()).ok_or_else(|| {
            anyhow::anyhow!("该服务包已加密：请输入导出时设置的密码")
        })?;
        decrypt(data, pwd)?
    } else if data.starts_with(&[0x1f, 0x8b]) {
        data.to_vec()
    } else {
        bail!("不是有效的 AlphaTar 服务包（文件头无法识别）");
    };
    let mut gz = GzDecoder::new(&plain[..]);
    let mut tar = Vec::new();
    gz.read_to_end(&mut tar).context("解压失败（包已损坏或密码错误）")?;
    tarx::unpack(&tar)
}

/// 把 AlphaTar 解包到 `<services_root>/<顶层层名>` 并返回顶层目录名（域名字符串）。
pub fn extract_package(entries: &[tarx::Entry], services_root: &Path) -> Result<String> {
    let tops = tarx::top_level(entries);
    if tops.len() != 1 {
        bail!("服务包结构异常：顶层应只有一个目录（当前 {} 个）", tops.len());
    }
    let top = tops.into_iter().next().unwrap();
    // 顶层必须是 `<域名>/`：简单校验非法字符
    if top.is_empty() || top.contains('\\') || top.contains(':') || top == ".." {
        bail!("服务包顶层目录名非法：{top}");
    }
    let dest = services_root;
    tarx::extract_to(entries, dest)?;
    Ok(top)
}

/// 是否加密包（供 UI 提示是否需要密码；测试中亦用于断言）。
#[allow(dead_code)]
pub fn is_encrypted(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

fn derive_key(password: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(password.as_bytes());
    let d = h.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&d);
    k
}

fn encrypt(plain: &[u8], password: &str) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(&derive_key(password)).expect("valid key");
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| anyhow::anyhow!("加密失败"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + 1 + 12 + ct.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn decrypt(data: &[u8], password: &str) -> Result<Vec<u8>> {
    if data.len() < MAGIC.len() + 1 + 12 + 16 {
        bail!("加密包过短或已损坏");
    }
    let ver = data[MAGIC.len()];
    if ver != VERSION {
        bail!("不支持的 AlphaTar 加密版本：{ver}");
    }
    let nonce = &data[MAGIC.len() + 1..MAGIC.len() + 13];
    let ct = &data[MAGIC.len() + 13..];
    let cipher = Aes256Gcm::new_from_slice(&derive_key(password)).expect("valid key");
    cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow::anyhow!("解密失败：密码错误或包已损坏"))
}

/// 从键值对里取某个键。
pub fn env_get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sample_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "acs-alphatar-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("logs")).unwrap();
        fs::write(d.join(".env"), b"ACS_PUBLIC_URL=https://example.test\n").unwrap();
        fs::write(d.join("logs/1.log"), b"line1\n").unwrap();
        fs::write(d.join("alpha_center.db"), vec![7u8; 4096]).unwrap();
        d
    }

    #[test]
    fn plain_roundtrip() {
        let d = sample_dir("plain");
        let pkg = export_package(&d, "example.test", None).unwrap();
        assert!(!is_encrypted(&pkg));
        let entries = import_package(&pkg, None).unwrap();
        let tops = tarx::top_level(&entries);
        assert_eq!(tops, vec!["example.test".to_string()], "顶层必须且只能是域名目录");
        // 包内应含 .env 与数据库，且总字节数为三者之和
        let env = entries.iter().find(|e| e.path == "example.test/.env").expect("应含 .env");
        assert!(String::from_utf8_lossy(&env.data).contains("ACS_PUBLIC_URL="));
        assert_eq!(tarx::total_bytes(&entries), 36 + 6 + 4096);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn encrypted_roundtrip_and_wrong_password() {
        let d = sample_dir("enc");
        let pkg = export_package(&d, "example.test", Some("P@ss-1234")).unwrap();
        assert!(is_encrypted(&pkg));
        assert!(import_package(&pkg, None).is_err(), "无密码应拒绝");
        assert!(import_package(&pkg, Some("wrong")).is_err(), "错误密码应拒绝");
        let entries = import_package(&pkg, Some("P@ss-1234")).unwrap();
        assert_eq!(tarx::top_level(&entries).len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_garbage() {
        assert!(import_package(b"not-a-package", None).is_err());
    }
}
