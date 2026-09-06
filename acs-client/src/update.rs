//! 客户端自动更新：检查新版本 + 下载安装包。
//!
//! 更新来源优先级：GitHub Releases（官方源）→ 中心服务器下载 API。
//! 若 GitHub 不可达 / 超时 / 下载缓慢，自动切换至服务器下载（服务器端已做清单白名单、
//! sha256 校验与 IP 限速，杜绝任意文件下发）。

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::wallet::Wallet;

pub const GITHUB_API: &str = "https://api.github.com/repos/MaxSingGamer/ACSystem/releases/latest";
pub const GITHUB_DL: &str = "https://github.com/MaxSingGamer/ACSystem/releases/latest/download";

/// 当前运行平台（与打包命名一致）。
pub fn platform() -> String {
    if cfg!(target_os = "windows") {
        "windows-x64".to_string()
    } else if cfg!(target_os = "macos") {
        "macos-x64".to_string()
    } else {
        "linux-x64".to_string()
    }
}

/// 从安装包文件名解析版本：acs-client-3.1.0-windows-x64-setup.exe -> 3.1.0
fn version_from_filename(name: &str) -> Option<String> {
    let name = name.strip_prefix("acs-client-")?;
    let end = name.find('-')?;
    Some(name[..end].to_string())
}

/// 简单版本比较：a > b。
pub fn version_gt(a: &str, b: &str) -> bool {
    let nums = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split(|c: char| c == '.' || c == '-')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let (an, bn) = (nums(a), nums(b));
    for i in 0..an.len().max(bn.len()) {
        let x = an.get(i).copied().unwrap_or(0);
        let y = bn.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

fn server_base(w: &Wallet) -> String {
    w.info.server_url.trim().trim_end_matches('/').to_string()
}

/// 请求 GitHub 最新 Release 中匹配当前平台的安装包。
/// 返回 Option<(version, download_url, elapsed_ms)>；不可达/超时/无匹配 返回 None。
fn github_latest(plat: &str) -> Option<(String, String, u128)> {
    let t0 = Instant::now();
    let resp = match crate::sync::shared_agent()
        .get(GITHUB_API)
        .set("User-Agent", "ACSystem-Wallet")
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(5))
        .call()
    {
        Ok(r) => r,
        Err(_) => {
            acs_core::log::net("update 检查 GitHub 不可达，切换服务器源");
            return None;
        }
    };
    let j: Value = match resp.into_json() {
        Ok(j) => j,
        Err(_) => return None,
    };
    let tag = j.get("tag_name").and_then(|v| v.as_str()).unwrap_or_default();
    let mut best: Option<(String, String)> = None;
    if let Some(assets) = j.get("assets").and_then(|v| v.as_array()) {
        for a in assets {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let url = a.get("browser_download_url").and_then(|v| v.as_str()).unwrap_or_default();
            if !name.contains(&format!("-{plat}-setup.exe")) {
                continue;
            }
            let Some(ver) = version_from_filename(name) else { continue };
            let newer = best
                .as_ref()
                .map(|(bv, _)| version_gt(&ver, bv))
                .unwrap_or(true);
            if newer && !url.is_empty() {
                best = Some((ver, url.to_string()));
            }
        }
    }
    let (ver, url) = best?;
    // 至少得到一个可比较的版本号：以 release tag 为准（去掉前导 v）
    let effective = if tag.is_empty() { ver.clone() } else { tag.trim_start_matches('v').to_string() };
    acs_core::log::net(&format!("update GitHub 源: latest={effective} 耗时={}ms", t0.elapsed().as_millis()));
    Some((effective, url, t0.elapsed().as_millis()))
}

/// 请求中心服务器版本信息。
fn server_info(w: &Wallet, current: &str, plat: &str) -> Result<Value> {
    let url = format!(
        "{}/api/client/update/info?current={}&platform={}",
        server_base(w),
        urlencode(current),
        plat
    );
    let resp = crate::sync::shared_agent()
        .get(&url)
        .timeout(Duration::from_secs(8))
        .call()
        .map_err(|e| anyhow!("连接更新服务失败：{e}"))?;
    let j: Value = resp.into_json().map_err(|e| anyhow!("解析更新信息失败：{e}"))?;
    acs_core::log::net("update 服务器源返回版本信息");
    Ok(j)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 检查更新。返回统一结构：
/// { update_available, latest, current, notes, platform, source: "github"|"server",
///   download_url, github_url, server_url, file, size, sha256 }
///
/// 版本权威：**默认服务端始终保持最新**。客户端先向中心获取最新版本号，
/// 再判断 GitHub Releases 是否恰好等于该最新版：是则走 GitHub（官方镜像，速度快），
/// 否则（GitHub 不是最新 / 不可达 / 无匹配安装包）直接向中心服务器请求安装包。
pub fn check(w: &Wallet) -> Result<Value> {
    let current = acs_core::VERSION.to_string();
    let plat = platform();

    // 1) 以中心为权威，获取最新版本号与下载信息
    let j = server_info(w, &current, &plat)?;
    let latest = j.get("latest").and_then(|v| v.as_str()).unwrap_or(&current).to_string();
    let available = version_gt(&latest, &current);
    let notes = j.get("notes").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let file = j.get("file").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sha256 = j.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let size = j.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let github_dl = j.get("github_download_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let server_rel = j.get("server_download_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let server_full = if server_rel.starts_with("http") {
        server_rel.clone()
    } else {
        format!("{}{}", server_base(w), server_rel)
    };
    let server_url = format!("{}/api/client/update/info?current={}&platform={plat}", server_base(w), current);

    // 2) 判断 GitHub Release 是否为最新（相等才算最新）
    let gh = github_latest(&plat); // Option<(version, download_url, elapsed_ms)>
    let github_is_latest = gh.as_ref().map(|(v, _, _)| v == &latest).unwrap_or(false);
    let gh_url = gh.as_ref().map(|(_, u, _)| u.clone()).unwrap_or_else(|| github_dl.clone());

    let source = if github_is_latest { "github" } else { "server" };
    let dl = if github_is_latest { gh_url.clone() } else { server_full.clone() };
    acs_core::log::net(&format!(
        "update 版本权威=中心(latest={latest}, available={available}), GitHub是否最新={github_is_latest} → 下载源={source}"
    ));

    Ok(json!({
        "ok": true,
        "update_available": available,
        "latest": latest,
        "current": current,
        "notes": notes,
        "platform": plat,
        "source": source,
        "download_url": dl,
        "github_url": gh_url,
        "server_url": server_url,
        "file": file,
        "size": size,
        "sha256": sha256,
    }))
}

fn get_bytes(url: &str, timeout_secs: u64, max_bytes: u64) -> Result<Vec<u8>> {
    let resp = crate::sync::shared_agent()
        .get(url)
        .timeout(Duration::from_secs(timeout_secs))
        .call()
        .map_err(|e| anyhow!("下载失败：{e}"))?;
    let len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    if len > max_bytes {
        return Err(anyhow!("安装包过大（{} 字节），已拒绝下载", len));
    }
    let mut body = Vec::new();
    let mut reader = resp.into_reader().take(max_bytes + 1);
    reader
        .read_to_end(&mut body)
        .map_err(|e| anyhow!("读取下载内容失败：{e}"))?;
    if body.len() as u64 > max_bytes {
        return Err(anyhow!("下载内容超过大小上限"));
    }
    Ok(body)
}

fn sha256_hex(data: &[u8]) -> String {
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

/// 下载安装包到本地（系统下载目录 / 数据目录后备）。source 由 check() 决定（github|server）；
/// 若 GitHub 下载失败/缓慢自动回退到服务器下载。返回 { downloaded, path, filename, bytes, source }
pub fn download(w: &Wallet, source: &str, _expected_version: &str) -> Result<Value> {
    let plat = platform();
    let current = acs_core::VERSION.to_string();
    // 从服务器取权威最新版本 / sha256 / 地址
    let j = server_info(w, &current, &plat)?;
    let latest = j.get("latest").and_then(|v| v.as_str()).unwrap_or(&current).to_string();
    let sha256 = j.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let file_name = format!("acs-client-{latest}-{plat}-setup.exe");
    let server_dl = {
        let p = j.get("server_download_url").and_then(|v| v.as_str()).unwrap_or("");
        if p.starts_with("http") {
            p.to_string()
        } else {
            format!("{}{}", server_base(w), p)
        }
    };
    let github_url = format!("{GITHUB_DL}/{file_name}");

    let updates_dir = downloads_dir(w);
    std::fs::create_dir_all(&updates_dir)?;
    let dest: PathBuf = updates_dir.join(&file_name);

    // 已有同版本缓存则直接使用
    if dest.exists() {
        acs_core::log::out(&format!("update 使用已下载安装包 {}", dest.display()));
        return Ok(json!({
            "ok": true, "downloaded": true, "path": dest.to_string_lossy(),
            "filename": file_name, "bytes": std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
            "source": "cached",
        }));
    }

    let mut source_used = source.to_string();
    let mut bytes = None;
    // 优先 GitHub；失败或过慢（>15s）则切服务器
    if source == "github" {
        let t0 = Instant::now();
        match get_bytes(&github_url, 20, 800 * 1024 * 1024) {
            Ok(b) => {
                if t0.elapsed().as_secs() > 15 {
                    acs_core::log::net("update GitHub 下载偏慢，切换服务器源");
                } else {
                    bytes = Some(b);
                    source_used = "github".into();
                }
            }
            Err(e) => {
                acs_core::log::net(&format!("update GitHub 下载失败回退服务器：{e}"));
            }
        }
    }
    if bytes.is_none() {
        bytes = Some(get_bytes(&server_dl, 600, 800 * 1024 * 1024)?);
        source_used = "server".into();
    }
    let data = bytes.unwrap();

    // 校验 sha256（服务器清单提供时）
    if !sha256.is_empty() && source_used == "server" {
        let got = sha256_hex(&data);
        if got != sha256.trim().to_ascii_lowercase() {
            return Err(anyhow!("下载文件校验失败（sha256 不匹配），已中止安装"));
        }
        acs_core::log::out("update 下载文件 sha256 校验通过");
    }

    std::fs::write(&dest, &data)?;
    acs_core::log::out(&format!("update 下载完成 {}（{} 字节，来源 {source_used}）", dest.display(), data.len()));
    Ok(json!({
        "ok": true, "downloaded": true,
        "path": dest.to_string_lossy(),
        "filename": file_name,
        "bytes": data.len(),
        "source": source_used,
    }))
}

fn downloads_dir(w: &Wallet) -> PathBuf {
    let _ = w;
    // 默认放用户下载目录；失败则放数据目录
    if let Some(dl) = dirs_downloads() {
        return dl;
    }
    crate::wallet::data_dir_str().join("updates")
}

/// 尽量取系统「下载」目录；取不到则 None。
fn dirs_downloads() -> Option<PathBuf> {
    // Windows: %USERPROFILE%\Downloads
    #[cfg(target_os = "windows")]
    {
        if let Ok(p) = std::env::var("USERPROFILE") {
            let d = PathBuf::from(p).join("Downloads");
            if d.exists() || std::fs::create_dir_all(&d).is_ok() {
                return Some(d);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(d) = std::env::var_os("HOME") {
            let d = PathBuf::from(d).join("Downloads");
            if d.exists() || std::fs::create_dir_all(&d).is_ok() {
                return Some(d);
            }
        }
    }
    None
}

/// 校验服务器信息连通（供设置页手动检查）。
#[allow(dead_code)]
pub fn _server_ok(w: &Wallet) -> bool {
    server_info(w, acs_core::VERSION, &platform()).is_ok()
}
