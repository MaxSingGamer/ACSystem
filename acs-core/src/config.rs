//! acs-core 通用配置（server / client / mirror 共用）。

use std::fs;
use std::path::{PathBuf, Path};

use crate::errors::Result;

#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// gpg.exe 路径（随安装包放入 acs 程序目录）。
    pub gpg_bin: PathBuf,
    /// gpg homedir（隔离，便于技术人员 `gpg --homedir <dir> --list-keys` 审查）。
    pub gpg_homedir: PathBuf,
    /// 数据目录：客户端 `~/.alpha_dir`，服务端为各自数据目录。
    pub data_dir: PathBuf,
    /// SQLite 数据库文件路径。
    pub db_path: PathBuf,
    /// 中心地址（client/mirror 使用）。
    pub server_url: Option<String>,
    /// 镜像 apikey（mirror 使用）。
    pub mirror_apikey: Option<String>,
}

impl CoreConfig {
    /// 默认目录 `~/.alpha_dir`（用户主目录下；可用环境变量 `ACS_ALPHA_DIR` 覆盖）。
    pub fn default_alpha_dir() -> PathBuf {
        if let Ok(d) = std::env::var("ACS_ALPHA_DIR") {
            if !d.trim().is_empty() {
                return PathBuf::from(d);
            }
        }
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".alpha_dir")
    }

    /// 客户端默认配置（数据统一存放：~/.alpha_dir/acs-client）。
    pub fn client_default() -> CoreConfig {
        let dir = Self::default_alpha_dir().join("acs-client");
        CoreConfig {
            gpg_bin: PathBuf::from("gpg.exe"),
            gpg_homedir: dir.join("gnupg"),
            data_dir: dir.clone(),
            db_path: dir.join("alpha.db"),
            server_url: None,
            mirror_apikey: None,
        }
    }

    /// 服务端默认配置。
    pub fn server_default(data_dir: impl Into<PathBuf>) -> CoreConfig {
        let dir = data_dir.into();
        CoreConfig {
            gpg_bin: PathBuf::from("gpg.exe"),
            // 与客户端（~/.alpha_dir/acs-client/gnupg）隔离，避免同机共用冲突
            gpg_homedir: dir.join("gnupg-server"),
            data_dir: dir.clone(),
            db_path: dir.join("alpha_center.db"),
            server_url: None,
            mirror_apikey: None,
        }
    }

    /// 确保数据目录与 gpg homedir 存在。
    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.data_dir)?;
        fs::create_dir_all(&self.gpg_homedir)?;
        Ok(())
    }
    /// gpg 是否可用。
    pub fn gpg_available(&self) -> bool {
        Path::new(&self.gpg_bin).exists()
    }
}

/// 把「各端数据目录」下 `.env` 的内容**载入进程环境变量**。
///
/// 这是让 `.env` 真正成为唯一配置入口的关键：所有读取都走环境变量
/// （端口、品牌、迭代次数、更新源……），而 `.env` 则在启动最早期注入。
///
/// 规则：
///  - 行格式 `KEY=VALUE`；忽略空行与 `#` 注释；兼容 `export KEY=VALUE`；
///  - 值首尾的引号（单/双）会被去掉；
///  - **已存在的进程环境变量优先，不覆盖**，便于临时用真实环境变量覆盖 `.env`；
///  - 文件不存在或无法读取时静默跳过（`.env` 属可选）。
///
/// 返回实际写入的键数量。
///
/// # Safety / 调用时机
/// `std::env::set_var` 在 Rust 2024 下是 `unsafe`：它要求调用时**没有其他线程
/// 在读环境变量**。因此本函数必须在进程启动的最早期（初始化线程池 / tokio runtime
/// 之前）调用；acs-server 在 `main` 第一段、acs-client 在打开钱包时调用。
pub fn load_env_file(dir: &Path) -> usize {
    let path = dir.join(".env");
    let Ok(text) = fs::read_to_string(&path) else {
        return 0;
    };
    let mut n = 0usize;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if std::env::var_os(k).is_some() {
            continue; // 真实环境变量优先
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v);
        // SAFETY: 调用约定见上文「调用时机」——启动早期、单线程。
        unsafe { std::env::set_var(k, v) };
        n += 1;
    }
    n
}
