//! acs-server：A€ 中心服务器（网页后台管理）。
//!
//! 启动：迁移旧库 → 按密码策略种子管理员/系统账户（无 .env 时默认 admin；
//! 有 .env 时按文件定义创建，密码只存哈希）→ 启动 Web。

mod api;
mod auth;
mod crypto;
mod legal;
mod state;
mod sysw;
mod update;
mod web;

use acs_core::account;
use acs_core::config::CoreConfig;
use acs_core::db;
use acs_core::gpg::GpgUtil;
use acs_core::log;
use acs_core::models::{Account, AccountStatus, AccountType};
use axum::http::header;
use axum::Router;
use std::path::{Path, PathBuf};

use state::AppState;
use tower_http::{
    limit::RequestBodyLimitLayer,
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var("ACS_DATA_DIR").unwrap_or_else(|_| {
        // 服务器数据统一分类存放：~/.alpha_dir/acs-server
        acs_core::config::CoreConfig::default_alpha_dir()
            .join("acs-server")
            .to_string_lossy()
            .into_owned()
    });
    let cfg = CoreConfig::server_default(data_dir.clone());
    cfg.ensure_dirs()?;
    // 初始化 .alphalog（启动日志，data_dir 下）
    let _ = log::init(&cfg.data_dir);
    log::info(format!("acs-server 启动 v{} 数据目录: {}", acs_core::VERSION, cfg.data_dir.display()));
    println!("[acs-server] 数据目录: {}", cfg.data_dir.display());

    let (gpg_bin, gpg_src) = acs_core::gpg_detect::ensure_gpg()?;
    println!("[acs-server] gpg 来源: {:?} -> {}", gpg_src, gpg_bin.display());
    let gpg = GpgUtil::new(gpg_bin, cfg.gpg_homedir.clone());

    // 设置控制台窗口标题（Windows）
    set_console_title();

    let conn = db::open_db(&cfg.db_path)?;
    db::init_central(&conn)?;
    state::init_server_db(&conn)?;
    db::migrate_center(&conn)?;
    // 校验并输出发行账户（.env 的 PRE_ISSUED_ACCOUNT）；校验失败会返回 Err，拒绝启动。
    let pre_issued = seed_from_config(&conn, &gpg, &cfg)?;

    // 客户端更新包目录（updates/update.json + 安装包）
    let updates_dir = cfg.data_dir.join("updates");
    std::fs::create_dir_all(&updates_dir)?;
    let state = AppState::new(conn, gpg, cfg.data_dir.clone(), pre_issued);
    // 网络安全加固（两服务共用）：
    // - 请求体大小限制 4MB（防超大 payload；pubkey/sig 均远小于此）
    // - 请求超时 30s（防慢速攻击）
    // - 隐藏 hyper 默认 Server 头
    // - 安全响应头：防点击劫持 / MIME 嗅探 / 敏感信息泄露 / 页面缓存 / XSS
    let admin_app: Router = Router::new()
        .merge(web::routes())
        .merge(api::admin_routes())
        .layer(RequestBodyLimitLayer::new(4 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(30),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::SERVER,
            axum::http::HeaderValue::from_static("ACS"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            axum::http::HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            axum::http::HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            axum::http::HeaderValue::from_static(
                "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
            ),
        ))
        .layer(axum::middleware::from_fn(crate::api::log_request))
        .with_state(state.clone());

    // 公开服务：仅 client 所需端点（无后台管理）
    let public_app: Router = Router::new()
        .merge(api::public_routes())
        .layer(RequestBodyLimitLayer::new(4 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(30),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::SERVER,
            axum::http::HeaderValue::from_static("ACS"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            axum::http::HeaderValue::from_static("DENY"),
        ))
        .layer(axum::middleware::from_fn(crate::api::log_request))
        .with_state(state);

    let public_bind = std::env::var("ACS_PUBLIC_BIND").unwrap_or_else(|_| "0.0.0.0".into());
    let public_port = std::env::var("ACS_PUBLIC_PORT").unwrap_or_else(|_| "9600".into());
    // 后台管理默认仅本机；可通过 ACS_ADMIN_BIND=0.0.0.0 开放公网
    let admin_bind = std::env::var("ACS_ADMIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let admin_port = std::env::var("ACS_ADMIN_PORT").unwrap_or_else(|_| "9680".into());

    let public_listener =
        tokio::net::TcpListener::bind(format!("{public_bind}:{public_port}")).await?;
    let admin_listener =
        tokio::net::TcpListener::bind(format!("{admin_bind}:{admin_port}")).await?;

    println!("[acs-server] 公开 API（client）: http://{public_bind}:{public_port}");
    println!("[acs-server] 后台管理: http://{admin_bind}:{admin_port}");
    println!("[acs-server] 账户种子：见 {}/.env（无配置时默认 admin，初始密码见 SYSTEM_LOGIN_PASSWORDS.txt）", cfg.data_dir.display());

    let pub_handle = tokio::spawn(async move { axum::serve(public_listener, public_app).await });
    let adm_handle = tokio::spawn(async move { axum::serve(admin_listener, admin_app).await });
    pub_handle.await.map_err(|e| anyhow::anyhow!("公开服务异常：{e}"))??;
    adm_handle.await.map_err(|e| anyhow::anyhow!("管理服务异常：{e}"))??;
    Ok(())
}

/// 账户种子（v2.1.0 密码策略）

/// .env 定义的管理员账户种子。
struct AdminSeed {
    uid: String,
    role: String,
    pwd: String,
}

/// .env 定义的系统账户种子。
struct SystemSeed {
    uid: String,
    pwd: String,
}

/// 账户配置（来自各端数据目录 .env，如服务端 ~/.alpha_dir/acs-server/.env）。
struct EnvConfig {
    admins: Vec<AdminSeed>,
    systems: Vec<SystemSeed>,
    /// 发行账户（.env 的 PRE_ISSUED_ACCOUNT），必须为 systems 中唯一一员。
    pre_issued: Option<String>,
}

/// .env 文件路径：各端独立目录（服务端 ~/.alpha_dir/acs-server/.env）。
fn data_dir_env_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".env")
}

/// 读取各端数据目录下的 .env（服务端 ~/.alpha_dir/acs-server/.env）中的账户定义。无文件或无定义返回 None。
fn load_env_config(data_dir: &Path) -> Option<EnvConfig> {
    let content = std::fs::read_to_string(data_dir_env_path(data_dir)).ok()?;
    let mut admins = Vec::new();
    let mut systems = Vec::new();
    let mut pre_issued: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            // uid:role:密码，多个用逗号分隔
            "ACS_ADMIN_ACCOUNTS" => {
                for item in value.split(',') {
                    let parts: Vec<&str> = item.split(':').collect();
                    if parts.len() >= 3 {
                        admins.push(AdminSeed {
                            uid: parts[0].trim().to_string(),
                            role: parts[1].trim().to_string(),
                            pwd: parts[2].trim().to_string(),
                        });
                    }
                }
            }
            // uid:密码，多个用逗号分隔
            "ACS_SYSTEM_ACCOUNTS" => {
                for item in value.split(',') {
                    if let Some((u, p)) = item.split_once(':') {
                        systems.push(SystemSeed {
                            uid: u.trim().to_string(),
                            pwd: p.trim().to_string(),
                        });
                    }
                }
            }
            // 发行账户：只能设置一个，须为 ACS_SYSTEM_ACCOUNTS 之一
            "PRE_ISSUED_ACCOUNT" => {
                pre_issued = Some(value.to_string());
            }
            _ => {}
        }
    }
    if admins.is_empty() && systems.is_empty() {
        None
    } else {
        Some(EnvConfig { admins, systems, pre_issued })
    }
}

/// 统一种子入口：
/// - 无 .env：默认 admin（随机密码输出 txt），不创建系统账户，发行账户为空串
/// - 有 .env：禁用默认 admin，按 .env 创建管理员 + 系统账户（密码只存哈希，不输出 txt）；
///   并校验 PRE_ISSUED_ACCOUNT（须存在、唯一、且为 ACS_SYSTEM_ACCOUNTS 之一），否则拒绝启动。
/// 返回发行账户 uid（用于 mint/展示），无配置时为空串。
fn seed_from_config(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    cfg: &CoreConfig,
) -> anyhow::Result<String> {
    match load_env_config(&cfg.data_dir) {
        None => {
            seed_default_admin(conn, gpg, cfg)?;
            Ok(String::new())
        }
        Some(conf) => {
            validate_pre_issued(&conf)?;
            disable_default_admin(conn)?;
            seed_admins_from_env(conn, gpg, &conf.admins)?;
            seed_systems_from_env(conn, gpg, cfg, &conf.systems)?;
            Ok(conf.pre_issued.clone().unwrap_or_default())
        }
    }
}

/// 校验 PRE_ISSUED_ACCOUNT：非空、单个、且为 ACS_SYSTEM_ACCOUNTS 之一。
/// 失败时写 .alphalog 错误并返回 Err，拒绝启动。
fn validate_pre_issued(conf: &EnvConfig) -> anyhow::Result<()> {
    let Some(pre) = &conf.pre_issued else {
        // 无系统账户时允许“仅管理员”模式；有系统账户则必须配置发行账户。
        if conf.systems.is_empty() {
            return Ok(());
        }
        log::err("发行账户未配置：需在 .env 设置 PRE_ISSUED_ACCOUNT，拒绝启动");
        anyhow::bail!("PRE_ISSUED_ACCOUNT 未设置；必须在 ACS_SYSTEM_ACCOUNTS 中指定唯一发行账户");
    };
    let pre = pre.trim();
    if pre.is_empty() || pre.contains(',') {
        log::err(format!("发行账户配置非法：{pre}；只能设置一个"));
        anyhow::bail!("PRE_ISSUED_ACCOUNT=<账户名> 只能设置一个，不能为空或含逗号");
    }
    let ok = conf.systems.iter().any(|s| s.uid == pre);
    if !ok {
        log::err(format!("发行账户 {pre} 不在 ACS_SYSTEM_ACCOUNTS 中，拒绝启动"));
        anyhow::bail!("PRE_ISSUED_ACCOUNT={pre} 必须是 ACS_SYSTEM_ACCOUNTS 中的一员");
    }
    Ok(())
}

/// 设置控制台窗口标题（Windows）为 Alpha Coin Central Server。
#[cfg(target_os = "windows")]
fn set_console_title() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleTitleW(lpConsoleTitle: *const u16) -> i32;
    }
    let title: Vec<u16> = "Alpha Coin Central Server"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetConsoleTitleW(title.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
fn set_console_title() {}

/// 无配置：创建默认 admin（root），随机密码输出到 SYSTEM_LOGIN_PASSWORDS.txt。
fn seed_default_admin(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    cfg: &CoreConfig,
) -> anyhow::Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM admins", [], |r| r.get(0))?;
    if n > 0 {
        return Ok(());
    }
    let pwd = random_password();
    create_admin(conn, gpg, "admin", "root", &pwd)?;
    let path = cfg.data_dir.join("SYSTEM_LOGIN_PASSWORDS.txt");
    std::fs::write(&path, format!("admin={pwd}\n"))?;
    println!("[acs-server] 默认根管理员 admin 已创建，初始密码输出到 {}", path.display());
    Ok(())
}

/// 有配置：完全禁用默认 admin 账户。
fn disable_default_admin(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute("UPDATE admins SET status='Disabled' WHERE uid='admin'", [])?;
    Ok(())
}

/// 有配置：按 .env 创建管理员（已存在则跳过）。
fn seed_admins_from_env(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    admins: &[AdminSeed],
) -> anyhow::Result<()> {
    for a in admins {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM admins WHERE uid=?1",
            rusqlite::params![a.uid],
            |r| r.get(0),
        )?;
        if exists > 0 {
            continue;
        }
        create_admin(conn, gpg, &a.uid, &a.role, &a.pwd)?;
        let label = if a.role == "root" { "根管理员" } else { "管理员" };
        println!("[acs-server] {label} 已创建（密码只存哈希，首登须改密）");
    }
    Ok(())
}

/// 创建管理员（密码只存哈希，首登强制改密，生成管理员 gpg 密钥）。
fn create_admin(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    uid: &str,
    role: &str,
    pwd: &str,
) -> anyhow::Result<()> {
    let salt = uuid::Uuid::new_v4().to_string();
    let pw_hash = format!("{salt}${}", auth::hash_password(pwd, &salt));
    conn.execute(
        "INSERT INTO admins(uid, role, password_hash, must_change_password, pubkey, encrypted_seckey, fingerprint, key_passphrase_enc, status, created_at) \
         VALUES (?1,?2,?3,1,'','','','','Active',?4)",
        rusqlite::params![uid, role, pw_hash, chrono::Utc::now().timestamp()],
    )?;
    let id: i64 = conn.query_row("SELECT id FROM admins WHERE uid=?1", rusqlite::params![uid], |r| r.get(0))?;
    auth::ensure_admin_keys(conn, gpg, id, uid, pwd)?;
    Ok(())
}

fn random_password() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789!@#$%^&*";
    (0..16)
        .map(|_| CHARS[rand::random::<usize>() % CHARS.len()] as char)
        .collect()
}

/// 有配置：按 .env 创建系统账户（密码=私钥口令，登录凭证只存哈希，不输出 txt）。
fn seed_systems_from_env(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    cfg: &CoreConfig,
    systems: &[SystemSeed],
) -> anyhow::Result<()> {
    let alpha_dir = cfg.data_dir.clone();
    std::fs::create_dir_all(&alpha_dir)?;
    let master_key = ensure_master_key(cfg)?;
    for s in systems {
        if account::account_exists(conn, &s.uid, AccountType::System)? {
            continue;
        }
        let gk = gpg
            .generate_key(&format!("{} <{}@maxshin.top>", s.uid, s.uid), &s.pwd)
            .map_err(|e| anyhow::anyhow!("生成系统账户密钥失败 {}: {e}", s.uid))?;
        // 导出加密私钥 + 用主密钥加密 passphrase（本地备份，供技术人员审查）
        std::fs::write(alpha_dir.join(format!("{}.asc", s.uid)), &gk.encrypted_seckey)?;
        let key_enc = crypto::encrypt_secret(&master_key, &s.pwd);
        std::fs::write(alpha_dir.join(format!("{}.key", s.uid)), key_enc)?;
        // 创建账户
        account::create_account(
            conn,
            &Account {
                uid: s.uid.clone(),
                account_type: AccountType::System,
                email: format!("{}@maxshin.top", s.uid),
                pubkey: gk.pubkey,
                encrypted_seckey: gk.encrypted_seckey,
                balance: 0,
                status: AccountStatus::Active,
                last_tx_hash: None,
                created_at: chrono::Utc::now(),
                changed_at: chrono::Utc::now(),
            },
        )?;
        // 存登录凭证（只存哈希）
        let salt = uuid::Uuid::new_v4().to_string();
        let ph = format!("{salt}${}", auth::hash_password(&s.pwd, &salt));
        conn.execute(
            "INSERT INTO account_credentials(uid, type, password_hash) VALUES (?1,'System',?2) \
             ON CONFLICT(uid,type) DO UPDATE SET password_hash=excluded.password_hash",
            rusqlite::params![s.uid, ph],
        )?;
        println!("[acs-server] 系统账户已创建: {}（登录凭证只存哈希）", s.uid);
    }
    Ok(())
}

/// 数据目录主密钥（用于加密系统账户密钥 passphrase 文件）。
fn ensure_master_key(cfg: &CoreConfig) -> anyhow::Result<String> {
    let path = cfg.data_dir.join("master.key");
    if path.exists() {
        return Ok(std::fs::read_to_string(&path)?);
    }
    let key = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    std::fs::write(&path, &key)?;
    Ok(key)
}
