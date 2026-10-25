//! 本地钱包：元数据、密钥、镜像账户快照与待提交（outbox）的持久化。
//!
//! 数据目录 `~/.alphawallet/<中心域名>/`，SQLite 本地库（`wallet.db`）+ gpg homedir + logs/（v3.3.0 #9）。
//! 复用 acs_core 的 LOCAL_SCHEMA（known_pubkeys/meta），
//! 另增 client 专用表：`local_accounts`（历史登录账户，登录界面免输 UID 快捷登录）、
//! `mirror_accounts`（镜像账户快照）与 `outbox`（本地签名待提交）。
//! 注：账号相关（uid / 公钥 / 加密私钥）**只存 local_accounts 一张表**，不含任何账目数据。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};

use acs_core::gpg::GpgUtil;
use acs_core::models::{AccountType, GeneratedKey};

/// client 专用附加表。
pub const CLIENT_SCHEMA: &str = r#"
-- 镜像账户快照（只读，来自中心 /api/mirror/pull 的 accounts）
-- 主键为 (uid,type)：同名不同类账户（如管理员系统身份 vs 个人账户）互不覆盖
CREATE TABLE IF NOT EXISTS mirror_accounts(
    uid TEXT NOT NULL, type TEXT NOT NULL, fullname TEXT NOT NULL DEFAULT '',
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
    /// 当前数据目录对应的「中心目录键」（v3.3.0 #9）：`~/.alphawallet/<domain_key>/`。
    /// 切换中心地址时整个数据目录随之切换（每个中心一套独立钱包）。
    pub domain: String,
    /// 本次会话内缓存的钱包口令：**仅内存、不落盘、退出即清**。
    ///
    /// 用途：只读查询的持有性证明（待收箱 nonce 签名）不该反复弹口令输入框；
    /// 用户在本次会话里已经为登录/转账/确认输过一次口令，即可复用。
    /// 明确不做任何持久化——落盘会破坏「口令不落盘」这一设计承诺。
    passphrase: Option<String>,
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

/// v3.3.0（#4/#5）：`mirror_accounts` 补列 `fullname`（国家 / 企业全称，用于展示「全称（简称）」）。
fn migrate_mirror_fullname(conn: &Connection) -> rusqlite::Result<()> {
    if meta_get(conn, "fix_mirror_fullname_v1").is_some() {
        return Ok(());
    }
    let has: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mirror_accounts') WHERE name='fullname'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(true);
    if !has {
        // 若表不存在（极早期库），CLIENT_SCHEMA 已建好新结构；此处 ALTER 仅对老表生效
        let _ = conn.execute(
            "ALTER TABLE mirror_accounts ADD COLUMN fullname TEXT NOT NULL DEFAULT ''",
            [],
        );
    }
    meta_set(conn, "fix_mirror_fullname_v1", "1")?;
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

fn load_info(conn: &Connection) -> WalletInfo {    let atype = meta_get(conn, "wallet_type")        .and_then(|s| AccountType::from_str(&s))
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

// ---------- v3.3.0（#9 / D34）：多中心目录与根级历史库 ----------

/// 记录「当前中心目录键」的文件（例如 `127.0.0.1_9699`）。
fn current_file(root: &Path) -> PathBuf {
    root.join("current")
}

fn read_current_domain(root: &Path) -> Option<String> {
    std::fs::read_to_string(current_file(root))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_current_domain(root: &Path, domain: &str) -> Result<()> {
    std::fs::write(current_file(root), domain).context("写入当前中心记录失败")?;
    Ok(())
}

/// 根级中心历史库（D34）：`<root>/central_server_history.db`，记录所有连过的中心。
/// 结构为后续 #15（按**服务名称**展示历史）预留 `name` 字段。
fn history_db_path(root: &Path) -> PathBuf {
    root.join("central_server_history.db")
}

fn record_center(root: &Path, domain: &str, url: &str) -> Result<()> {
    let conn = Connection::open(history_db_path(root)).context("打开中心历史库失败")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS centers(
            domain    TEXT PRIMARY KEY,          -- 目录键（域名[_端口]）
            url       TEXT NOT NULL,             -- 中心地址
            name      TEXT NOT NULL DEFAULT '',  -- 服务名称（#15：由 /api/status 提供）
            last_used INTEGER NOT NULL DEFAULT 0);",
    )?;
    conn.execute(
        "INSERT INTO centers(domain,url,last_used) VALUES (?1,?2,?3) \
         ON CONFLICT(domain) DO UPDATE SET url=excluded.url, last_used=excluded.last_used",
        params![domain, url, chrono::Utc::now().timestamp()],
    )?;
    Ok(())
}

/// 中心历史条目（供 QT 启动页展示；v3.3.0 #8/#15）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CenterInfo {
    /// 目录键（域名[_端口]）
    pub domain: String,
    /// 中心地址
    pub url: String,
    /// 服务名称（由 /api/client/config 的 service_name 回填；#15 优先用它展示）
    pub name: String,
    pub last_used: i64,
    /// 是否当前选中
    pub current: bool,
}

/// 列出所有历史中心（按最近使用倒序）。
pub fn list_centers() -> Result<Vec<CenterInfo>> {
    let root = acs_core::config::wallet_root();
    std::fs::create_dir_all(&root).ok();
    let current = read_current_domain(&root).unwrap_or_default();
    let conn = Connection::open(history_db_path(&root)).context("打开中心历史库失败")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS centers(
            domain TEXT PRIMARY KEY, url TEXT NOT NULL,
            name TEXT NOT NULL DEFAULT '', last_used INTEGER NOT NULL DEFAULT 0);",
    )?;
    let mut stmt = conn.prepare("SELECT domain,url,name,last_used FROM centers ORDER BY last_used DESC")?;
    let rows = stmt.query_map([], |r| {
        let domain: String = r.get(0)?;
        Ok(CenterInfo {
            current: domain == current,
            domain,
            url: r.get(1)?,
            name: r.get(2)?,
            last_used: r.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 回填某中心的服务名称（由 `/api/client/config` 的 service_name 提供，#15）。
pub fn update_center_name(domain: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Ok(());
    }
    let root = acs_core::config::wallet_root();
    let conn = Connection::open(history_db_path(&root)).context("打开中心历史库失败")?;
    conn.execute(
        "UPDATE centers SET name=?2 WHERE domain=?1",
        params![domain, name.trim()],
    )?;
    Ok(())
}

/// 删除一条中心历史（只删历史记录，不删该中心的数据目录）。
pub fn remove_center(domain: &str) -> Result<()> {
    let root = acs_core::config::wallet_root();
    let conn = Connection::open(history_db_path(&root)).context("打开中心历史库失败")?;
    conn.execute("DELETE FROM centers WHERE domain=?1", params![domain])?;
    Ok(())
}

/// 当前中心目录键（未配置时为 None）。
pub fn current_center_domain() -> Option<String> {
    read_current_domain(&acs_core::config::wallet_root())
}
/// 打开（或创建）某中心的数据目录：`<root>/<domain>/`（wallet.db + gnupg + logs）。
fn open_domain(root: &Path, domain: &str) -> Result<(Connection, GpgUtil, WalletInfo)> {
    let dir = acs_core::config::client_domain_dir(root, domain);
    std::fs::create_dir_all(&dir).context("创建中心数据目录失败")?;
    // v3.3.0（#7）：客户端**不再读取 .env** —— 品牌/更新源/行为参数由服务端
    // `/api/client/config` 下发并缓存（见 set_client_config / client_config）。
    let _ = acs_core::log::init(&dir.join("logs"));
    acs_core::log::info(format!(
        "A€ 钱包启动 v{} 数据目录: {}（中心目录键 {}）",
        acs_core::VERSION,
        dir.display(),
        domain
    ));
    let conn = db_open(&dir.join("wallet.db"))?;
    acs_core::db::init_local(&conn)?;
    conn.execute_batch(CLIENT_SCHEMA)?;
    migrate_uid_type(&conn)?;
    migrate_local_v2(&conn)?;
    migrate_mirror_fullname(&conn)?;
    let (gpg_bin, _src) =
        acs_core::gpg_detect::ensure_gpg().map_err(|e| anyhow!("未找到 gpg：{e}"))?;
    let gpg = GpgUtil::new(gpg_bin, dir.join("gnupg"));
    let info = load_info(&conn);
    Ok((conn, gpg, info))
}

impl Wallet {
    /// 打开（或创建）**当前中心**的本地钱包存储（v3.3.0 #9：按中心域名分子目录）。    ///
    /// 目录：`<home>/.alphawallet/<domain_key>/`（wallet.db + gnupg + logs/）；
    /// 根目录另有 `central_server_history.db` 记录所有中心（D34），`current` 文件记录当前中心。
    /// 若尚未配置任何中心，则使用占位目录 `_default`（`new --server` / GUI 首次设置后会切换）。
    pub fn open() -> Result<Wallet> {
        let root = acs_core::config::wallet_root();
        std::fs::create_dir_all(&root).context("创建钱包根目录失败")?;
        let domain = read_current_domain(&root).unwrap_or_else(|| "_default".to_string());
        let (conn, gpg, info) = open_domain(&root, &domain)?;
        Ok(Wallet { conn, gpg, info, domain, passphrase: None })
    }

    /// 切换到指定中心的数据目录（同一进程内切换中心时调用；口令缓存随之清空）。
    fn switch_domain(&mut self, domain: &str) -> Result<()> {
        let root = acs_core::config::wallet_root();
        let (conn, gpg, info) = open_domain(&root, domain)?;
        self.conn = conn;
        self.gpg = gpg;
        self.info = info;
        self.domain = domain.to_string();
        self.passphrase = None;
        write_current_domain(&root, domain)?;
        Ok(())
    }

    /// 记录本次会话的钱包口令（仅内存；登录/转账/确认成功后调用）。
    pub fn set_session_passphrase(&mut self, pass: &str) {
        self.passphrase = Some(pass.to_string());
    }

    /// 清除会话口令（退出登录 / 应用退出 / 注销账户）。
    pub fn clear_session_passphrase(&mut self) {
        self.passphrase = None;
    }

    /// 用账户私钥签名（用于只读查询的持有性证明）。口令来源优先级：
    /// ① 本次会话缓存 → ② 调用方显式提供 → ③ 未加密密钥直接签（历史遗留密钥）。
    /// 全部不可用时报错，由调用方决定是否要求用户输入口令。
    pub fn sign_with(&self, data: &[u8], explicit: Option<&str>) -> Result<String> {
        let fp = self
            .fingerprint(&self.info.uid)
            .ok_or_else(|| anyhow!("未找到钱包密钥（指纹），请重新登录以导入密钥"))?;
        if let Some(pass) = self.passphrase.as_deref().or(explicit) {
            return self
                .gpg
                .sign_detached(&fp, pass, data)
                .map_err(|e| anyhow!("签名失败（会话口令可能已失效，请重新登录）：{e}"));
        }
        self.gpg
            .sign_detached_public_key(&fp, data)
            .map_err(|e| anyhow!("需要钱包口令才能签名，但本次会话尚未缓存口令：{e}"))
    }

    /// 生成钱包密钥（ed25519）并写入本地，返回密钥信息。
    /// 注：加密私钥统一由 `save_local_account` 存入 local_accounts（不再写 keys 表）。
    ///
    /// 出错文案带上账户与底层原因：生成失败最常见的原因是 gpg 环境问题
    /// （信任库损坏、agent 残留等），`generate_key` 会自动自愈并重试，仍失败时
    /// 必须让用户看到真实报错，而不是一句「生成钱包密钥失败」。
    pub fn create_key(&self, uid: &str, email: &str, passphrase: &str) -> Result<GeneratedKey> {
        let gk = self
            .gpg
            .generate_key(&format!("{uid} <{email}>"), passphrase)
            .with_context(|| {
                format!("为账户 {uid} 生成 gpg 密钥失败（gpg 环境异常，可重试一次让程序自动修复）")
            })?;
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

    /// 从本地库重新读取钱包元信息（同步推进水位后刷新内存态，避免界面显示旧值）。
    pub fn reload_info(&mut self) {
        self.info = load_info(&self.conn);
    }

    /// 设置中心地址（v3.3.0 #9）：地址变化 = **切换中心数据目录**（每个中心一套独立钱包）。
    pub fn set_server_url(&mut self, url: &str) -> Result<()> {
        let key = acs_core::config::domain_key(url);
        if key != self.domain {
            self.switch_domain(&key)?;
        }
        meta_set(&self.conn, "server_url", url)?;
        self.info.server_url = url.to_string();
        // 记录到根级中心历史库（所有中心一份）
        let root = acs_core::config::wallet_root();
        let _ = record_center(&root, &key, url);
        Ok(())
    }

    pub fn set_mirror_apikey(&mut self, key: &str) -> Result<()> {
        meta_set(&self.conn, "mirror_apikey", key)?;
        self.info.mirror_apikey = key.to_string();
        Ok(())
    }

    /// 缓存服务端下发的客户端配置（v3.3.0 #7 / D39 白名单）。
    ///
    /// 存本地库是为了**离线可用**：断网时沿用上一份，不阻断界面。
    pub fn set_client_config(&mut self, cfg: &serde_json::Value) -> Result<()> {
        meta_set(&self.conn, "client_config", &serde_json::to_string(cfg)?)?;
        Ok(())
    }

    /// 读取缓存的客户端配置（无缓存时 None）。
    pub fn client_config(&self) -> Option<serde_json::Value> {
        meta_get(&self.conn, "client_config").and_then(|s| serde_json::from_str(&s).ok())
    }

    /// 记录同步水位。
    ///
    /// **水位必须取中心的 `server_time`**（`pull()` 返回的 `r.server_time`），
    /// 不能用本机时钟：增量同步是 `GET /api/sync?since=<水位>`，服务端按
    /// `transactions.ts > since` 过滤。若水位写成「本机 now」，而客户端时钟比服务端快
    /// （哪怕几秒），服务端刚写入的 Confirmed 交易（ts 夹在两者之间）就会被永久跳过，
    /// 表现为「已确认的交易在客户端一直不出现、镜像里的链头停在旧值，之后再转账就报链头不一致」。
    pub fn mark_synced(&mut self, server_time: i64, last_tx_hash: Option<&str>) -> Result<()> {
        // server_time 缺失（=0）时退回本机时间，避免把水位写成 0 导致每次全量拉取
        let watermark = if server_time > 0 {
            server_time
        } else {
            chrono::Utc::now().timestamp()
        };
        meta_set(&self.conn, "synced_at", &watermark.to_string())?;
        self.info.synced_at = watermark;
        if let Some(h) = last_tx_hash {
            meta_set(&self.conn, "last_tx_hash", h)?;
            self.info.last_tx_hash = h.to_string();
        }
        Ok(())
    }

    /// 本地密钥指纹（若无则 None）。
    pub fn fingerprint(&self, uid: &str) -> Option<String> {
        self.gpg.fingerprint(uid).ok()
    }

    /// 清理「同一 User ID 的多把密钥」，每类只保留**最新生成的一把**。返回删除数量。
    ///
    /// 为什么会有重复：gpg 对已存在的 User ID 执行 `--quick-generate-key` 时会新建一把
    /// （提示 `A key for "…" already exists` / `creating anyway`），于是注册反复失败就会
    /// 在本地留下多把同名密钥，既难分辨又占空间。
    ///
    /// 若某个 UID 已有登记账户（`local_accounts`），则保留该账户记录对应的那一把
    /// （避免把正在用的密钥删掉、导致登录时找不到密钥）。
    pub fn dedup_duplicate_keys(&self) -> Result<usize> {
        // uid -> [(fpr, created_ts)]，created 取 pub 记录里的真实创建时间
        let mut groups: std::collections::HashMap<String, Vec<(String, i64)>> =
            std::collections::HashMap::new();
        for (uid, fpr, ts) in self.gpg.list_keys_with_uid()? {
            groups.entry(uid).or_default().push((fpr, ts));
        }

        let mut removed = 0usize;
        for (uid, list) in groups {
            if list.len() < 2 {
                continue;
            }
            let bare_uid = uid.split('<').next().unwrap_or(&uid).trim().to_string();
            // ① 优先保留已登记账户对应的那一把（正在用的密钥绝不能删）
            // ② 否则保留**创建时间最新**的一把
            let keep = self
                .local_account(&bare_uid)
                .and_then(|a| self.gpg.fingerprint_of_armored_pubkey(&a.pubkey).ok())
                .filter(|k| list.iter().any(|(f, _)| f == k))
                .or_else(|| list.iter().max_by_key(|(_, ts)| *ts).map(|(f, _)| f.clone()))
                .unwrap_or_default();
            for (f, _) in list {
                if f != keep && self.gpg.delete_key_strict(&f) {
                    acs_core::log::info(format!("已清理重复密钥：{uid} → {f}"));
                    removed += 1;
                }
            }
        }
        Ok(removed)
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

    /// 本账户在最近一次镜像快照中的状态（中心口径：Active / Frozen / Closed / Deleted）。
    /// 用于界面提前提示「不可交易」，避免只能等被服务端拒绝才知道。
    pub fn mirror_status(&self) -> String {
        self.conn
            .query_row(
                "SELECT status FROM mirror_accounts WHERE uid=?1 AND type=?2",
                params![self.info.uid, self.info.atype.as_str()],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_else(|_| "Active".to_string())
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

/// 客户端钱包根目录（`~/.alphawallet`），打印/展示用。
pub fn data_dir_str() -> PathBuf {
    acs_core::config::wallet_root()
}
