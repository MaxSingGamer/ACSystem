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
    /// 数据目录：服务端 `~/.alphacoinserver/<服务域名>`，客户端 `~/.alphawallet/<中心域名>`（v3.3.0 #9）。
    pub data_dir: PathBuf,
    /// SQLite 数据库文件路径。
    pub db_path: PathBuf,
    /// 中心地址（client/mirror 使用）。
    pub server_url: Option<String>,
    /// 镜像 apikey（mirror 使用）。
    pub mirror_apikey: Option<String>,
}

impl CoreConfig {
    /// 客户端默认配置（v3.3.0 #9：数据按「中心域名」分子目录，见 [`crate::config::client_domain_dir`]）。
    /// 注意：本函数只用于「尚未确定中心域名」时的兜底路径；正常打开钱包走 `wallet::Wallet::open()`。
    pub fn client_default() -> CoreConfig {
        let dir = crate::config::wallet_root().join("_default");
        CoreConfig {
            gpg_bin: PathBuf::from("gpg.exe"),
            gpg_homedir: dir.join("gnupg"),
            data_dir: dir.clone(),
            db_path: dir.join("wallet.db"),
            server_url: None,
            mirror_apikey: None,
        }
    }

    /// 服务端默认配置（数据目录由调用方决定：`-homedir` / `ACS_DATA_DIR` / `~/.alphacoinserver/<域名>`）。
    pub fn server_default(data_dir: impl Into<PathBuf>) -> CoreConfig {
        let dir = data_dir.into();
        CoreConfig {
            gpg_bin: PathBuf::from("gpg.exe"),
            // 与客户端（~/.alphawallet/<域名>/gnupg）隔离，避免同机共用冲突
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

// ============================================================================
//  v3.3.0（#9 / D33 / D34）：数据目录布局
//
//  服务端面板： ~/.alphacoinserver/            （servers.json、面板 .env；见 §13）
//                  └─ <服务域名>/              （该服务的 .env、db、gnupg、logs/）
//  客户端钱包： ~/.alphawallet/                （central_server_history.db —— 所有中心的历史）
//                  └─ <中心域名>/              （该中心的 wallet.db、gnupg、logs/）
//  企业客户端： ~/.alphabankmanage/             （结构与钱包相同）
//
//  三个根目录名 **固定不可改名**（D33）；仅测试/并行验证可用 `ACS_HOME_BASE` 改变 home 基准。
// ============================================================================

/// 用户主目录（Windows: `USERPROFILE`；*nix: `HOME`）。
pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// home 基准目录：生产环境 = 用户主目录。
///
/// `ACS_HOME_BASE` **仅用于测试 / 多套并行验证**（把三个 `~/.alpha*` 放到临时目录下）。
/// 它只改「home 基准」，**不提供**给三个根目录改名的入口（D33）。
pub fn home_base() -> PathBuf {
    if let Some(d) = std::env::var_os("ACS_HOME_BASE") {
        let p = PathBuf::from(d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    home_dir()
}

/// 服务端面板根目录（固定 `~/.alphacoinserver`）。
pub fn server_root() -> PathBuf {
    home_base().join(".alphacoinserver")
}

/// 客户端钱包根目录（固定 `~/.alphawallet`）。
pub fn wallet_root() -> PathBuf {
    home_base().join(".alphawallet")
}

/// 企业管理客户端根目录（固定 `~/.alphabankmanage`）。
pub fn bankmanage_root() -> PathBuf {
    home_base().join(".alphabankmanage")
}

/// 客户端「某中心」的目录：`<root>/<domain_key>`。
pub fn client_domain_dir(root: &Path, domain_key: &str) -> PathBuf {
    root.join(if domain_key.is_empty() { "_default" } else { domain_key })
}

/// 中心地址 → **目录键**：`host[ _port ]`，仅保留文件系统安全字符。
///
/// 例：`http://127.0.0.1:9699` → `127.0.0.1_9699`；
///     `https://acsystem.maxshin.top/` → `acsystem.maxshin.top`。
pub fn domain_key(url: &str) -> String {
    let u = url.trim();
    let rest = u
        .strip_prefix("http://")
        .or_else(|| u.strip_prefix("https://"))
        .unwrap_or(u);
    let hostport = rest
        .split('/')
        .next()
        .unwrap_or(rest)
        .rsplit('@')
        .next()
        .unwrap_or(rest);
    let mut s = String::with_capacity(hostport.len());
    for c in hostport.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            s.push(c);
        } else {
            s.push('_');
        }
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "default".to_string()
    } else {
        s
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
