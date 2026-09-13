//! 本地钱包：元数据、密钥、镜像账户快照与待提交（outbox）的持久化。
//!
//! 数据目录 `~/.alpha_dir`，SQLite 本地库（`alpha.db`）+ gpg homedir。
//! 复用 acs_core 的 LOCAL_SCHEMA（known_pubkeys/meta），
//! 另增 client 专用表：`local_accounts`（历史登录账户，登录界面免输 UID 快捷登录）、
//! `mirror_accounts`（镜像账户快照）与 `outbox`（本地签名待提交）。
//! 注：账号相关（uid / 公钥 / 加密私钥）**只存 local_accounts 一张表**，不含任何账目数据。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};

use acs_core::config::CoreConfig;
use acs_core::gpg::GpgUtil;
use acs_core::models::{AccountType, GeneratedKey};

/// client 专用附加表。
pub const CLIENT_SCHEMA: &str = r#"
-- 镜像账户快照（只读，来自中心 /api/mirror/pull 的 accounts）
-- 主键为 (uid,type)：同名不同类账户（如管理员系统身份 vs 个人账户）互不覆盖
CREATE TABLE IF NOT EXISTS mirror_accounts(
    uid TEXT NOT NULL, type TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, changed_at INTEGER NOT NULL DEFAULT 0, synced_at INTEGER NOT NULL,
    PRIMARY KEY(uid, type));
-- 待提交交易（本地构建 + 签名，尚未/等待提交至中心）
CREATE TABLE IF NOT EXISTS outbox(
    tx_id TEXT PRIMARY KEY, tx_json TEXT NOT NULL, created_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'Pending');  -- Pending | Submitted | Failed
-- 历史登录账户（登录界面快捷选择；**唯一一张账户表**，不含账目）
-- pubkey 仅用于展示与验签，encrypted_seckey 为用户口令加密的私钥（空=需联网向中心取回）
CREATE TABLE IF NOT EXISTS local_accounts(
    uid TEXT NOT NULL, type TEXT NOT NULL,
    email TEXT NOT NULL DEFAULT '', pubkey TEXT NOT NULL DEFAULT '',
    encrypted_seckey TEXT NOT NULL DEFAULT '',
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

/// 本地历史登录账户（多账户清单中的一项）。
#[derive(Debug, Clone)]
pub struct LocalAccount {
    pub uid: String,
    pub atype: AccountType,
    pub email: String,
    pub pubkey: String,           // gpg 公钥（armored），仅展示/验签用
    pub encrypted_seckey: String, // 密码加密私钥缓存（可能为空，空=需联网取回）
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

/// 一次性迁移：修正“只按 uid 归属”导致的同名账户串账。
/// - `mirror_accounts` 旧版主键为 `uid`（同名不同类账户互相覆盖）→ 删除重建为复合主键 `(uid,type)`
/// - `local_ledger`（本地账本副本）已取消 → 直接删除该表（账本改由中心按需提供）
fn migrate_uid_type(conn: &Connection) -> rusqlite::Result<()> {
    if meta_get(conn, "fix_uidtype_v1").is_some() {
        return Ok(());
    }
    let old: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='mirror_accounts'",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some(s) = old {
        if s.contains("uid TEXT PRIMARY KEY") {
            conn.execute_batch("DROP TABLE mirror_accounts;")?;
        }
    }
    // CLIENT_SCHEMA 为幂等 CREATE IF NOT EXISTS，重建被删除的表
    conn.execute_batch(CLIENT_SCHEMA)?;
    conn.execute_batch("DROP TABLE IF EXISTS local_ledger;")?;
    meta_set(conn, "fix_uidtype_v1", "1")?;
    acs_core::log::info("已迁移：账户归属改为 (uid,type)；本地账本副本（local_ledger）已取消");
    Ok(())
}

/// 一次性迁移 v2：账户信息归并到 `local_accounts` 一张表
/// - `local_accounts` 补列 `pubkey`（登录界面展示历史账户 / 验签用）
/// - `keys`（旧加密私钥缓存） → 搬入 `local_accounts.encrypted_seckey` 后删表
/// - `login_history`（旧登录历史） → 已由 `local_accounts.last_login` 取代，删表
/// 注：**先搬后删**，避免丢掉旧库里唯一一份加密私钥。
fn migrate_local_v2(conn: &Connection) -> rusqlite::Result<()> {
    if meta_get(conn, "fix_localacct_v2").is_some() {
        return Ok(());
    }
    // 1) 补 pubkey 列
    let has_pubkey: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('local_accounts') WHERE name='pubkey'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(true);
    if !has_pubkey {
        conn.execute(
            "ALTER TABLE local_accounts ADD COLUMN pubkey TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    // 2) keys -> local_accounts（仅在本地无缓存时回填）
    if table_exists(conn, "keys") {
        conn.execute(
            "UPDATE local_accounts SET encrypted_seckey=IFNULL((\
                 SELECT k.encrypted_seckey FROM keys k \
                 WHERE k.uid=local_accounts.uid AND k.encrypted_seckey<>'' \
                 ORDER BY k.rowid DESC LIMIT 1), encrypted_seckey) \
             WHERE encrypted_seckey=''",
            [],
        )?;
        conn.execute("DROP TABLE keys", [])?;
    }
    if table_exists(conn, "login_history") {
        conn.execute("DROP TABLE login_history", [])?;
    }
    // known_pubkeys：旧库无主键（可重复写入）→ 重建为 PRIMARY KEY(uid,type)。
    // 该表是可重新获取的公钥缓存（目前为空），重建不丢数据。
    if table_exists(conn, "known_pubkeys") && !table_has_pk(conn, "known_pubkeys") {
        conn.execute("DROP TABLE known_pubkeys", [])?;
        conn.execute_batch(
            "CREATE TABLE known_pubkeys(
                uid TEXT NOT NULL, type TEXT NOT NULL, pubkey TEXT NOT NULL, source TEXT NOT NULL,
                PRIMARY KEY(uid, type));",
        )?;
        acs_core::log::info("已迁移：known_pubkeys 重建为 PRIMARY KEY(uid,type)");
    }
    meta_set(conn, "fix_localacct_v2", "1")?;
    acs_core::log::info("已迁移：账户信息（uid/公钥/加密私钥）统一存于 local_accounts；keys / login_history 已并入");
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |_| Ok(1),
    )
    .is_ok()
}

/// 该表是否已有主键（用于识别老库中缺主键的缓存表）。
fn table_has_pk(conn: &Connection, name: &str) -> bool {
    let sql = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default()
        .to_uppercase();
    sql.contains("PRIMARY KEY")
}

fn load_info(conn: &Connection) -> WalletInfo {    let atype = meta_get(conn, "wallet_type")
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
        // 把客户端数据目录 .env 注入进程环境（品牌 / 域名等），并初始化本次启动的 .alphalog
        let _ = acs_core::config::load_env_file(&cfg.data_dir);
        let _ = acs_core::log::init(&cfg.data_dir);
        acs_core::log::info(format!(
            "A€ 钱包启动 v{} 数据目录: {}",
            acs_core::VERSION,
            cfg.data_dir.display()
        ));
        let conn = db_open(&cfg.db_path)?;
        acs_core::db::init_local(&conn)?;
        conn.execute_batch(CLIENT_SCHEMA)?;
        migrate_uid_type(&conn)?;
        migrate_local_v2(&conn)?;

        let (gpg_bin, _src) = acs_core::gpg_detect::ensure_gpg()
            .map_err(|e| anyhow!("未找到 gpg：{e}"))?;
        let gpg = GpgUtil::new(gpg_bin, cfg.gpg_homedir.clone());

        let info = load_info(&conn);
        Ok(Wallet { conn, gpg, info })
    }

    /// 生成钱包密钥（ed25519）并写入本地，返回密钥信息。
    /// 注：加密私钥统一由 `save_local_account` 存入 local_accounts（不再写 keys 表）。
    pub fn create_key(&self, uid: &str, email: &str, passphrase: &str) -> Result<GeneratedKey> {
        let gk = self
            .gpg
            .generate_key(&format!("{uid} <{email}>"), passphrase)
            .context("生成钱包密钥失败")?;
        Ok(gk)
    }

    /// 初始化钱包元信息（注册完成后调用）。
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
    /// 按 (uid,type) 查询：避免同名不同类型的账户（如管理员系统身份）串账。
    pub fn mirror_balance(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT balance FROM mirror_accounts WHERE uid=?1 AND type=?2",
                params![self.info.uid, self.info.atype.as_str()],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    // ---- 多账户 ----

    /// 列出本地历史登录账户（最近登录优先）。
    pub fn list_local_accounts(&self) -> Vec<LocalAccount> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT uid, type, email, pubkey, encrypted_seckey, last_login \
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
                    pubkey: r.get(3)?,
                    encrypted_seckey: r.get(4)?,
                    last_login: r.get(5)?,
                })
            })
            .unwrap();
        rows.flatten().collect()
    }

    /// 按 uid 查询本地账户（任一类型）。
    pub fn local_account(&self, uid: &str) -> Option<LocalAccount> {
        self.list_local_accounts().into_iter().find(|a| a.uid == uid)
    }

    /// 当前钱包的加密私钥缓存（取自 local_accounts；空=需联网向中心取回）。
    pub fn encrypted_seckey(&self) -> Option<String> {
        self.conn
            .query_row(
                "SELECT encrypted_seckey FROM local_accounts WHERE uid=?1 AND type=?2",
                params![self.info.uid, self.info.atype.as_str()],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// 保存/更新本地账户（注册、取回密钥或登录后补写公钥时）。
    #[allow(clippy::too_many_arguments)]
    pub fn save_local_account(
        &self,
        uid: &str,
        atype: AccountType,
        email: &str,
        pubkey: &str,
        encrypted_seckey: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO local_accounts(uid, type, email, pubkey, encrypted_seckey, server_url, last_login) \
             VALUES(?1,?2,?3,?4,?5,?6,?7) \
             ON CONFLICT(uid,type) DO UPDATE SET email=excluded.email, \
               pubkey=CASE WHEN excluded.pubkey<>'' THEN excluded.pubkey ELSE local_accounts.pubkey END, \
               encrypted_seckey=CASE WHEN excluded.encrypted_seckey<>'' THEN excluded.encrypted_seckey ELSE local_accounts.encrypted_seckey END, \
               server_url=excluded.server_url",
            params![
                uid,
                atype.as_str(),
                email,
                pubkey,
                encrypted_seckey,
                self.info.server_url,
                chrono::Utc::now().timestamp()
            ],
        )?;
        Ok(())
    }

    /// 登出：清除当前登录账户（回到登录/选择界面）。
    pub fn clear_current(&mut self) -> Result<()> {
        meta_set(&self.conn, "wallet_uid", "")?;
        meta_set(&self.conn, "wallet_type", "")?;
        meta_set(&self.conn, "wallet_email", "")?;
        self.info = load_info(&self.conn);
        Ok(())
    }

    /// 注销账户：从本机删除该账户记录与密钥缓存（中心账户保留）。
    pub fn delete_local_account(&mut self, uid: &str, atype: AccountType) -> Result<()> {
        self.conn.execute(
            "DELETE FROM local_accounts WHERE uid=?1 AND type=?2",
            params![uid, atype.as_str()],
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
            "UPDATE local_accounts SET last_login=?3 WHERE uid=?1 AND type=?2",
            params![acc.uid, acc.atype.as_str(), chrono::Utc::now().timestamp()],
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

/// 默认客户端数据目录（~/.alpha_dir/acs-client），打印用。
pub fn data_dir_str() -> PathBuf {
    CoreConfig::default_alpha_dir().join("acs-client")
}
