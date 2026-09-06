//! 统一文件日志（.alphalog）。
//!
//! 输出格式：`时间 - [记录类型] 记录内容`
//! 记录类型：INFO / CALL / NET / OUT / ERR / IN
//! 隐私保护：口令、密钥、User 下用户名目录等一律以 `***` 打码后再写入。

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

/// 初始化：在 data_dir 下新建 `{启动时间}.alphalog`。每次启动均新建。
pub fn init(data_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let started_at = chrono::Utc::now().timestamp_millis();
    let name = format!("{started_at}.alphalog");
    let path = data_dir.join(name);
    let mut l = LOGGER.lock().unwrap();
    *l = Some(Logger { path, started_at });
    Ok(())
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
