//! SQLite 连接与建库（中心库 / 客户端库）。
//!
//! 账户分表：accounts_country / accounts_bank / accounts_individual / accounts_system（系统账户：PreIssuedAccount/AESystem/AlphaEU）。
//! 管理员：admins（root/finance 两级，密钥内置）。成员注册表：member_countries / member_companies。
//! 交易：transactions（统一总账）+ tx_confirmations（双方确认）。

use std::path::Path;

use rusqlite::Connection;

use crate::errors::Result;

/// 打开（或创建）SQLite 数据库，启用 WAL。
pub fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

/// 中心库 schema（新结构）。
pub const CENTRAL_SCHEMA: &str = r#"
-- AEU 成员注册表（client 注册下拉）
CREATE TABLE IF NOT EXISTS member_countries(
    id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'Active');
CREATE TABLE IF NOT EXISTS member_companies(
    id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'Active');

-- 客户端账户登录凭证（密码哈希，用于登录取回加密私钥；客户端注册时写入）
CREATE TABLE IF NOT EXISTS account_credentials(
    uid TEXT NOT NULL,
    type TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    PRIMARY KEY(uid, type));

-- 账本账户分表（无 abbr；UID 为唯一识别符）
CREATE TABLE IF NOT EXISTS accounts_country(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS accounts_bank(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS accounts_individual(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);
-- 系统账户（PreIssuedAccount / AESystem / AlphaEU）
CREATE TABLE IF NOT EXISTS accounts_system(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);

-- 统一交易总账
CREATE TABLE IF NOT EXISTS transactions(
    tx_id TEXT PRIMARY KEY, tx_type TEXT NOT NULL,
    sender TEXT NOT NULL, sender_type TEXT NOT NULL,
    receiver TEXT NOT NULL, receiver_type TEXT NOT NULL,
    amount INTEGER NOT NULL, ts INTEGER NOT NULL,
    tx_hash TEXT NOT NULL, sender_sig TEXT NOT NULL,
    central_sig TEXT, sender_last_hash TEXT, receiver_last_hash TEXT,
    status TEXT NOT NULL DEFAULT 'Pending');
CREATE INDEX IF NOT EXISTS idx_tx_sender ON transactions(sender, sender_type);
CREATE INDEX IF NOT EXISTS idx_tx_receiver ON transactions(receiver, receiver_type);

-- 双方确认记录（Mint 除外）
CREATE TABLE IF NOT EXISTS tx_confirmations(
    tx_id TEXT PRIMARY KEY, confirmed INTEGER NOT NULL DEFAULT 0,
    reject_reason TEXT, confirmed_at INTEGER);

-- 邮箱验证码
CREATE TABLE IF NOT EXISTS email_codes(
    email TEXT NOT NULL, code_hash TEXT NOT NULL, purpose TEXT NOT NULL,
    expires_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
    verified INTEGER NOT NULL DEFAULT 0);

-- 镜像 apikey
CREATE TABLE IF NOT EXISTS mirror_keys(
    apikey TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'Active',
    last_pull_at INTEGER);

-- 管理审计日志
CREATE TABLE IF NOT EXISTS audit_log(
    id INTEGER PRIMARY KEY AUTOINCREMENT, actor TEXT NOT NULL, op TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '', ts INTEGER NOT NULL);

-- 商品篮子储备账（Issue/Redeem 双向兑换）
CREATE TABLE IF NOT EXISTS reserve(
    id INTEGER PRIMARY KEY AUTOINCREMENT, item TEXT NOT NULL, qty REAL NOT NULL,
    holder TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'Active', ts INTEGER NOT NULL);
"#;

/// 客户端库 schema。
pub const LOCAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_ledger(
    tx_id TEXT PRIMARY KEY, tx_type TEXT NOT NULL,
    peer TEXT NOT NULL, peer_type TEXT NOT NULL, amount INTEGER NOT NULL,
    ts INTEGER NOT NULL, tx_hash TEXT NOT NULL, central_sig TEXT, status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS known_pubkeys(
    uid TEXT NOT NULL, type TEXT NOT NULL, pubkey TEXT NOT NULL, source TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS keys(
    uid TEXT NOT NULL, type TEXT NOT NULL, encrypted_seckey TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS login_history(
    uid TEXT NOT NULL, type TEXT NOT NULL, last_login INTEGER NOT NULL,
    remember INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
"#;

/// 初始化中心库表。
pub fn init_central(conn: &Connection) -> Result<()> {
    conn.execute_batch(CENTRAL_SCHEMA)?;
    Ok(())
}

/// 初始化客户端（本地）库表。
pub fn init_local(conn: &Connection) -> Result<()> {
    conn.execute_batch(LOCAL_SCHEMA)?;
    Ok(())
}

/// 旧库迁移：处理 abbr 列、旧 member_banks/member_towns、central_keys、pending_registrations。
pub fn migrate_center(conn: &Connection) -> Result<()> {
    // member_banks -> member_companies（数据迁移）
    if table_exists(conn, "member_banks") && !table_exists(conn, "member_companies") {
        conn.execute_batch(
            "CREATE TABLE member_companies(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'Active');
             INSERT INTO member_companies(name, status) SELECT name, status FROM member_banks;
             DROP TABLE member_banks;",
        )?;
    }
    // 废弃表
    for t in ["member_towns", "central_keys", "pending_registrations"] {
        if table_exists(conn, t) {
            conn.execute(&format!("DROP TABLE IF EXISTS {t}"), [])?;
        }
    }
    // 删除账本账户表的 abbr 列
    for t in ["accounts_country", "accounts_bank", "accounts_individual", "accounts_system"] {
        if column_exists(conn, t, "abbr") {
            conn.execute(&format!("ALTER TABLE {t} DROP COLUMN abbr"), [])?;
        }
    }
    // 新增确认表
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tx_confirmations(
            tx_id TEXT PRIMARY KEY, confirmed INTEGER NOT NULL DEFAULT 0,
            reject_reason TEXT, confirmed_at INTEGER);",
    )?;
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        rusqlite::params![name],
        |_| Ok(1),
    )
    .is_ok()
}

fn column_exists(conn: &Connection, table: &str, col: &str) -> bool {
    let mut stmt = match conn.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows = stmt.query_map([], |r| r.get::<_, String>(1));
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            if row == col {
                return true;
            }
        }
    }
    false
}
