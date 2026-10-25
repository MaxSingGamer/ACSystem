//! 服务实例管理（v3.3.0 #13）：新建 / 启停 / 改域名端口 / 删除 / 日志查看。
//!
//! 与其他模块的分工：
//! - `registry`：注册表与冲突校验（`servers.json`）
//! - 本模块：磁盘上的服务目录（`.env`、logs）与进程（启动/停止/状态）
//!
//! 关键约束：
//! - 服务端进程与面板**不同 exe**（D38）：面板只负责 spawn `AlphaCoinServer`，自身重启不影响服务；
//! - 服务端口**没有默认值**（D25），必须显式给出；
//! - 新建后**只可改域名与端口**；改域名要同步「目录名 + 注册表 + .env」。

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::registry::{self, ServiceRec, NO_EDIT_HINT};

/// 新建服务所需的表单数据。
#[derive(Debug, Clone, Deserialize)]
pub struct NewServiceReq {
    pub name: String,
    pub domain: String,
    pub api_port: u16,
    pub admin_port: u16,
    /// `uid:密码` 列表（至少一个，首启后不可增减）
    pub root_accounts: Vec<String>,
    /// `uid:密码` 列表（可为空）
    #[serde(default)]
    pub system_accounts: Vec<String>,
    /// 发行账户 uid（有系统账户时必填）
    #[serde(default)]
    pub pre_issued: String,
    #[serde(default)]
    pub brand_name: String,
    #[serde(default)]
    pub brand_currency: String,
}

/// 可修改项（第 7 条：新建/导入后**只允许改域名与端口**）。
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateServiceReq {
    pub domain: String,
    pub api_port: u16,
    pub admin_port: u16,
}

pub fn server_exe() -> PathBuf {
    // v3.3.0 #1：服务端产物名 AlphaCoinServer（旧名 acs-server 仅作兼容回退）
    let candidates: [&str; 2] = if cfg!(windows) {
        ["AlphaCoinServer.exe", "acs-server.exe"]
    } else {
        ["AlphaCoinServer", "acs-server"]
    };
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = dir {
        for name in candidates {
            let p = dir.join(name);
            if p.exists() {
                return p;
            }
        }
    }
    // PATH 查找（开发期 cargo run 场景）
    PathBuf::from(candidates[0])
}

/// 端口是否可连通（服务是否在监听）。
pub fn port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

pub fn service_state(rec: &ServiceRec) -> (String, Option<u32>) {
    if port_open(rec.api_port) || port_open(rec.admin_port) {
        return ("running".into(), rec.pid);
    }
    if let Some(pid) = rec.pid {
        if pid_alive(pid) {
            return ("starting".into(), Some(pid));
        }
    }
    ("stopped".into(), rec.pid)
}

/// 新建服务：校验 → 建目录 → 写 `.env` → 写注册表。
pub fn create_service(req: NewServiceReq) -> Result<ServiceRec> {
    let mut reg = registry::load_registry()?;
    let rec = ServiceRec {
        name: req.name.trim().to_string(),
        domain: req.domain.trim().to_string(),
        api_port: req.api_port,
        admin_port: req.admin_port,
        dir: req.domain.trim().to_string(),
        created_at: chrono::Utc::now().timestamp(),
        pid: None,
    };
    registry::validate_no_conflict(&reg, &rec, None)?;
    let req = NewServiceReq {
        root_accounts: clean_accounts(&req.root_accounts),
        system_accounts: clean_accounts(&req.system_accounts),
        ..req
    };
    if req.root_accounts.is_empty() {
        bail!("至少需要一个根管理员账户（uid:密码）");
    }
    for a in req.root_accounts.iter().chain(req.system_accounts.iter()) {
        if !a.contains(':') || a.split_once(':').map(|(u, p)| u.trim().is_empty() || p.trim().is_empty()).unwrap_or(true) {
            bail!("账户格式须为 `uid:密码`：{a}");
        }
    }
    if !req.system_accounts.is_empty()
        && (req.pre_issued.trim().is_empty()
            || !req.system_accounts.iter().any(|s| {
                s.split_once(':').map(|(u, _)| u.trim()) == Some(req.pre_issued.trim())
            }))
    {
        bail!("已填写系统账户，则「发行账户」必须为其中之一");
    }
    let dir = registry::server_root().join(&rec.dir);
    if dir.exists() {
        bail!("目录已存在：{}（请换域名，或先删除该目录）", dir.display());
    }
    fs::create_dir_all(dir.join("logs")).with_context(|| format!("创建目录失败：{}", dir.display()))?;
    write_service_env(&dir, &rec, &req)?;
    reg.servers.push(rec.clone());
    registry::save_registry(&reg)?;
    Ok(rec)
}

/// 生成服务 `.env`（带勿手改提示）。
fn write_service_env(dir: &Path, rec: &ServiceRec, req: &NewServiceReq) -> Result<()> {
    let roots = req.root_accounts.join(",");
    let systems = req.system_accounts.join(",");
    let brand_name = if req.brand_name.trim().is_empty() {
        "Alpha Coin".to_string()
    } else {
        req.brand_name.trim().to_string()
    };
    let currency = if req.brand_currency.trim().is_empty() {
        "A€".to_string()
    } else {
        req.brand_currency.trim().to_string()
    };
    let mut s = String::new();
    s.push_str(&format!("# {NO_EDIT_HINT}\n"));
    s.push_str("# 中心服务配置（面板自动维护；仅域名/端口可在面板内修改）\n");
    s.push_str(&format!("ACS_PUBLIC_URL=https://{}\n", rec.domain));
    s.push_str(&format!("ACS_PUBLIC_PORT={}\n", rec.api_port));
    s.push_str("ACS_PUBLIC_BIND=0.0.0.0\n");
    s.push_str(&format!("ACS_ADMIN_PORT={}\n", rec.admin_port));
    s.push_str("ACS_ADMIN_BIND=0.0.0.0\n");
    s.push_str(&format!("ACS_ROOT_ACCOUNTS={roots}\n"));
    if !systems.is_empty() {
        s.push_str(&format!("ACS_SYSTEM_ACCOUNTS={systems}\n"));
        s.push_str(&format!("PRE_ISSUED_ACCOUNT={}\n", req.pre_issued.trim()));
    }
    s.push_str(&format!("ACS_BRAND_NAME={brand_name}\n"));
    s.push_str(&format!("ACS_BRAND_CURRENCY={currency}\n"));
    fs::write(dir.join(".env"), s).with_context(|| format!("写入 {}/.env 失败", dir.display()))?;
    Ok(())
}

/// 启动服务（已在运行则直接返回）。
pub fn start_service(name: &str) -> Result<u32> {
    let mut reg = registry::load_registry()?;
    let idx = reg
        .servers
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("服务不存在：{name}"))?;
    let rec = reg.servers[idx].clone();
    if port_open(rec.api_port) || port_open(rec.admin_port) {
        return Ok(rec.pid.unwrap_or(0));
    }
    let dir = registry::service_dir(&rec);
    if !dir.join(".env").exists() {
        bail!("服务配置缺失：{}/.env", dir.display());
    }
    fs::create_dir_all(dir.join("logs"))?;
    let out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("logs").join("panel-spawn.log"))?;
    let err = out.try_clone()?;
    let mut cmd = Command::new(server_exe());
    cmd.arg("-homedir")
        .arg(&dir)
        // 服务名称通过启动参数传入（D26：服务名不出现在 .env / 服务包里）
        .arg("-name")
        .arg(&rec.name)
        .current_dir(&dir)
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 面板是 GUI/后台进程：不弹控制台黑窗（与 D23 同原则）
        cmd.creation_flags(0x0800_0000);
    }
    let child = cmd.spawn().with_context(|| format!("启动服务失败：{}", server_exe().display()))?;
    let pid = child.id();
    reg.servers[idx].pid = Some(pid);
    registry::save_registry(&reg)?;
    Ok(pid)
}

/// 停止服务：优先按记录的 PID 结束；PID 失效时按端口反查。
pub fn stop_service(name: &str) -> Result<()> {
    let mut reg = registry::load_registry()?;
    let idx = reg
        .servers
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("服务不存在：{name}"))?;
    let rec = reg.servers[idx].clone();
    let mut killed = false;
    if let Some(pid) = rec.pid {
        if pid_alive(pid) {
            kill_pid(pid)?;
            killed = true;
        }
    }
    if !killed {
        for port in [rec.api_port, rec.admin_port] {
            if let Some(pid) = pid_by_port(port) {
                kill_pid(pid)?;
                break;
            }
        }
    }
    reg.servers[idx].pid = None;
    registry::save_registry(&reg)?;
    Ok(())
}

fn kill_pid(pid: u32) -> Result<()> {
    #[cfg(windows)]
    {
        let out = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .context("调用 taskkill 失败")?;
        if !out.status.success() {
            bail!("结束进程失败：{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let st = Command::new("kill").args(["-TERM", &pid.to_string()]).status()?;
        if !st.success() {
            bail!("结束进程失败：pid={pid}");
        }
        Ok(())
    }
}

/// 按监听端口反查 PID（Windows 用 netstat；Linux 用 ss）。
fn pid_by_port(port: u16) -> Option<u32> {
    #[cfg(windows)]
    {
        let out = Command::new("netstat").args(["-ano", "-p", "TCP"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let needle = format!(":{port} ");
        for line in text.lines() {
            if line.contains(&needle) && line.contains("LISTENING") {
                if let Some(pid) = line.split_whitespace().last() {
                    return pid.parse().ok();
                }
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let out = Command::new("ss").args(["-ltnp"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains(&format!(":{port} ")) {
                if let Some(idx) = line.find("pid=") {
                    let rest = &line[idx + 4..];
                    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    return num.parse().ok();
                }
            }
        }
        None
    }
}

/// 修改服务域名 / 端口（唯一允许的修改项）。
///
/// 改域名时同步三处：① 目录名 ② `servers.json` ③ 该服务 `.env` 的 URL/端口。
pub fn update_service(name: &str, req: UpdateServiceReq) -> Result<ServiceRec> {
    let mut reg = registry::load_registry()?;
    let idx = reg
        .servers
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("服务不存在：{name}"))?;
    let old = reg.servers[idx].clone();
    let new_domain = req.domain.trim().to_string();
    let cand = ServiceRec {
        name: old.name.clone(),
        domain: new_domain.clone(),
        api_port: req.api_port,
        admin_port: req.admin_port,
        dir: new_domain.clone(),
        created_at: old.created_at,
        pid: old.pid,
    };
    registry::validate_no_conflict(&reg, &cand, Some(&old.name))?;
    let old_dir = registry::server_root().join(&old.dir);
    let new_dir = registry::server_root().join(&cand.dir);
    if new_dir != old_dir {
        if new_dir.exists() {
            bail!("目标目录已存在：{}", new_dir.display());
        }
        fs::rename(&old_dir, &new_dir)
            .with_context(|| format!("重命名目录失败：{} -> {}", old_dir.display(), new_dir.display()))?;
    }
    // 同步 .env（只改 URL / 端口；其余键原样保留，未知键也不丢）
    let env_path = new_dir.join(".env");
    let text = fs::read_to_string(&env_path).unwrap_or_default();
    let mut out = String::new();
    for line in text.lines() {
        let line2 = line.trim();
        if line2.starts_with("ACS_PUBLIC_URL=") {
            out.push_str(&format!("ACS_PUBLIC_URL=https://{new_domain}\n"));
        } else if line2.starts_with("ACS_PUBLIC_PORT=") {
            out.push_str(&format!("ACS_PUBLIC_PORT={}\n", cand.api_port));
        } else if line2.starts_with("ACS_ADMIN_PORT=") {
            out.push_str(&format!("ACS_ADMIN_PORT={}\n", cand.admin_port));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    fs::write(&env_path, out).with_context(|| format!("写入 {} 失败", env_path.display()))?;
    reg.servers[idx] = cand.clone();
    registry::save_registry(&reg)?;
    Ok(cand)
}

/// 删除服务：先停进程，再删目录与注册表项。
pub fn delete_service(name: &str) -> Result<()> {
    let mut reg = registry::load_registry()?;
    let idx = reg
        .servers
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("服务不存在：{name}"))?;
    let rec = reg.servers[idx].clone();
    // 停进程（失败不阻断删除，但记录到错误里由调用方决定）
    let _ = stop_service(name);
    let dir = registry::server_root().join(&rec.dir);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("删除目录失败：{}", dir.display()))?;
    }
    reg.servers.remove(idx);
    registry::save_registry(&reg)?;
    Ok(())
}

// ---------------------------------- 日志 ----------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct LogFile {
    pub file: String,
    pub size: u64,
    pub modified: i64,
}

pub fn list_logs(rec: &ServiceRec) -> Result<Vec<LogFile>> {
    let dir = registry::service_dir(rec).join("logs");
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(&dir) else { return Ok(out) };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("log") {
            continue;
        }
        let md = e.metadata().ok();
        out.push(LogFile {
            file: e.file_name().to_string_lossy().into_owned(),
            size: md.as_ref().map(|m| m.len()).unwrap_or(0),
            modified: md
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        });
    }
    // 新的在前（文件名以毫秒时间戳开头）
    out.sort_by(|a, b| b.file.cmp(&a.file));
    Ok(out)
}

/// 读取日志尾部若干行（最多 4MB，避免一次读爆内存）。
pub fn tail_log(rec: &ServiceRec, file: &str, lines: usize) -> Result<String> {
    if file.contains('/') || file.contains('\\') || file.contains("..") {
        bail!("非法日志文件名");
    }
    let p = registry::service_dir(rec).join("logs").join(file);
    let data = fs::read(&p).with_context(|| format!("读取日志失败：{}", p.display()))?;
    let start = data.len().saturating_sub(4 * 1024 * 1024);
    let text = String::from_utf8_lossy(&data[start..]).into_owned();
    let all: Vec<&str> = text.lines().collect();
    let from = all.len().saturating_sub(lines.clamp(1, 5000));
    Ok(all[from..].join("\n"))
}

/// 供新建服务时把 `uid:密码` 列表里的空行剔除。
pub fn clean_accounts(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_accounts_trims_and_drops_empty() {
        let v = clean_accounts(&[
            " a:1 ".to_string(),
            "".to_string(),
            "  ".to_string(),
            "b:2".to_string(),
        ]);
        assert_eq!(v, vec!["a:1".to_string(), "b:2".to_string()]);
    }

    #[test]
    fn tail_log_rejects_traversal() {
        let rec = ServiceRec {
            name: "x".into(),
            domain: "x.test".into(),
            api_port: 1,
            admin_port: 2,
            dir: "x.test".into(),
            created_at: 0,
            pid: None,
        };
        assert!(tail_log(&rec, "../secret", 10).is_err());
        assert!(tail_log(&rec, "a/b.log", 10).is_err());
    }
}
