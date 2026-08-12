//! 本地钱包：元数据、密钥、镜像账户快照与待提交（outbox）的持久化。
//!
//! 数据目录 `~/.alpha_dir`，SQLite 本地库（`alpha.db`）+ gpg homedir。
//! 复用 acs_core 的 LOCAL_SCHEMA（local_ledger/known_pubkeys/keys/login_history/meta），
//! 另增 client 专用表：`mirror_accounts`（镜像账户快照）与 `outbox`（本地签名待提交）。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};

use acs_core::config::CoreConfig;
use acs_core::gpg::GpgUtil;
use acs_core::models::{AccountType, GeneratedKey};

/// client 专用附加表。
pub const CLIENT_SCHEMA: &str = r#"
-- 镜像账户快照（只读，来自中心 /api/mirror/pull 的 accounts）
CREATE TABLE IF NOT EXISTS mirror_accounts(
    uid TEXT PRIMARY KEY, type TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, changed_at INTEGER NOT NULL DEFAULT 0, synced_at INTEGER NOT NULL);
-- 待提交交易（本地构建 + 签名，尚未/等待提交至中心）
CREATE TABLE IF NOT EXISTS outbox(
    tx_id TEXT PRIMARY KEY, tx_json TEXT NOT NULL, created_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'Pending');  -- Pending | Submitted | Failed
-- 本地已登录账户清单（多账户；encrypted_seckey 为密码加密私钥缓存，空=需联网取回）
CREATE TABLE IF NOT EXISTS local_accounts(
    uid TEXT NOT NULL, type TEXT NOT NULL,
    email TEXT NOT NULL, encrypted_seckey TEXT NOT NULL DEFAULT '',
    server_url TEXT NOT NULL DEFAULT '', last_login INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(uid, type));
"#;

/// 钱包元信息（meta 表）。
#[derive(Debug, Clone)]
pub struct WalletInfo {
    pub uid: String,
    pub atype: AccountType,
    pub email: String,
    pub server_url: String,
    pub mirror_apikey: String,
    pub created_at: i64,
    pub synced_at: i64,
    pub last_tx_hash: String,
}

impl Default for WalletInfo {
    fn default() -> Self {
        WalletInfo {
            uid: String::new(),
            atype: AccountType::Individual,
            email: String::new(),
            server_url: String::new(),
            mirror_apikey: String::new(),
            created_at: 0,
            synced_at: 0,
            last_tx_hash: String::new(),
        }
    }
}

impl WalletInfo {
    pub fn initialized(&self) -> bool {
        !self.uid.is_empty()
    }
}

/// 本地已登录账户（多账户清单中的一项）。
#[derive(Debug, Clone)]
pub struct LocalAccount {
    pub uid: String,
    pub atype: AccountType,
    pub email: String,
    pub encrypted_seckey: String, // 密码加密私钥缓存（可能为空）
    pub last_login: i64,
}

/// 本地钱包存储。
pub struct Wallet {
    pub conn: Connection,
    pub gpg: GpgUtil,
    pub info: WalletInfo,
}

fn meta_get(conn: &Connection, k: &str) -> Option<String> {
    conn.query_row("SELECT v FROM meta WHERE k=?1", params![k], |r| r.get(0))
        .ok()
}

fn meta_set(conn: &Connection, k: &str, v: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
        params![k, v],
    )?;
    Ok(())
}

fn load_info(conn: &Connection) -> WalletInfo {
    let atype = meta_get(conn, "wallet_type")
        .and_then(|s| AccountType::from_str(&s))
        .unwrap_or(AccountType::Individual);
    WalletInfo {
        uid: meta_get(conn, "wallet_uid").unwrap_or_default(),
        atype,
        email: meta_get(conn, "wallet_email").unwrap_or_default(),
        server_url: meta_get(conn, "server_url").unwrap_or_default(),
        mirror_apikey: meta_get(conn, "mirror_apikey").unwrap_or_default(),
        created_at: meta_get(conn, "created_at")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        synced_at: meta_get(conn, "synced_at")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        last_tx_hash: meta_get(conn, "last_tx_hash").unwrap_or_default(),
    }
}

impl Wallet {
    /// 打开（或创建）本地钱包存储。若尚未初始化钱包，info.uid 为空。
    pub fn open() -> Result<Wallet> {
        let cfg = CoreConfig::client_default();
        cfg.ensure_dirs()?;
        let conn = db_open(&cfg.db_path)?;
        acs_core::db::init_local(&conn)?;
        conn.execute_batch(CLIENT_SCHEMA)?;

        let (gpg_bin, _src) = acs_core::gpg_detect::ensure_gpg()
            .map_err(|e| anyhow!("未找到 gpg：{e}"))?;
        let gpg = GpgUtil::new(gpg_bin, cfg.gpg_homedir.clone());

        let info = load_info(&conn);
        Ok(Wallet { conn, gpg, info })
    }

    /// 生成钱包密钥（ed25519）并写入本地，返回密钥信息。
    pub fn create_key(&self, uid: &str, email: &str, passphrase: &str) -> Result<GeneratedKey> {
        let gk = self
            .gpg
            .generate_key(&format!("{uid} <{email}>"), passphrase)
            .context("生成钱包密钥失败")?;
        // 本地 keys 表保存（与中心一致的密码上锁私钥，供恢复/换机导入）
        self.conn.execute(
            "INSERT OR REPLACE INTO keys(uid,type,encrypted_seckey) VALUES(?1,?2,?3)",
            params![uid, "wallet", gk.encrypted_seckey],
        )?;
        Ok(gk)
    }

    /// 初始化钱包元信息（首次引导完成后调用）。
    pub fn init_wallet(&mut self, uid: &str, atype: AccountType, email: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        for (k, v) in [
            ("wallet_uid", uid.to_string()),
            ("wallet_type", atype.as_str().to_string()),
            ("wallet_email", email.to_string()),
            ("created_at", now.to_string()),
        ] {
            meta_set(&self.conn, k, &v)?;
        }
        self.info = load_info(&self.conn);
        Ok(())
    }

    pub fn set_server_url(&mut self, url: &str) -> Result<()> {
        meta_set(&self.conn, "server_url", url)?;
        self.info.server_url = url.to_string();
        Ok(())
    }

    pub fn set_mirror_apikey(&mut self, key: &str) -> Result<()> {
        meta_set(&self.conn, "mirror_apikey", key)?;
        self.info.mirror_apikey = key.to_string();
        Ok(())
    }

    pub fn mark_synced(&mut self, server_time: i64, last_tx_hash: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        meta_set(&self.conn, "synced_at", &now.to_string())?;
        self.info.synced_at = now;
        if let Some(h) = last_tx_hash {
            meta_set(&self.conn, "last_tx_hash", h)?;
            self.info.last_tx_hash = h.to_string();
        }
        let _ = server_time;
        Ok(())
    }

    /// 本地密钥指纹（若无则 None）。
    pub fn fingerprint(&self, uid: &str) -> Option<String> {
        self.gpg.fingerprint(uid).ok()
    }

    /// 本账户在最近一次镜像快照中的余额（中心口径）。
    pub fn mirror_balance(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT balance FROM mirror_accounts WHERE uid=?1",
                params![self.info.uid],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    // ---- 多账户 ----

    /// 列出本地已登录账户（多账户清单，最近登录优先）。
    pub fn list_local_accounts(&self) -> Vec<LocalAccount> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT uid, type, email, encrypted_seckey, last_login \
                 FROM local_accounts ORDER BY last_login DESC",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                let t: String = r.get(1)?;
                Ok(LocalAccount {
                    uid: r.get(0)?,
                    atype: AccountType::from_str(&t).unwrap_or(AccountType::Individual),
                    email: r.get(2)?,
                    encrypted_seckey: r.get(3)?,
                    last_login: r.get(4)?,
                })
            })
            .unwrap();
        rows.flatten().collect()
    }

    /// 按 uid 查询本地账户（任一类型）。
    pub fn local_account(&self, uid: &str) -> Option<LocalAccount> {
        self.list_local_accounts().into_iter().find(|a| a.uid == uid)
    }

    /// 当前钱包的加密私钥缓存（keys 表，注册时写入）。
    pub fn encrypted_seckey(&self) -> Option<String> {
        self.conn
            .query_row(
                "SELECT encrypted_seckey FROM keys WHERE uid=?1 ORDER BY rowid DESC LIMIT 1",
                params![self.info.uid],
                |r| r.get(0),
            )
            .ok()
    }

    /// 保存/更新本地账户（注册或取回后）。
    pub fn save_local_account(
        &self,
        uid: &str,
        atype: AccountType,
        email: &str,
        encrypted_seckey: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO local_accounts(uid, type, email, encrypted_seckey, server_url, last_login) \
             VALUES(?1,?2,?3,?4,?5,?6) \
             ON CONFLICT(uid,type) DO UPDATE SET email=excluded.email, \
               encrypted_seckey=excluded.encrypted_seckey, server_url=excluded.server_url",
            params![
                uid,
                atype.as_str(),
                email,
                encrypted_seckey,
                self.info.server_url,
                chrono::Utc::now().timestamp()
            ],
        )?;
        Ok(())
    }

    /// 切换到指定账户（更新当前登录元信息；密钥已导入 gpg homedir，互不干扰）。
    pub fn switch_account(&mut self, uid: &str) -> Result<()> {
        let acc = self
            .local_account(uid)
            .ok_or_else(|| anyhow!("本地无账户 {uid}"))?;
        for (k, v) in [
            ("wallet_uid", acc.uid.clone()),
            ("wallet_type", acc.atype.as_str().to_string()),
            ("wallet_email", acc.email.clone()),
        ] {
            meta_set(&self.conn, k, &v)?;
        }
        self.conn.execute(
            "UPDATE local_accounts SET last_login=?2 WHERE uid=?1",
            params![uid, chrono::Utc::now().timestamp()],
        )?;
        self.info = load_info(&self.conn);
        Ok(())
    }
}

fn db_open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    Ok(conn)
}

/// 默认数据目录（~/.alpha_dir），打印用。
pub fn data_dir_str() -> PathBuf {
    CoreConfig::default_alpha_dir()
}
