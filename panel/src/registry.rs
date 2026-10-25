//! 面板配置与服务中心注册表（v3.3.0 #13 / D20 / D25 / D37）。
//!
//! 文件（均在 `~/.alphacoinserver/` 下）：
//! - `.env`        面板自身配置：`ACS_PANEL_DOMAIN`（仅记录）+ `ACS_PANEL_PORT`（默认 9600）
//! - `servers.json` 已注册的服务清单（`name/domain/api_port/admin_port/dir`）
//!
//! 规则：
//! - **服务名称唯一**、**域名唯一**、**API/管理端口各自唯一**，且都不得与面板冲突；
//! - 服务端口**没有默认值**（D25），新建/导入时必须显式填写；
//! - 新建 / 导入后**只允许改域名与端口**；改域名要同步「文件夹名 + 注册表 + 该服务 .env」。

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use acs_core::config;

/// 服务包/服务配置的「勿手改」提示（第 8 条）。
pub const NO_EDIT_HINT: &str = "⚠️ 请勿手动修改本文件（由 AlphaCoinServerPanel 自动维护）";

// ============================ 面板配置 ============================

#[derive(Debug, Clone)]
pub struct PanelConfig {
    pub domain: String,
    pub port: u16,
    /// 配置是否已由用户在面板内确认过（首次启动为 false → UI 引导）
    pub configured: bool,
}

impl Default for PanelConfig {
    fn default() -> Self {
        PanelConfig { domain: String::new(), port: 9600, configured: false }
    }
}

pub fn panel_env_path() -> PathBuf {
    config::server_root().join(".env")
}

pub fn server_root() -> PathBuf {
    config::server_root()
}

/// 读取面板配置（文件不存在 → 默认 9600 / 未配置）。
pub fn load_panel_config() -> PanelConfig {
    let mut cfg = PanelConfig::default();
    let Ok(text) = fs::read_to_string(panel_env_path()) else {
        return cfg;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "ACS_PANEL_DOMAIN" => {
                    cfg.domain = v.trim().to_string();
                }
                "ACS_PANEL_PORT" => {
                    if let Ok(p) = v.trim().parse::<u16>() {
                        if p > 0 {
                            cfg.port = p;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // 「已配置」= 域名已填（首次启动会提示配置域名与端口）
    cfg.configured = !cfg.domain.is_empty();
    cfg
}

/// 写入面板配置（带勿手改提示）。
pub fn save_panel_config(domain: &str, port: u16) -> Result<()> {
    if port == 0 {
        bail!("端口必须为 1–65535");
    }
    let root = server_root();
    fs::create_dir_all(&root)?;
    let domain = domain.trim();
    let text = format!(
        "# {NO_EDIT_HINT}\n\
         # 面板域名：仅作标识/记录，不参与任何网络绑定（不要求可解析）\n\
         ACS_PANEL_DOMAIN={domain}\n\
         # 面板 Web 监听端口（默认 9600；修改后需重启面板）\n\
         ACS_PANEL_PORT={port}\n\
         # 面板仅监听本机（D40）：如需公网访问请用反向代理并自行加认证\n\
         ACS_PANEL_BIND=127.0.0.1\n"
    );
    fs::write(panel_env_path(), text).context("写入面板 .env 失败")?;
    Ok(())
}

/// 面板绑定地址（安全默认：仅本机）。
pub fn panel_bind() -> String {
    let Ok(text) = fs::read_to_string(panel_env_path()) else {
        return "127.0.0.1".into();
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "ACS_PANEL_BIND" && !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    "127.0.0.1".into()
}

// ============================ 服务注册表 ============================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceRec {
    pub name: String,
    pub domain: String,
    pub api_port: u16,
    pub admin_port: u16,
    /// 相对 `~/.alphacoinserver/` 的目录名（通常等于 domain）
    pub dir: String,
    #[serde(default)]
    pub created_at: i64,
    /// 面板启动的服务进程 PID（运行态提示；退出后可能失效，以端口探测为准）
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    #[serde(rename = "_comment")]
    pub comment: String,
    #[serde(default)]
    pub servers: Vec<ServiceRec>,
}

pub fn registry_path() -> PathBuf {
    server_root().join("servers.json")
}

pub fn load_registry() -> Result<Registry> {
    let p = registry_path();
    if !p.exists() {
        return Ok(Registry {
            comment: NO_EDIT_HINT.to_string(),
            servers: Vec::new(),
        });
    }
    let text = fs::read_to_string(&p).with_context(|| format!("读取 {} 失败", p.display()))?;
    let mut r: Registry = serde_json::from_str(&text)
        .with_context(|| format!("{} 解析失败（可能被手动改坏）", p.display()))?;
    r.comment = NO_EDIT_HINT.to_string();
    Ok(r)
}

pub fn save_registry(reg: &Registry) -> Result<()> {
    let root = server_root();
    fs::create_dir_all(&root)?;
    let mut reg = reg.clone();
    reg.comment = NO_EDIT_HINT.to_string();
    let text = serde_json::to_string_pretty(&reg)?;
    // 原子写：先写临时文件再替换，避免写一半导致注册表损坏
    let tmp = registry_path().with_extension("json.tmp");
    fs::write(&tmp, text).context("写入 servers.json 失败")?;
    fs::rename(&tmp, registry_path()).context("替换 servers.json 失败")?;
    Ok(())
}

/// 校验一个候选服务记录是否与「面板 + 其他服务」冲突。
/// `skip_name`：更新时跳过自身（按 name 匹配）。
pub fn validate_no_conflict(reg: &Registry, cand: &ServiceRec, skip_name: Option<&str>) -> Result<()> {
    let panel = load_panel_config();
    if cand.name.trim().is_empty() {
        bail!("服务名称不能为空");
    }
    if cand.domain.trim().is_empty() {
        bail!("域名不能为空");
    }
    if cand.api_port == 0 || cand.admin_port == 0 {
        bail!("端口必须为 1–65535，且不能为 0");
    }
    if cand.api_port == cand.admin_port {
        bail!("API 端口与管理端口不能相同");
    }
    // 面板冲突
    if !panel.domain.trim().is_empty() && panel.domain.trim() == cand.domain.trim() {
        bail!("域名与面板冲突：{}", cand.domain);
    }
    if cand.api_port == panel.port || cand.admin_port == panel.port {
        bail!("端口与面板端口 {} 冲突", panel.port);
    }
    for s in &reg.servers {
        if Some(s.name.as_str()) == skip_name {
            continue;
        }
        if s.name.eq_ignore_ascii_case(cand.name.trim()) {
            bail!("服务名称已被占用：{}", cand.name);
        }
        if s.domain.eq_ignore_ascii_case(cand.domain.trim()) {
            bail!("域名已被占用：{}", cand.domain);
        }
        if s.api_port == cand.api_port {
            bail!("API 端口已被其他服务占用：{}", cand.api_port);
        }
        if s.admin_port == cand.admin_port {
            bail!("管理端口已被其他服务占用：{}", cand.admin_port);
        }
    }
    Ok(())
}

pub fn find_service<'a>(reg: &'a Registry, name: &str) -> Option<&'a ServiceRec> {
    reg.servers.iter().find(|s| s.name == name)
}

/// 服务数据目录（绝对路径）。
pub fn service_dir(rec: &ServiceRec) -> PathBuf {
    server_root().join(&rec.dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, domain: &str, api: u16, admin: u16) -> ServiceRec {
        ServiceRec {
            name: name.into(),
            domain: domain.into(),
            api_port: api,
            admin_port: admin,
            dir: domain.into(),
            created_at: 0,
            pid: None,
        }
    }

    #[test]
    fn conflicts() {
        let reg = Registry {
            comment: String::new(),
            servers: vec![rec("A", "a.test", 9700, 9701)],
        };
        assert!(validate_no_conflict(&reg, &rec("B", "b.test", 9702, 9703), None).is_ok());
        assert!(validate_no_conflict(&reg, &rec("A", "b.test", 9702, 9703), None).is_err(), "重名");
        assert!(
            validate_no_conflict(&reg, &rec("B", "a.test", 9702, 9703), None).is_err(),
            "域名重复"
        );
        assert!(validate_no_conflict(&reg, &rec("B", "b.test", 9700, 9703), None).is_err(), "API 端口重复");
        assert!(
            validate_no_conflict(&reg, &rec("B", "b.test", 9702, 9701), None).is_err(),
            "管理端口重复"
        );
        // 更新自身时跳过
        assert!(validate_no_conflict(&reg, &rec("A", "a.test", 9700, 9701), Some("A")).is_ok());
    }

    #[test]
    fn rejects_same_ports_and_zero() {
        let reg = Registry::default();
        assert!(validate_no_conflict(&reg, &rec("A", "a.test", 9700, 9700), None).is_err());
        assert!(validate_no_conflict(&reg, &rec("A", "a.test", 0, 9701), None).is_err());
        assert!(validate_no_conflict(&reg, &rec("", "a.test", 9700, 9701), None).is_err());
    }
}
