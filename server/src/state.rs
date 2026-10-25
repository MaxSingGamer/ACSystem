//! 服务端全局状态：共享 SQLite + 内存会话（10 分钟待机）+ 根管理员密钥解锁态 + 审计二次鉴权。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use acs_core::gpg::GpgUtil;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub type SharedDb = Arc<Mutex<Connection>>;

/// 取锁并**从「中毒」状态恢复**。
///
/// Rust 的 `Mutex` 在持锁线程 panic 时会被「毒化」，此后每次 `lock().unwrap()` 都会 panic：
/// 对一个长驻服务而言，这意味着**一次局部 panic 会把所有后续请求变成永久 500，只能重启**。
/// 账本一致性由 SQLite 事务与 `BEGIN IMMEDIATE` 保证（panic 时未提交的事务会被回滚），
/// 因此这里选择「记录告警 + 继续用」，而不是让整个服务停摆。
///
/// 用法：`let conn = lock(&st.db);`
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            acs_core::log::err("互斥锁已中毒（此前有线程 panic）：已恢复继续使用，请排查上面的 panic 日志");
            poisoned.into_inner()
        }
    }
}

/// 共享数据库连接（服务端所有请求都经此取锁）。
pub fn db_lock(db: &SharedDb) -> MutexGuard<'_, Connection> {
    lock(db)
}

/// 会话表（token -> Session）。
pub fn sessions_lock(
    st: &AppState,
) -> MutexGuard<'_, HashMap<String, Session>> {
    lock(&st.sessions)
}

/// 系统账本代管会话（管理员 token -> 系统账户 uid）。
pub fn sys_acting_lock(st: &AppState) -> MutexGuard<'_, HashMap<String, String>> {
    lock(&st.sys_acting)
}

/// `pending` 接口的短期挑战值：(uid, type) -> (nonce, 过期时间戳)。
pub type PendingNonces = HashMap<(String, String), (String, i64)>;

/// `pending` 接口的短期挑战值（(uid, type) -> (nonce, 过期时间)）。
pub fn pending_nonce_lock(st: &AppState) -> MutexGuard<'_, PendingNonces> {
    lock(&st.pending_nonce)
}

/// 登录失败计数（key -> LoginFail）。
pub fn login_fails_lock(st: &AppState) -> MutexGuard<'_, HashMap<String, LoginFail>> {
    lock(&st.login_fails)
}

/// `fetch-key` 限流窗口（key -> 最近请求时间戳）。
pub fn key_fetch_hits_lock(st: &AppState) -> MutexGuard<'_, HashMap<String, KeyFetchHits>> {
    lock(&st.key_fetch_hits)
}

/// 根管理员密钥解锁态。
pub fn central_lock(st: &AppState) -> MutexGuard<'_, CentralState> {
    lock(&st.central)
}

/// 主密钥（中心主密钥）内存态：`None` = 未解锁（冷启动后 / 无 Root 登录过）。
///
/// v3.3.0（D8/D10/D15）：主密钥**不再落文件**，只以「各 Root 口令封装的副本」存于
/// `admin_master_key` 表；进程重启后必须由某个根管理员登录（输入自己的密码）解密其副本
/// 并载入本槽，之后系统账本相关操作（解封系统账户私钥口令）才可用。
pub fn master_lock(st: &AppState) -> MutexGuard<'_, Option<String>> {
    lock(&st.master)
}

/// 单个 key 的限流窗口：**按时间递增**，只从尾部 push、从头部淘汰，
/// 淘汰是 O(过期数) 而不是每次 `retain` 全量扫描。
#[derive(Default, Debug)]
pub struct KeyFetchHits {
    hits: std::collections::VecDeque<i64>,
}

impl KeyFetchHits {
    /// 丢弃窗口外的记录（`now - t >= window`），返回窗口内剩余数量。
    pub fn prune(&mut self, now: i64, window: i64) -> usize {
        while let Some(front) = self.hits.front() {
            if now - *front >= window {
                self.hits.pop_front();
            } else {
                break;
            }
        }
        self.hits.len()
    }

    /// 记录一次请求。
    pub fn push(&mut self, now: i64) {
        self.hits.push_back(now);
    }

    /// 窗口内记录数。
    pub fn len(&self) -> usize {
        self.hits.len()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: SharedDb,
    pub sessions: Arc<Mutex<HashMap<String, Session>>>,
    /// 当前解锁的根管理员密钥（铸造签名用；锁定时清空）。
    pub central: Arc<Mutex<CentralState>>,
    /// 中心主密钥内存态（v3.3.0 D8/D10/D15）：`None` = 未解锁。
    /// 只由「根管理员登录成功」时用其口令解密自己的 `admin_master_key` 副本后写入。
    pub master: Arc<Mutex<Option<String>>>,
    /// 「系统账本账户登录」：管理员 token -> 其正在代管的系统账户 uid。
    pub sys_acting: Arc<Mutex<HashMap<String, String>>>,
    pub gpg: GpgUtil,
    pub token_ttl_secs: i64,
    /// 服务器数据目录（~/.alphacoinserver/<服务域名>），内置 updates/ 供客户端下载。
    pub data_dir: PathBuf,
    /// 发行账户 uid（.env 的 PRE_ISSUED_ACCOUNT）；铸造/展示用。
    pub pre_issued: String,
    /// 服务名称（v3.3.0 D26：面板 `-name` 启动参数；为空表示未由面板管理）。
    /// 仅用于 `/api/status` 下发给客户端，**不写入 .env / 服务包**（第 5 条）。
    pub service_name: String,
    /// 登录失败次数与锁定截止（暴力破解防护），key = 账户 uid。
    pub login_fails: Arc<Mutex<HashMap<String, LoginFail>>>,
    /// `fetch-key` 限流窗口：key = uid，另用 `__all__` 记录全局计数，值为最近请求时间戳（秒，递增）。
    /// 为何存在：`fetch-key` 仅凭 uid 就能取回加密私钥（无口令校验），必须防枚举遍历。
    pub key_fetch_hits: Arc<Mutex<HashMap<String, KeyFetchHits>>>,
    /// `pending`（私人待收箱）的一次性挑战值：(uid, type) -> (nonce, 过期时间戳)。
    /// 为何存在：待收明细含发送方/金额/时间，**属私人数据**，不能仅凭 uid 枚举拉取；
    /// 客户端先取 nonce、用账户私钥签名后凭签名查询（见 api/client.rs::pending）。
    pub pending_nonce: Arc<Mutex<PendingNonces>>,
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
    pub role: String, // "root" | "admin"
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
        service_name: String,
    ) -> Self {
        AppState {
            db: Arc::new(Mutex::new(conn)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            central: Arc::new(Mutex::new(CentralState::default())),
            master: Arc::new(Mutex::new(None)),
            sys_acting: Arc::new(Mutex::new(HashMap::new())),
            gpg,
            token_ttl_secs: std::env::var("ACS_TOKEN_TTL_SECS")
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(600), // 默认 10 分钟待机
            data_dir,
            pre_issued,
            service_name,
            login_fails: Arc::new(Mutex::new(HashMap::new())),
            key_fetch_hits: Arc::new(Mutex::new(HashMap::new())),
            pending_nonce: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// 服务端额外表（后台管理员，两级：root / admin）。
pub const SERVER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS admins(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT UNIQUE NOT NULL,
    role TEXT NOT NULL DEFAULT 'admin',
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

-- v3.3.0（D8/D10）：主密钥的「按根管理员口令封装的副本」。
-- 不再有 master.key 文件；每行 = AES-256-GCM(nonce||ct hex)，密钥由该 Root 的登录口令派生。
-- 只有 Root 有行（Admin 不持有主密钥）。Root 改密时由 change_password 重封自己的行。
CREATE TABLE IF NOT EXISTS admin_master_key(
    admin_id INTEGER PRIMARY KEY,
    wrapped TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL);
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
