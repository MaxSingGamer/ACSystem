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
    pub fn new(conn: Connection, gpg: GpgUtil, data_dir: std::path::PathBuf) -> Self {
        AppState {
            db: Arc::new(Mutex::new(conn)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            central: Arc::new(Mutex::new(CentralState::default())),
            audit_unlocked: Arc::new(Mutex::new(HashMap::new())),
            sys_acting: Arc::new(Mutex::new(HashMap::new())),
            gpg,
            token_ttl_secs: 600, // 10 分钟待机
            data_dir,
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
    created_at INTEGER NOT NULL);
"#;

pub fn init_server_db(conn: &Connection) -> acs_core::errors::Result<()> {
    conn.execute_batch(SERVER_SCHEMA)?;
    // 老库补列：admins.last_login
    ensure_column(conn, "admins", "last_login", "INTEGER NOT NULL DEFAULT 0")?;
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
