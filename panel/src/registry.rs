//! 面板配置与服务中心注册表（v4.0.0：服务域名拆分为「服务接口域名 / 后台管理域名」）。
//!
//! 文件（均在 `~/.alphacoinserver/` 下）：
//! - `.env`            面板自身配置：`ACS_PANEL_DOMAIN`（面板域名，仅记录）+ `ACS_PANEL_PORT`（默认 9600）
//! - `panel-account.json` 面板登录账户（见 `auth.rs`）
//! - `servers.json`    已注册的服务清单
//!
//! 域名（v4.0.0 Panel #3）：
//! - **服务接口域名（api_domain）**：客户端 API 域名，**唯一识别符**，同时决定服务文件夹名；
//! - **后台管理域名（admin_domain）**：网页域名，仅用于把人带到该服务的后台登录页。
//!
//! 规则：
//! - **服务名称唯一**、**两个域名各自唯一**、**API/管理端口各自唯一**，且都不得与面板冲突；
//! - 服务端口**没有默认值**（D25），新建/导入时必须显式填写；
//! - 新建 / 导入后**只允许改域名与端口**；改「服务接口域名」要同步「文件夹名 + 注册表 + 该服务 .env」，
//!   且必须**先停止服务**（Windows 下运行中的进程会占用文件夹，导致改名失败）。

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
    /// 面板域名（仅记录/展示；顶栏会显示 `https://<域名>`）。
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
         # 面板域名：用于顶栏显示与 Passkey 的 rpId（不参与本进程网络绑定）\n\
         ACS_PANEL_DOMAIN={domain}\n\
         # 面板 Web 监听端口（默认 9600；修改后需重启面板）\n\
         ACS_PANEL_PORT={port}\n\
         # 面板仅监听本机（D40）：如需公网访问请用反向代理（按这里的域名）\n\
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

/// 面板对外地址（v4.0.0 Panel #6：顶栏显示设定的面板域名）。
pub fn panel_url(cfg: &PanelConfig) -> String {
    if cfg.domain.trim().is_empty() {
        format!("http://{}:{}", panel_bind(), cfg.port)
    } else {
        format!("https://{}", cfg.domain.trim())
    }
}

// ============================ 服务注册表 ============================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceRec {
    pub name: String,
    /// 服务接口域名（API）：唯一识别符，同时是服务文件夹名（见 `dir`）。
    #[serde(alias = "domain")]
    pub api_domain: String,
    /// 后台管理域名：网页入口，仅用于重定向到管理后台登录页。
    #[serde(default)]
    pub admin_domain: String,
    pub api_port: u16,
    pub admin_port: u16,
    /// 相对 `~/.alphacoinserver/` 的目录名（正常情况下等于 `api_domain`）。
    pub dir: String,
    #[serde(default)]
    pub created_at: i64,
    /// 面板启动的服务进程 PID（运行态提示；退出后可能失效，以端口探测为准）
    #[serde(default)]
    pub pid: Option<u32>,
}

impl ServiceRec {
    /// 文件夹名与服务接口域名是否不一致（v4.0.0 Panel #4：需要用户选择以哪一个为准）。
    pub fn dir_mismatch(&self) -> bool {
        self.dir != self.api_domain
    }
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
    // 兼容旧注册表：缺 admin_domain / dir 时补默认值
    for s in &mut r.servers {
        if s.admin_domain.trim().is_empty() {
            s.admin_domain = default_admin_domain(&s.api_domain);
        }
        if s.dir.trim().is_empty() {
            s.dir = s.api_domain.clone();
        }
    }
    Ok(r)
}

/// 未配置后台管理域名时的默认值：`admin.<服务接口域名>`。
pub fn default_admin_domain(api_domain: &str) -> String {
    if api_domain.trim().is_empty() {
        String::new()
    } else {
        format!("admin.{}", api_domain.trim())
    }
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

/// 合法的文件夹名：非空、不含路径分隔符与 `..`、不是绝对路径。
fn valid_dir_name(dir: &str) -> bool {
    !dir.trim().is_empty()
        && dir != "."
        && dir != ".."
        && !dir.contains('/')
        && !dir.contains('\\')
        && !dir.contains(':')
        && !dir.contains("..")
}

/// 校验一个候选服务记录是否与「面板 + 其他服务」冲突。
/// `skip_name`：更新时跳过自身（按 name 匹配）。
pub fn validate_no_conflict(reg: &Registry, cand: &ServiceRec, skip_name: Option<&str>) -> Result<()> {
    let panel = load_panel_config();
    if cand.name.trim().is_empty() {
        bail!("服务名称不能为空");
    }
    if cand.api_domain.trim().is_empty() {
        bail!("服务接口域名不能为空");
    }
    if cand.admin_domain.trim().is_empty() {
        bail!("后台管理域名不能为空");
    }
    if cand.api_domain.trim().eq_ignore_ascii_case(cand.admin_domain.trim()) {
        bail!("服务接口域名与后台管理域名不能相同");
    }
    if !valid_dir_name(&cand.dir) {
        bail!("文件夹名非法：{}", cand.dir);
    }
    if cand.api_port == 0 || cand.admin_port == 0 {
        bail!("端口必须为 1–65535，且不能为 0");
    }
    if cand.api_port == cand.admin_port {
        bail!("API 端口与管理端口不能相同");
    }
    // 面板冲突
    if !panel.domain.trim().is_empty() && panel.domain.trim().eq_ignore_ascii_case(cand.api_domain.trim()) {
        bail!("服务接口域名与面板域名冲突：{}", cand.api_domain);
    }
    if !panel.domain.trim().is_empty() && panel.domain.trim().eq_ignore_ascii_case(cand.admin_domain.trim()) {
        bail!("后台管理域名与面板域名冲突：{}", cand.admin_domain);
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
        if s.api_domain.eq_ignore_ascii_case(cand.api_domain.trim()) {
            bail!("服务接口域名已被占用：{}", cand.api_domain);
        }
        if s.admin_domain.eq_ignore_ascii_case(cand.admin_domain.trim()) {
            bail!("后台管理域名已被占用：{}", cand.admin_domain);
        }
        // 交叉冲突：某服务的接口域名不能等于别的服务的后台域名
        if s.admin_domain.eq_ignore_ascii_case(cand.api_domain.trim())
            || s.api_domain.eq_ignore_ascii_case(cand.admin_domain.trim())
        {
            bail!("域名与已有服务冲突：{} / {}", cand.api_domain, cand.admin_domain);
        }
        if s.dir.eq_ignore_ascii_case(&cand.dir) && s.name != cand.name {
            bail!("文件夹名已被其他服务占用：{}", cand.dir);
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

    fn rec(name: &str, api: &str, admin: &str, ap: u16, admp: u16) -> ServiceRec {
        ServiceRec {
            name: name.into(),
            api_domain: api.into(),
            admin_domain: admin.into(),
            api_port: ap,
            admin_port: admp,
            dir: api.into(),
            created_at: 0,
            pid: None,
        }
    }

    #[test]
    fn conflicts() {
        let reg = Registry {
            comment: String::new(),
            servers: vec![rec("A", "a.test", "admin.a.test", 9700, 9701)],
        };
        assert!(validate_no_conflict(&reg, &rec("B", "b.test", "admin.b.test", 9702, 9703), None).is_ok());
        assert!(validate_no_conflict(&reg, &rec("A", "b.test", "admin.b.test", 9702, 9703), None).is_err(), "重名");
        assert!(
            validate_no_conflict(&reg, &rec("B", "a.test", "admin.b.test", 9702, 9703), None).is_err(),
            "接口域名重复"
        );
        assert!(
            validate_no_conflict(&reg, &rec("B", "b.test", "admin.a.test", 9702, 9703), None).is_err(),
            "管理域名重复"
        );
        // 交叉冲突：B 的接口域名 == A 的管理域名
        assert!(
            validate_no_conflict(&reg, &rec("B", "admin.a.test", "admin.b.test", 9702, 9703), None).is_err(),
            "接口域名撞他人管理域名"
        );
        assert!(validate_no_conflict(&reg, &rec("B", "b.test", "admin.b.test", 9700, 9703), None).is_err(), "API 端口重复");
        assert!(
            validate_no_conflict(&reg, &rec("B", "b.test", "admin.b.test", 9702, 9701), None).is_err(),
            "管理端口重复"
        );
        // 更新自身时跳过
        assert!(
            validate_no_conflict(&reg, &rec("A", "a.test", "admin.a.test", 9700, 9701), Some("A")).is_ok()
        );
    }

    #[test]
    fn rejects_same_ports_zero_and_bad_dir() {
        let reg = Registry::default();
        assert!(validate_no_conflict(&reg, &rec("A", "a.test", "admin.a.test", 9700, 9700), None).is_err());
        assert!(validate_no_conflict(&reg, &rec("A", "a.test", "admin.a.test", 0, 9701), None).is_err());
        assert!(validate_no_conflict(&reg, &rec("", "a.test", "admin.a.test", 9700, 9701), None).is_err());
        // 接口域名 == 管理域名
        assert!(validate_no_conflict(&reg, &rec("A", "a.test", "a.test", 9700, 9701), None).is_err());
        // 非法文件夹名
        let mut bad = rec("A", "a.test", "admin.a.test", 9700, 9701);
        bad.dir = "../evil".into();
        assert!(validate_no_conflict(&reg, &bad, None).is_err());
    }

    #[test]
    fn dir_mismatch_flag() {
        let mut r = rec("A", "a.test", "admin.a.test", 9700, 9701);
        assert!(!r.dir_mismatch());
        r.dir = "old-folder".into();
        assert!(r.dir_mismatch());
    }

    #[test]
    fn default_admin_domain_is_prefixed() {
        assert_eq!(default_admin_domain("a.test"), "admin.a.test");
        assert_eq!(default_admin_domain(""), "");
    }
}
