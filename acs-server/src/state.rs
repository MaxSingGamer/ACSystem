//! 服务端全局状态：共享 SQLite + 内存会话（10 分钟待机）+ 根管理员密钥解锁态 + 审计二次鉴权。

use std::collections::HashMap;
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
    pub gpg: GpgUtil,
    pub token_ttl_secs: i64,
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
    pub fn new(conn: Connection, gpg: GpgUtil) -> Self {
        AppState {
            db: Arc::new(Mutex::new(conn)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            central: Arc::new(Mutex::new(CentralState::default())),
            audit_unlocked: Arc::new(Mutex::new(HashMap::new())),
            gpg,
            token_ttl_secs: 600, // 10 分钟待机
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
    created_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS mirror_registry(
    url TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Active',
    created_at INTEGER NOT NULL);
"#;

pub fn init_server_db(conn: &Connection) -> acs_core::errors::Result<()> {
    conn.execute_batch(SERVER_SCHEMA)?;
    Ok(())
}
