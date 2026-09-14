//! SQLite 连接与建库（中心库 / 客户端库）。
//!
//! 账户分表：accounts_country / accounts_company / accounts_individual / accounts_system（系统账户：PreIssuedAccount/AESystem/AlphaEU）。
//! 管理员：admins（root/finance 两级，密钥内置）。成员注册表：member_countries / member_companies。
//! 交易：transactions（统一总账，确认时间/拒收理由内联，不再另建确认表）。

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

-- 账本账户分表（无 abbr；UID 为唯一识别符）
-- 注：不保存任何口令哈希（私钥由用户口令加密，口令不落库）；仅保留同意标识与最近登录时间。
CREATE TABLE IF NOT EXISTS accounts_country(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL,
    last_login INTEGER NOT NULL DEFAULT 0,
    agree_terms INTEGER NOT NULL DEFAULT 0, agree_privacy INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS accounts_company(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL,
    last_login INTEGER NOT NULL DEFAULT 0,
    agree_terms INTEGER NOT NULL DEFAULT 0, agree_privacy INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS accounts_individual(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL,
    last_login INTEGER NOT NULL DEFAULT 0,
    agree_terms INTEGER NOT NULL DEFAULT 0, agree_privacy INTEGER NOT NULL DEFAULT 0);
-- 系统账户（PreIssuedAccount / AESystem / AlphaEU）
-- 注：密钥材料只入库，不再向数据目录导出 .asc/.key 文件；
--     key_passphrase_enc = 系统账户私钥口令的密文（AES-GCM，密钥为数据目录 master.key），仅服务端代管签名时使用。
--     ledger_pw_enc      = 账本访问口令的密文（后台进入该账本时输入的口令，同样用 master.key 封存，**不存哈希**）；
--                          留空 = 旧库遗留，回退为校验 key_passphrase_enc 解出的私钥口令（登录成功后自动补写）。
--                          访问口令与私钥口令分离：改访问口令不影响中心侧签名能力。
--     must_change_password = 1 表示首次登录必须修改账本口令（种子创建时置 1）。
CREATE TABLE IF NOT EXISTS accounts_system(
    uid TEXT PRIMARY KEY, email TEXT NOT NULL,
    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL,
    last_login INTEGER NOT NULL DEFAULT 0,
    agree_terms INTEGER NOT NULL DEFAULT 0, agree_privacy INTEGER NOT NULL DEFAULT 0,
    key_passphrase_enc TEXT NOT NULL DEFAULT '',
    ledger_pw_enc TEXT NOT NULL DEFAULT '',
    must_change_password INTEGER NOT NULL DEFAULT 0);

-- 统一交易总账
-- 时间语义：ts = 客户端声明时间（保留原值，参与 tx_hash）；received_at = 服务端收到时间（权威、不可伪造）。
-- 签名语义：central_sig = 根管理员对 tx_hash 的分离签名（**仅 Mint**）；
--          receiver_sig = 接收方对 tx_id 的确认签名（Transfer/Issue/Redeem；后台代管确认为 NULL）。
CREATE TABLE IF NOT EXISTS transactions(
    tx_id TEXT PRIMARY KEY, tx_type TEXT NOT NULL,
    sender TEXT NOT NULL, sender_type TEXT NOT NULL,
    receiver TEXT NOT NULL, receiver_type TEXT NOT NULL,
    amount INTEGER NOT NULL, ts INTEGER NOT NULL,
    received_at INTEGER NOT NULL DEFAULT 0,
    tx_hash TEXT NOT NULL, sender_sig TEXT NOT NULL,
    central_sig TEXT, receiver_sig TEXT,
    sender_last_hash TEXT, receiver_last_hash TEXT,
    status TEXT NOT NULL DEFAULT 'Pending',
    confirmed_at INTEGER, reject_reason TEXT);
CREATE INDEX IF NOT EXISTS idx_tx_sender ON transactions(sender, sender_type);
CREATE INDEX IF NOT EXISTS idx_tx_receiver ON transactions(receiver, receiver_type);
-- 每笔交易的哈希必须唯一（tx_hash 含随机 tx_id，正常不会重复；唯一索引使重复哈希直接写入失败）
CREATE UNIQUE INDEX IF NOT EXISTS idx_tx_hash ON transactions(tx_hash);

-- 管理审计日志
CREATE TABLE IF NOT EXISTS audit_log(
    id INTEGER PRIMARY KEY AUTOINCREMENT, actor TEXT NOT NULL, op TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '', ts INTEGER NOT NULL);
"#;

/// 客户端库 schema（核心表；账户类表见 acs-client wallet.rs::CLIENT_SCHEMA）。
/// 注：客户端**不保存账本副本**（local_ledger 已取消）；账本按需从中心拉取（见 acs-client sync::fetch_ledger）。
/// 注：历史登录账户（含 uid/公钥/加密私钥）统一存于 `local_accounts` 一张表，
///     原 `keys`/`login_history` 两表已并入其中（迁移见 acs-client wallet.rs::migrate_local_v2）。
pub const LOCAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS known_pubkeys(
    uid TEXT NOT NULL, type TEXT NOT NULL, pubkey TEXT NOT NULL, source TEXT NOT NULL,
    PRIMARY KEY(uid, type));
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
    // 客户端不再保留账本副本：清掉历史遗留的 local_ledger 表（若存在）
    conn.execute_batch("DROP TABLE IF EXISTS local_ledger;")?;
    // keys / login_history 的数据迁移由 wallet::migrate_local_v2 处理（先搬后删）
    Ok(())
}

/// 若本地表缺列则 ALTER TABLE 补列（SQLite 老库升级）。
#[allow(dead_code)]
fn ensure_col(conn: &Connection, table: &str, col: &str, decl: &str) -> Result<()> {
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
    // accounts_bank -> accounts_company（企业账户重命名，数据迁移）
    // 注意：init_central 可能已按新 schema 创建空的 accounts_company，因此这里
    // 只要存在 accounts_bank 就要合并数据并删除旧表（OR IGNORE 防重复）。
    if table_exists(conn, "accounts_bank") {
        if !table_exists(conn, "accounts_company") {
            conn.execute_batch(
                "CREATE TABLE accounts_company(\n\
                    uid TEXT PRIMARY KEY, email TEXT NOT NULL,\n\
                    pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,\n\
                    balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',\n\
                    last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);",
            )?;
        }
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts_company(uid,email,pubkey,encrypted_seckey,balance,status,last_tx_hash,created_at,changed_at)\n\
                SELECT uid,email,pubkey,encrypted_seckey,balance,status,last_tx_hash,created_at,changed_at FROM accounts_bank;\n\
             DROP TABLE accounts_bank;",
        )?;
    }
    // 类型字符串迁移：交易中的 'Bank' -> 'Company'（企业账户重命名）
    conn.execute("UPDATE transactions SET sender_type='Company' WHERE sender_type='Bank'", [])?;
    conn.execute("UPDATE transactions SET receiver_type='Company' WHERE receiver_type='Bank'", [])?;
    // 废弃表：老版本遗留（mirror_* 为已取消的只读镜像；reserve/email_codes 从未启用）
    for t in [
        "member_towns", "central_keys", "pending_registrations",
        "mirror_keys", "mirror_registry", "reserve", "email_codes",
    ] {
        if table_exists(conn, t) {
            conn.execute(&format!("DROP TABLE IF EXISTS {t}"), [])?;
        }
    }
    // 删除账本账户表的 abbr 列
    for t in ["accounts_country", "accounts_company", "accounts_individual", "accounts_system"] {
        if column_exists(conn, t, "abbr") {
            conn.execute(&format!("ALTER TABLE {t} DROP COLUMN abbr"), [])?;
        }
    }
    // 账户表补列：同意标识与最近登录时间
    for t in ["accounts_country", "accounts_company", "accounts_individual", "accounts_system"] {
        ensure_col(conn, t, "last_login", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_col(conn, t, "agree_terms", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_col(conn, t, "agree_privacy", "INTEGER NOT NULL DEFAULT 0")?;
    }
    // 系统账户：口令密文列（密钥材料只入库，不再导出 .asc/.key 文件）
    ensure_col(conn, "accounts_system", "key_passphrase_enc", "TEXT NOT NULL DEFAULT ''")?;
    // v3.1.0：系统账户账本访问口令（封存）+ 首次登录强制改密标记
    // 注：不使用 password_hash 列 —— 本版的原则是不保存任何口令哈希，
    //     这里与 key_passphrase_enc 一样只存 AES-GCM 密文（密钥为 master.key）。
    ensure_col(conn, "accounts_system", "ledger_pw_enc", "TEXT NOT NULL DEFAULT ''")?;
    ensure_col(
        conn,
        "accounts_system",
        "must_change_password",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    // 合并 account_credentials → accounts_*（老库升级），随后删除旧表
    // 注：旧表里的 password_hash 一律丢弃（不再保存任何口令哈希）
    if table_exists(conn, "account_credentials") {
        ensure_col(conn, "account_credentials", "last_login", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_col(conn, "account_credentials", "agree_terms", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_col(conn, "account_credentials", "agree_privacy", "INTEGER NOT NULL DEFAULT 0")?;
        // 老库类型字符串归一（企业账户重命名 Bank -> Company）
        conn.execute("UPDATE account_credentials SET type='Company' WHERE type='Bank'", [])?;
        for (t, ty) in [
            ("accounts_country", "Country"),
            ("accounts_company", "Company"),
            ("accounts_individual", "Individual"),
            ("accounts_system", "System"),
        ] {
            conn.execute(
                &format!(
                    "UPDATE {t} SET \
                       last_login=IFNULL((SELECT c.last_login FROM account_credentials c WHERE c.uid={t}.uid AND c.type=?1), last_login), \
                       agree_terms=IFNULL((SELECT c.agree_terms FROM account_credentials c WHERE c.uid={t}.uid AND c.type=?1), agree_terms), \
                       agree_privacy=IFNULL((SELECT c.agree_privacy FROM account_credentials c WHERE c.uid={t}.uid AND c.type=?1), agree_privacy)"
                ),
                rusqlite::params![ty],
            )?;
        }
        conn.execute("DROP TABLE account_credentials", [])?;
    }
    // 移除历史遗留的 password_hash 列（不再保存任何口令哈希）
    for t in ["accounts_country", "accounts_company", "accounts_individual", "accounts_system"] {
        if column_exists(conn, t, "password_hash") {
            let _ = conn.execute(&format!("ALTER TABLE {t} DROP COLUMN password_hash"), []);
        }
    }
    // 交易表补列：服务端收到时间 / 接收方确认签名 / 确认时间 / 拒收理由
    ensure_col(conn, "transactions", "received_at", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_col(conn, "transactions", "receiver_sig", "TEXT")?;
    ensure_col(conn, "transactions", "confirmed_at", "INTEGER")?;
    ensure_col(conn, "transactions", "reject_reason", "TEXT")?;
    // 交易哈希唯一索引（老库若存在历史重复哈希则创建失败，此处只记录不中断启动）
    if let Err(e) = conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_tx_hash ON transactions(tx_hash)",
        [],
    ) {
        crate::log::err(format!(
            "交易哈希唯一索引创建失败（可能存在历史重复哈希，请人工核查）：{e}"
        ));
    }
    // 合并 tx_confirmations → transactions（老库升级），随后删除旧表：
    // 该表的 confirmed 列与 transactions.status 冗余（且从无任何查询读取），
    // 真正有价值的 reject_reason/confirmed_at 已并入主表。
    if table_exists(conn, "tx_confirmations") {
        conn.execute(
            "UPDATE transactions SET \
               confirmed_at=IFNULL((SELECT c.confirmed_at FROM tx_confirmations c WHERE c.tx_id=transactions.tx_id), confirmed_at), \
               reject_reason=IFNULL((SELECT c.reject_reason FROM tx_confirmations c WHERE c.tx_id=transactions.tx_id), reject_reason) \
             WHERE EXISTS(SELECT 1 FROM tx_confirmations c WHERE c.tx_id=transactions.tx_id)",
            [],
        )?;
        conn.execute("DROP TABLE tx_confirmations", [])?;
    }
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
