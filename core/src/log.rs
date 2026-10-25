//! 统一文件日志（v3.3.0 #9 / D35：扩展名统一 `.log`，按中心域名分目录）。
//!
//! 输出格式：`时间 - [记录类型] 记录内容`
//! 记录类型：INFO / CALL / NET / OUT / ERR / IN
//! 隐私保护：口令、密钥、User 下用户名目录等一律以 `***` 打码后再写入。
//!
//! 目录约定：
//! - 服务端：`~/.alphacoinserver/<服务域名>/logs/`
//! - 客户端：`~/.alphawallet/<中心域名>/logs/`、`~/.alphabankmanage/<中心域名>/logs/`
//!
//! 轮转：每次启动新建 `{启动时间戳ms}.log`；初始化时清理最旧的，仅保留最近
//! `ACS_LOG_KEEP_FILES`（默认 30）个日志文件。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;

/// 进程内单例日志器。
static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

pub struct Logger {
    path: PathBuf,
    #[allow(dead_code)]
    started_at: i64,
}

/// 默认保留的日志文件数。
const DEFAULT_KEEP_FILES: usize = 30;

/// 初始化：在给定 **logs 目录**下新建 `{启动时间戳ms}.log`（每次启动一个文件）。
///
/// v3.3.0：参数从「数据目录」改为「日志目录」——因为日志按中心域名分目录，
/// 域名是运行时才知道的，由调用方显式传入 `<域名>/logs`。
pub fn init(logs_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(logs_dir).map_err(|e| e.to_string())?;
    // 轮转：先清理旧日志，再创建本次文件（避免把本次文件算进「待删」）
    let keep = std::env::var("ACS_LOG_KEEP_FILES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_KEEP_FILES);
    prune_old(logs_dir, keep);
    let started_at = chrono::Utc::now().timestamp_millis();
    let name = format!("{started_at}.log");
    let path = logs_dir.join(name);
    let mut l = LOGGER.lock().unwrap();
    *l = Some(Logger { path, started_at });
    Ok(())
}

/// 只保留最近 `keep` 个 `*.log`（文件名以毫秒时间戳开头，按名排序即按时间排序）。
fn prune_old(dir: &Path, keep: usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("log")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(false)
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    // 从最旧的开始删，直到只剩 keep 个
    let remove_n = files.len() - keep;
    for p in files.into_iter().take(remove_n) {
        let _ = fs::remove_file(p);
    }
}

/// 当前日志路径（供状态/诊断）。
pub fn path() -> Option<PathBuf> {
    LOGGER.lock().unwrap().as_ref().map(|l| l.path.clone())
}

fn now_str() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// 写一行。ty: INFO/CALL/NET/OUT/ERR/IN
pub fn log(ty: &str, content: &str) {
    let line = format!("{} - [{}] {}", now_str(), ty, mask(content));
    let lock = LOGGER.lock().unwrap();
    if let Some(l) = lock.as_ref() {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&l.path) {
            let _ = writeln!(f, "{line}");
        }
    }
    // 控制台亦同步（不写敏感内容，mask 已处理）
    println!("{line}");
}

/// 操作 / 调用
pub fn call(msg: impl std::fmt::Display) {
    log("CALL", &msg.to_string());
}
/// 网络通讯
pub fn net(msg: impl std::fmt::Display) {
    log("NET", &msg.to_string());
}
/// 输出 / 返回
pub fn out(msg: impl std::fmt::Display) {
    log("OUT", &msg.to_string());
}
/// 输入（注意对敏感项打码后再传）
pub fn input(msg: impl std::fmt::Display) {
    log("IN", &msg.to_string());
}
/// 错误
pub fn err(msg: impl std::fmt::Display) {
    log("ERR", &msg.to_string());
}
/// 一般信息
pub fn info(msg: impl std::fmt::Display) {
    log("INFO", &msg.to_string());
}

/// 对内容做隐私打码：
/// - 常见键值口令（password / passphrase / passwd / pwd / key / secret / token）
/// - User 下用户文件夹名（如 C:\Users\Shin\... 中的 Shin）
pub fn mask(raw: &str) -> String {
    let mut s = raw.to_string();
    // 1) 打码 Windows 用户主目录文件夹名
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        if let Some(dir) = Path::new(&home).file_name().map(|f| f.to_string_lossy().into_owned()) {
            if !dir.is_empty() {
                s = s.replace(&dir, "***");
            }
        }
    }
    // 2) 形如 password=xxx / "password":"xxx" / password: xxx 的敏感值打码
    s = mask_kv(&s);
    s
}

const SENSITIVE_KEYS: [&str; 9] = [
    "password", "passphrase", "passwd", "pwd", "secret", "seckey",
    "private_key", "token", "apikey",
];

/// 将形如 `key<分隔>value` 的敏感键值对打码（不区分大小写，key 边界需为字母数字/引号）。
fn mask_kv(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // 尝试从当前位置匹配任一敏感键
        let mut matched: Option<usize> = None;
        for k in SENSITIVE_KEYS {
            let kl = k.len();
            if i + kl <= chars.len() {
                let seg: String = chars[i..i + kl].iter().collect();
                if seg.to_lowercase() == *k {
                    matched = Some(kl);
                    break;
                }
            }
        }
        if let Some(kl) = matched {
            // 收集 key 之后的空白与分隔符（= : 或 引号）
            let mut j = i + kl;
            let mut sep = String::new();
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                sep.push(chars[j]);
                j += 1;
            }
            let mut delim_open: Option<char> = None;
            if j < chars.len() && (chars[j] == '=' || chars[j] == ':') {
                sep.push(chars[j]);
                j += 1;
                while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                    sep.push(chars[j]);
                    j += 1;
                }
                if j < chars.len() && (chars[j] == '"' || chars[j] == '\'') {
                    delim_open = Some(chars[j]);
                    sep.push(chars[j]);
                    j += 1;
                }
            }
            // 收集 value（到空白 / 分隔符 / 引号结束）
            let vstart = j;
            let mut vlen = 0usize;
            while j < chars.len() {
                let c = chars[j];
                if let Some(d) = delim_open {
                    if c == d {
                        break;
                    }
                } else if c == ' ' || c == '\t' || c == ',' || c == '&' || c == ';' || c == '}' {
                    break;
                }
                vlen += 1;
                j += 1;
            }
            let v: String = chars[vstart..vstart + vlen].iter().collect();
            let v = if v.is_empty() { v } else { mask_secret(&v) };
            out.push_str(&chars[i..i + kl].iter().collect::<String>());
            out.push_str(&sep);
            out.push_str(&v);
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// 将疑似秘密打码：非空时仅保留首 1 位与末 1 位，中间 ***。
fn mask_secret(v: &str) -> String {
    if v.is_empty() {
        return "***".to_string();
    }
    let chars: Vec<char> = v.chars().collect();
    if chars.len() <= 4 {
        return "***".to_string();
    }
    let mut o = String::new();
    o.push(chars[0]);
    o.push_str("***");
    o.push(chars[chars.len() - 1]);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "acs-log-test-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// v3.3.0（D35）：日志文件名必须以毫秒时间戳开头、扩展名 `.log`。
    #[test]
    fn init_creates_dot_log_file() {
        let d = tmp_dir("init");
        init(&d).unwrap();
        let p = path().expect("日志路径应为 Some");
        assert_eq!(p.parent().unwrap(), d.as_path());
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("log"), "扩展名必须是 .log");
        // 文件在首次写入时落盘（懒创建）
        info("log-test-line");
        assert!(p.exists());
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("log-test-line"));
        let _ = fs::remove_dir_all(&d);
    }

    /// D35：只保留最近 N 个日志文件（按时间戳文件名从旧到新删除）。
    #[test]
    fn prune_keeps_only_latest() {
        let d = tmp_dir("prune");
        for i in 0..5 {
            fs::write(d.join(format!("{}.log", 1000 + i)), "x").unwrap();
        }
        fs::write(d.join("not-a-log.txt"), "keep me").unwrap();
        prune_old(&d, 2);
        let mut names: Vec<String> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // 留下最新两个 .log + 非日志文件不被触碰
        assert!(names.contains(&"1003.log".to_string()));
        assert!(names.contains(&"1004.log".to_string()));
        assert!(!names.contains(&"1000.log".to_string()));
        assert!(names.contains(&"not-a-log.txt".to_string()));
        let _ = fs::remove_dir_all(&d);
    }
}
