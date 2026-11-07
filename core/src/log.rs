//! 统一文件日志（v4.0.0：文件名 `YYMMDD-HH_mm_SS.alphalog`，按中心域名分目录）。
//!
//! 输出格式：`时间 - [记录类型] 记录内容`
//! 记录类型：INFO / CALL / NET / OUT / ERR / IN
//! 隐私保护：口令、密钥、User 下用户名目录等一律以 `***` 打码后再写入。
//!
//! 目录约定：
//! - 服务端：`~/.alphacoinserver/<服务域名>/logs/`
//! - 面板：`~/.alphacoinserver/logs/`（仅面板自身日志；服务进程日志由服务自己写）
//! - 客户端：`~/.alphawallet/<中心域名>/logs/`、`~/.alphacompanywallet/<中心域名>/logs/`
//!
//! 轮转：每次启动新建 `<本地时间 YYMMDD-HH_mm_SS>.alphalog`；初始化时清理最旧的，
//! 仅保留最近 `ACS_LOG_KEEP_FILES`（默认 30）个日志文件。

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;

/// 进程内单例日志器。
static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

pub struct Logger {
    path: PathBuf,
    #[allow(dead_code)]
    started_at: i64,
    /// 常驻文件句柄 + 缓冲写入（v4.0.0 DDoS 加固）：
    /// 旧实现每写一行都 `create+open+writeln+close`，高并发下 syscall 放大明显；
    /// 现在初始化时打开一次，写行后 flush，保证落盘语义同时减少系统调用。
    writer: BufWriter<File>,
}

/// 默认保留的日志文件数。
const DEFAULT_KEEP_FILES: usize = 30;

/// 读取 `ACS_LOG_KEEP_FILES`（默认 30，至少 1）。
fn keep_count() -> usize {
    std::env::var("ACS_LOG_KEEP_FILES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_KEEP_FILES)
}

/// 生成日志文件名：`YYMMDD-HH_mm_SS.alphalog`（本地时间）。
pub fn log_file_name() -> String {
    format!("{}.alphalog", Local::now().format("%y%m%d-%H_%M_%S"))
}

/// 初始化：在给定 **logs 目录**下新建 `YYMMDD-HH_mm_SS.alphalog`（每次启动一个文件）。
///
/// 参数是「日志目录」——因为日志按中心域名分目录，域名是运行时才知道的，
/// 由调用方显式传入 `<域名>/logs`。
pub fn init(logs_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(logs_dir).map_err(|e| e.to_string())?;
    // 轮转：先清理旧日志，再创建本次文件（避免把本次文件算进「待删」）
    prune_old(logs_dir, keep_count());
    let started_at = chrono::Utc::now().timestamp_millis();
    let path = unique_path(logs_dir.join(log_file_name()));
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut l = LOGGER.lock().unwrap();
    *l = Some(Logger {
        path,
        started_at,
        writer: BufWriter::with_capacity(16 * 1024, f),
    });
    Ok(())
}

/// 使用**指定文件**作为本次日志输出（面板启动服务时传入 `-logfile`，
/// 使服务进程日志与面板自己选定的文件名一致，不再需要额外的 `panel-spawn.log`）。
///
/// 同样按 `ACS_LOG_KEEP_FILES` 轮转同目录下的旧 `.alphalog`。
pub fn init_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        prune_old(parent, keep_count());
    }
    let started_at = chrono::Utc::now().timestamp_millis();
    let path = path.to_path_buf();
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut l = LOGGER.lock().unwrap();
    *l = Some(Logger {
        path,
        started_at,
        writer: BufWriter::with_capacity(16 * 1024, f),
    });
    Ok(())
}

/// 若目标文件已存在（同一秒内两次启动），追加 `-2` / `-3` 后缀，避免互相覆盖。
fn unique_path(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("log").to_string();
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("alphalog").to_string();
    for n in 2..1000 {
        let cand = p.with_file_name(format!("{stem}-{n}.{ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    p
}

/// 文件名是否符合日志命名（`YYMMDD-HH_mm_SS[-\d+].alphalog`）。
fn is_log_name(stem: &str) -> bool {
    // 忽略可能存在的 `-N` 去重后缀：只校验前 15 个字符的固定格式
    let chars: Vec<char> = stem.chars().collect();
    if chars.len() < 15 {
        return false;
    }
    let pat = [0usize, 1, 2, 3, 4, 5, 7, 8, 10, 11, 13, 14];
    for i in pat {
        if !chars[i].is_ascii_digit() {
            return false;
        }
    }
    chars[6] == '-' && chars[9] == '_' && chars[12] == '_'
}

/// 只保留最近 `keep` 个 `*.alphalog`（文件名以时间开头，按名排序即按时间排序）。
fn prune_old(dir: &Path, keep: usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("alphalog")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(is_log_name)
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
    let mut lock = LOGGER.lock().unwrap();
    if let Some(l) = lock.as_mut() {
        let _ = writeln!(l.writer, "{line}");
        // 保持「写一行即落盘」语义（崩溃不丢最近日志），但仍复用文件句柄。
        let _ = l.writer.flush();
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

    /// 日志器是进程内单例，多个测试并行会互相覆盖 → 串行执行涉及 `init*` 的用例。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

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

    /// v4.0.0：日志文件名必须是 `YYMMDD-HH_mm_SS.alphalog`。
    #[test]
    fn init_creates_alphalog_file() {
        let _g = test_guard();
        let d = tmp_dir("init");
        init(&d).unwrap();
        let p = path().expect("日志路径应为 Some");
        assert_eq!(p.parent().unwrap(), d.as_path());
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("alphalog"),
            "扩展名必须是 .alphalog"
        );
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap().to_string();
        assert!(is_log_name(&stem), "文件名格式应为 YYMMDD-HH_mm_SS：{stem}");
        assert_eq!(stem.len(), 15);
        // 文件在首次写入时落盘（懒创建）
        info("log-test-line");
        assert!(p.exists());
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("log-test-line"));
        let _ = fs::remove_dir_all(&d);
    }

    /// init_file：使用调用方指定的文件（面板 `-logfile` 场景），不另建文件。
    #[test]
    fn init_file_uses_given_path() {
        let _g = test_guard();
        let d = tmp_dir("initfile");
        let target = d.join("260101-12_34_56.alphalog");
        init_file(&target).unwrap();
        assert_eq!(path().unwrap(), target);
        info("file-log-line");
        assert!(fs::read_to_string(&d.join("260101-12_34_56.alphalog")).unwrap().contains("file-log-line"));
        let _ = fs::remove_dir_all(&d);
    }

    /// 只保留最近 N 个日志文件（按时间文件名从旧到新删除）。
    #[test]
    fn prune_keeps_only_latest() {
        let _g = test_guard();
        let d = tmp_dir("prune");
        let mut names_in = Vec::new();
        for i in 0..5 {
            let n = format!("260101-12_00_0{i}.alphalog");
            fs::write(d.join(&n), "x").unwrap();
            names_in.push(n);
        }
        fs::write(d.join("not-a-log.txt"), "keep me").unwrap();
        fs::write(d.join("panel-spawn.log"), "legacy").unwrap();
        prune_old(&d, 2);
        let mut names: Vec<String> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // 留下最新两个 .alphalog + 非日志文件不被触碰
        assert!(names.contains(&names_in[3]));
        assert!(names.contains(&names_in[4]));
        assert!(!names.contains(&names_in[0]));
        assert!(names.contains(&"not-a-log.txt".to_string()));
        // 旧格式文件不再被当作日志轮转
        assert!(names.contains(&"panel-spawn.log".to_string()));
        let _ = fs::remove_dir_all(&d);
    }
}
