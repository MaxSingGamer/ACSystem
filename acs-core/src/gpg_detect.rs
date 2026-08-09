//! gpg.exe 定位与自动下载。
//!
//! 查找优先级：**PATH → 程序目录（随发行包附带）→ 自动下载 Gpg4win 静默安装**（Windows）。
//! 程序目录兼容：`<exe 目录>/gpg.exe`、`<exe 目录>/gpg/bin/gpg.exe`、`<exe 目录>/bin/gpg.exe`。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::errors::{AcsError, Result};

/// Gpg4win 安装包下载地址（可用环境变量 `ACS_GPG_DOWNLOAD_URL` 覆盖）。
const GPG4WIN_URL: &str = "https://files.gpg4win.org/gpg4win-4.3.0.exe";

/// gpg 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpgSource {
    Path,
    ProgramDir,
    Downloaded,
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "gpg.exe" } else { "gpg" }
}

/// 在 PATH 中查找 gpg。
pub fn find_gpg_in_path() -> Option<PathBuf> {
    let name = exe_name();
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// 在程序目录（当前可执行文件同级）查找 gpg。
pub fn find_gpg_in_program_dir() -> Option<PathBuf> {
    let name = exe_name();
    let base = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for cand in [
        base.join(name),
        base.join("gpg").join("bin").join(name),
        base.join("bin").join(name),
    ] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// 定位 gpg：PATH → 程序目录。
pub fn find_gpg() -> Option<PathBuf> {
    find_gpg_in_path().or_else(find_gpg_in_program_dir)
}

/// 确保存在可用 gpg；找不到且为 Windows 时自动下载安装。
/// 返回（gpg 路径，来源）。
pub fn ensure_gpg(data_dir: &Path) -> Result<(PathBuf, GpgSource)> {
    if let Some(p) = find_gpg_in_path() {
        return Ok((p, GpgSource::Path));
    }
    if let Some(p) = find_gpg_in_program_dir() {
        return Ok((p, GpgSource::ProgramDir));
    }
    #[cfg(windows)]
    {
        let p = download_and_install(data_dir)?;
        return Ok((p, GpgSource::Downloaded));
    }
    #[cfg(not(windows))]
    Err(AcsError::message(
        "未找到 gpg，且当前平台不支持自动下载，请安装 GnuPG 或将 gpg 放入程序目录",
    ))
}

/// 下载 Gpg4win 并静默安装（Windows）。
#[cfg(windows)]
pub fn download_and_install(data_dir: &Path) -> Result<PathBuf> {
    let url = std::env::var("ACS_GPG_DOWNLOAD_URL").unwrap_or_else(|_| GPG4WIN_URL.to_string());
    let installer_name = url.rsplit('/').next().unwrap_or("gpg4win.exe");
    let dl_dir = data_dir.join("downloads");
    fs::create_dir_all(&dl_dir)?;
    let installer = dl_dir.join(installer_name);

    if !installer.is_file() {
        println!("[gpg] 未找到 gpg，正在下载 GnuPG 安装包（{url}）...");
        download(&url, &installer)?;
        println!("[gpg] 下载完成：{}", installer.display());
    }

    // 安装目录：优先程序目录（随发行包），不可写则退回 data_dir/gnupg
    let install_dir = match std::env::current_exe() {
        Ok(cur) => {
            let pd = cur.parent().unwrap_or(data_dir).to_path_buf();
            let cand = pd.join("gnupg");
            if is_writable(&pd) { cand } else { data_dir.join("gnupg") }
        }
        Err(_) => data_dir.join("gnupg"),
    };
    fs::create_dir_all(&install_dir)?;

    println!("[gpg] 静默安装 GnuPG 到 {} ...", install_dir.display());
    let status = std::process::Command::new(&installer)
        .args(["/S", &format!("/D={}", install_dir.display())])
        .status()
        .map_err(|e| AcsError::message(format!("执行安装程序失败: {e}")))?;
    if !status.success() {
        return Err(AcsError::message("Gpg4win 静默安装失败"));
    }

    for cand in [install_dir.join("bin").join("gpg.exe"), install_dir.join("gpg.exe")] {
        if cand.is_file() {
            println!("[gpg] 安装完成：{}", cand.display());
            return Ok(cand);
        }
    }
    Err(AcsError::message(format!(
        "安装后未找到 gpg.exe（位于 {}）",
        install_dir.display()
    )))
}

#[cfg(windows)]
fn download(url: &str, dest: &Path) -> Result<()> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| AcsError::message(format!("下载失败: {e}")))?;
    let mut body = Vec::new();
    resp.into_reader()
        .take(300 * 1024 * 1024)
        .read_to_end(&mut body)
        .map_err(AcsError::Io)?;
    fs::write(dest, body)?;
    Ok(())
}

#[cfg(windows)]
fn is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".acs_write_test");
    let ok = fs::write(&probe, b"").is_ok();
    if ok {
        let _ = fs::remove_file(&probe);
    }
    ok
}
