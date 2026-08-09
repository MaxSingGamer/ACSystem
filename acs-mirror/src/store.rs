//! 本地镜像库：只读账本 + 账户快照 + 元信息。
//! 数据目录默认 `~/.alpha_mirror`（可用 `ACS_MIRROR_DIR` 覆盖）。

use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{params, Connection};

/// 镜像库 schema。
pub const MIRROR_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_ledger(
    tx_id TEXT PRIMARY KEY, tx_type TEXT NOT NULL,
    peer TEXT NOT NULL, peer_type TEXT NOT NULL, amount INTEGER NOT NULL,
    ts INTEGER NOT NULL, tx_hash TEXT NOT NULL, central_sig TEXT, status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS mirror_accounts(
    uid TEXT PRIMARY KEY, type TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, changed_at INTEGER NOT NULL DEFAULT 0, synced_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
"#;

#[derive(Debug, Clone, Default)]
pub struct MirrorInfo {
    pub server_url: String,
    pub apikey: String,
    pub last_sync: i64,
    pub last_hash: String,
    pub central_pubkey: String, // 可选：中心公钥（用于校验快照签名）
}

pub struct Store {
    pub conn: Connection,
    pub info: MirrorInfo,
}

fn meta_get(conn: &Connection, k: &str) -> Option<String> {
    conn.query_row("SELECT v FROM meta WHERE k=?1", params![k], |r| r.get(0))
        .ok()
}
pub fn meta_set(conn: &Connection, k: &str, v: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
        params![k, v],
    )?;
    Ok(())
}

impl Store {
    pub fn open() -> Result<Store> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("mirror.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(MIRROR_SCHEMA)?;
        let info = MirrorInfo {
            server_url: meta_get(&conn, "server_url").unwrap_or_default(),
            apikey: meta_get(&conn, "apikey").unwrap_or_default(),
            last_sync: meta_get(&conn, "last_sync")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            last_hash: meta_get(&conn, "last_hash").unwrap_or_default(),
            central_pubkey: meta_get(&conn, "central_pubkey").unwrap_or_default(),
        };
        Ok(Store { conn, info })
    }

    pub fn set_config(&mut self, server_url: &str, apikey: &str) -> Result<()> {
        meta_set(&self.conn, "server_url", server_url)?;
        meta_set(&self.conn, "apikey", apikey)?;
        self.info.server_url = server_url.to_string();
        self.info.apikey = apikey.to_string();
        Ok(())
    }

    pub fn mark_synced(&mut self, server_time: i64, hash: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        meta_set(&self.conn, "last_sync", &now.to_string())?;
        meta_set(&self.conn, "last_hash", hash)?;
        self.info.last_sync = now;
        self.info.last_hash = hash.to_string();
        let _ = server_time;
        Ok(())
    }

    pub fn since(&self) -> i64 {
        self.conn
            .query_row("SELECT COALESCE(MAX(ts),0) FROM local_ledger", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn account_count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM mirror_accounts", [], |r| r.get(0))
            .unwrap_or(0)
    }
    pub fn tx_count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM local_ledger", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// 查询单账户快照。
    pub fn account(&self, uid: &str) -> Option<(String, String, i64, String, Option<String>)> {
        self.conn
            .query_row(
                "SELECT uid, type, balance, status, last_tx_hash FROM mirror_accounts WHERE uid=?1",
                params![uid],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get::<_, i64>(2)?,
                        r.get(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .ok()
    }
}

/// 默认数据目录。
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ACS_MIRROR_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".alpha_mirror")
}

// 让 Path 可被引用（避免未用告警）
pub fn _p(p: &Path) -> &Path {
    p
}
