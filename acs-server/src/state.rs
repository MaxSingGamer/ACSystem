//! 服务端全局状态：共享 SQLite + 内存会话（10 分钟待机）+ 根管理员密钥解锁态 + 审计二次鉴权。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use acs_core::gpg::GpgUtil;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub type SharedDb = Arc<Mutex<Connection>>;

#[derive(Clone)]
pub struct AppState {
    pub db: SharedDb,
    pub sessions: Arc<Mutex<HashMap<String, Session>>>,
    /// 当前解锁的根管理员密钥（铸造签名用；锁定时清空）。
    pub central: Arc<Mutex<CentralState>>,
    /// 审计/账单二次鉴权：bearer token -> 过期时间戳。
    pub audit_unlocked: Arc<Mutex<HashMap<String, i64>>>,
    /// 「系统账本账户登录」：管理员 token -> 其正在代管的系统账户 uid。
    pub sys_acting: Arc<Mutex<HashMap<String, String>>>,
    pub gpg: GpgUtil,
    pub token_ttl_secs: i64,
    /// 服务器数据目录（~/.alpha_dir/acs-server），内置 updates/ 供客户端下载。
    pub data_dir: PathBuf,
    /// 发行账户 uid（.env 的 PRE_ISSUED_ACCOUNT）；铸造/展示用。
    pub pre_issued: String,
    /// 登录失败次数与锁定截止（暴力破解防护），key = 账户 uid。
    pub login_fails: Arc<Mutex<HashMap<String, LoginFail>>>,
    /// `fetch-key` 限流窗口：key = uid，另用 `__all__` 记录全局计数，值为最近请求时间戳（秒）。
    /// 为何存在：`fetch-key` 仅凭 uid 就能取回加密私钥（无口令校验），必须防枚举遍历。
    pub key_fetch_hits: Arc<Mutex<HashMap<String, Vec<i64>>>>,
}

/// 单个账户的登录失败记录。
pub struct LoginFail {
    pub count: u32,
    pub locked_until: i64,
}

#[derive(Clone, Default)]
pub struct CentralState {
    pub admin_uid: Option<String>,
    pub fingerprint: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub admin_id: i64,
    pub username: String,
    pub role: String, // "root" | "finance"
    pub must_change_password: bool,
    /// 首次强制改密时短暂持有登录密码（仅内存、改密后立即清除），用于解开既有密钥 passphrase。
    pub pending_pwd: Option<String>,
    pub expires_at: i64,
}

impl AppState {
    pub fn new(
        conn: Connection,
        gpg: GpgUtil,
        data_dir: std::path::PathBuf,
        pre_issued: String,
    ) -> Self {
        AppState {
            db: Arc::new(Mutex::new(conn)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            central: Arc::new(Mutex::new(CentralState::default())),
            audit_unlocked: Arc::new(Mutex::new(HashMap::new())),
            sys_acting: Arc::new(Mutex::new(HashMap::new())),
            gpg,
            token_ttl_secs: std::env::var("ACS_TOKEN_TTL_SECS")
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(600), // 默认 10 分钟待机
            data_dir,
            pre_issued,
            login_fails: Arc::new(Mutex::new(HashMap::new())),
            key_fetch_hits: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// 服务端额外表（后台管理员，两级：root / finance）。
pub const SERVER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS admins(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT UNIQUE NOT NULL,
    role TEXT NOT NULL DEFAULT 'finance',
    password_hash TEXT NOT NULL,
    must_change_password INTEGER NOT NULL DEFAULT 0,
    pubkey TEXT NOT NULL DEFAULT '',
    encrypted_seckey TEXT NOT NULL DEFAULT '',
    fingerprint TEXT NOT NULL DEFAULT '',
    key_passphrase_enc TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Active',
    last_login INTEGER NOT NULL DEFAULT 0,
    -- 登录失败计数与锁定截止时间：持久化，重启不清零（防「重启绕过锁定」）
    fail_count INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL);
"#;

pub fn init_server_db(conn: &Connection) -> acs_core::errors::Result<()> {
    conn.execute_batch(SERVER_SCHEMA)?;
    // 老库补列
    ensure_column(conn, "admins", "last_login", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(conn, "admins", "fail_count", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(conn, "admins", "locked_until", "INTEGER NOT NULL DEFAULT 0")?;
    Ok(())
}

/// 若表缺列则 ALTER TABLE 补列（SQLite 老库升级）。
fn ensure_column(conn: &Connection, table: &str, col: &str, decl: &str) -> acs_core::errors::Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name=?2",
            rusqlite::params![table, col],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !exists {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {col} {decl}");
        let _ = conn.execute(&sql, []);
    }
    Ok(())
}
