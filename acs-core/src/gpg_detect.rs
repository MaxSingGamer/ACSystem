//! gpg.exe 定位。
//!
//! 查找优先级：**PATH → 程序目录（随发行包附带）**。
//! 程序目录兼容：`<exe 目录>/gpg.exe`、`<exe 目录>/gpg/bin/gpg.exe`、`<exe 目录>/bin/gpg.exe`。
//! 发行包需将 gpg.exe 与其依赖 DLL（Gpg4win bin 目录）一并放入上述位置，不再自动下载。

use std::path::PathBuf;

use crate::errors::{AcsError, Result};

/// gpg 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpgSource {
    Path,
    ProgramDir,
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

/// 确保存在可用 gpg；找不到时报错并提示将 gpg 放入程序目录。
/// 返回（gpg 路径，来源）。
pub fn ensure_gpg() -> Result<(PathBuf, GpgSource)> {
    if let Some(p) = find_gpg_in_path() {
        return Ok((p, GpgSource::Path));
    }
    if let Some(p) = find_gpg_in_program_dir() {
        return Ok((p, GpgSource::ProgramDir));
    }
    Err(AcsError::message(
        "未找到 gpg：请将 gpg.exe 与其依赖 DLL 放入程序目录（gpg/bin/ 或 bin/），或安装 GnuPG 并加入 PATH",
    ))
}
