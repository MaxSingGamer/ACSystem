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

    /// 客户端默认配置。
    pub fn client_default() -> CoreConfig {
        let dir = Self::default_alpha_dir();
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
            // 与客户端（~/.alpha_dir/gnupg）隔离，避免同机共用冲突
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
