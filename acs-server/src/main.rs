//! acs-server：A€ 中心服务器（网页后台管理）。
//!
//! 启动：迁移旧库 → 种子默认根管理员（max_shin/mitra）→ 种子系统账户
//! （PreIssuedAccount/AESystem/AlphaEU，导出私钥到 ./alpha_dir）→ 启动 Web。

mod api;
mod auth;
mod crypto;
mod state;
mod web;

use acs_core::account;
use acs_core::config::CoreConfig;
use acs_core::db;
use acs_core::gpg::GpgUtil;
use acs_core::models::{Account, AccountStatus, AccountType};
use axum::http::header;
use axum::Router;

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
    let cfg = CoreConfig::server_default(data_dir);
    cfg.ensure_dirs()?;
    println!("[acs-server] 数据目录: {}", cfg.data_dir.display());

    let (gpg_bin, gpg_src) = acs_core::gpg_detect::ensure_gpg()?;
    println!("[acs-server] gpg 来源: {:?} -> {}", gpg_src, gpg_bin.display());
    let gpg = GpgUtil::new(gpg_bin, cfg.gpg_homedir.clone());

    let conn = db::open_db(&cfg.db_path)?;
    db::init_central(&conn)?;
    state::init_server_db(&conn)?;
    db::migrate_center(&conn)?;
    seed_default_admins(&conn, &gpg)?;
    seed_system_accounts(&conn, &gpg, &cfg)?;
    init_system_credentials(&conn, &cfg)?;

    let state = AppState::new(conn, gpg);
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
        .with_state(state.clone());

    // 公开服务：仅 client / mirror 所需端点（apikey 认证），无后台管理
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
        .with_state(state);

    let public_bind = std::env::var("ACS_PUBLIC_BIND").unwrap_or_else(|_| "0.0.0.0".into());
    let public_port = std::env::var("ACS_PUBLIC_PORT").unwrap_or_else(|_| "9600".into());
    // 后台管理默认仅本机（不开放公网）
    let admin_bind = std::env::var("ACS_ADMIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let admin_port = std::env::var("ACS_ADMIN_PORT").unwrap_or_else(|_| "9680".into());

    let public_listener =
        tokio::net::TcpListener::bind(format!("{public_bind}:{public_port}")).await?;
    let admin_listener =
        tokio::net::TcpListener::bind(format!("{admin_bind}:{admin_port}")).await?;

    println!("[acs-server] 公开 API（client/mirror）: http://{public_bind}:{public_port}");
    println!("[acs-server] 后台管理（仅内网）: http://{admin_bind}:{admin_port}");
    println!("[acs-server] 默认根管理员: max_shin / mitra（首次登录须改密）");

    let pub_handle = tokio::spawn(async move { axum::serve(public_listener, public_app).await });
    let adm_handle = tokio::spawn(async move { axum::serve(admin_listener, admin_app).await });
    pub_handle.await.map_err(|e| anyhow::anyhow!("公开服务异常：{e}"))??;
    adm_handle.await.map_err(|e| anyhow::anyhow!("管理服务异常：{e}"))??;
    Ok(())
}

/// 首次启动种子两个根管理员：max_shin（理事长）、mitra（副理事长）。
fn seed_default_admins(conn: &rusqlite::Connection, gpg: &GpgUtil) -> anyhow::Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM admins", [], |r| r.get(0))?;
    if n > 0 {
        return Ok(());
    }
    // 初始密码：优先环境变量 ACS_ADMIN_PASSWORD；未设置则随机生成并在日志打印一次
    // （安全：源码不硬编码密码；首次登录强制改密 must_change_password=1）
    let default_pwd = std::env::var("ACS_ADMIN_PASSWORD").unwrap_or_else(|_| {
        const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789!@#$%^&*";
        let p: String = (0..16)
            .map(|_| CHARS[rand::random::<usize>() % CHARS.len()] as char)
            .collect();
        println!("[acs-server] 未设置 ACS_ADMIN_PASSWORD，随机初始密码: {p}（首次登录须改密）");
        p
    });
    for uid in ["max_shin", "mitra"] {
        let salt = uuid::Uuid::new_v4().to_string();
        let pw = auth::hash_password(&default_pwd, &salt);
        let pw_hash = format!("{salt}${pw}");
        conn.execute(
            "INSERT INTO admins(uid, role, password_hash, must_change_password, pubkey, encrypted_seckey, fingerprint, key_passphrase_enc, status, created_at) \
             VALUES (?1,'root',?2,1,'','','','','Active',?3)",
            rusqlite::params![uid, pw_hash, chrono::Utc::now().timestamp()],
        )?;
        let id: i64 = conn.query_row("SELECT id FROM admins WHERE uid=?1", rusqlite::params![uid], |r| r.get(0))?;
        auth::ensure_admin_keys(conn, gpg, id, uid, &default_pwd)?;
        println!("[acs-server] 根管理员已创建: {uid}");
    }
    Ok(())
}

/// 种子系统账户：PreIssuedAccount / AESystem / AlphaEU（首启自动生成密钥并导出私钥到数据目录）。
fn seed_system_accounts(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    cfg: &CoreConfig,
) -> anyhow::Result<()> {
    // 系统账户私钥统一导出到服务器数据目录（~/.alpha_dir/acs-server）
    let alpha_dir = cfg.data_dir.clone();
    std::fs::create_dir_all(&alpha_dir)?;
    // 生成主密钥（用于加密系统账户 passphrase 文件）
    let master_key = ensure_master_key(cfg)?;

    for (uid, kind) in [
        ("PreIssuedAccount", "MintTarget"),
        ("AESystem", "StockSystem"),
        ("AlphaEU", "OrgPool"),
    ] {
        if account::account_exists(conn, uid, AccountType::System)? {
            continue;
        }
        let pass = uuid::Uuid::new_v4().to_string();
        let gk = gpg
            .generate_key(&format!("{uid} <{uid}@maxshin.top>"), &pass)
            .map_err(|e| anyhow::anyhow!("生成系统账户密钥失败 {uid}: {e}"))?;
        // 导出私钥到 ./alpha_dir
        let asc = alpha_dir.join(format!("{uid}.asc"));
        std::fs::write(&asc, &gk.encrypted_seckey)?;
        // 加密保存 passphrase（用主密钥）
        let key_enc = crypto::encrypt_secret(&master_key, &pass);
        std::fs::write(alpha_dir.join(format!("{uid}.key")), key_enc)?;

        account::create_account(
            conn,
            &Account {
                uid: uid.to_string(),
                account_type: AccountType::System,
                email: format!("{uid}@maxshin.top"),
                pubkey: gk.pubkey,
                encrypted_seckey: gk.encrypted_seckey,
                balance: 0,
                status: AccountStatus::Active,
                last_tx_hash: None,
                created_at: chrono::Utc::now(),
                changed_at: chrono::Utc::now(),
            },
        )?;
        println!("[acs-server] 系统账户已创建: {uid} ({kind}), 私钥已导出到 {}", asc.display());
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

/// 初始化系统账户登录凭证：用现有私钥 passphrase 作为登录密码（密码=私钥口令，
/// 与一般账户一致，登录后可直接签名），写入 account_credentials；密码输出到
/// 数据目录 SYSTEM_LOGIN_PASSWORDS.txt（仅首次初始化时写一次）。
fn init_system_credentials(
    conn: &rusqlite::Connection,
    cfg: &CoreConfig,
) -> anyhow::Result<()> {
    let master_key = ensure_master_key(cfg)?;
    let mut pw_lines = String::new();
    for uid in ["PreIssuedAccount", "AESystem", "AlphaEU"] {
        // 已有登录凭证则跳过
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM account_credentials WHERE uid=?1 AND type='System'",
            rusqlite::params![uid],
            |r| r.get(0),
        )?;
        if exists > 0 {
            continue;
        }
        // 用现有私钥 passphrase 作为登录密码（读 .key 用主密钥解密）
        let key_enc = std::fs::read_to_string(cfg.data_dir.join(format!("{uid}.key")))?;
        let pwd = crypto::decrypt_secret(&key_enc, &master_key)
            .map_err(|e| anyhow::anyhow!("解密系统账户 {uid} passphrase 失败: {e}"))?;
        // 存登录凭证（$salt$sha256）
        let salt = uuid::Uuid::new_v4().to_string();
        let ph = format!("{salt}${}", auth::hash_password(&pwd, &salt));
        conn.execute(
            "INSERT INTO account_credentials(uid, type, password_hash) VALUES (?1,'System',?2) \
             ON CONFLICT(uid,type) DO UPDATE SET password_hash=excluded.password_hash",
            rusqlite::params![uid, ph],
        )?;
        pw_lines.push_str(&format!("{uid}={pwd}\n"));
        println!("[acs-server] 系统账户登录凭证已初始化: {uid}");
    }
    if !pw_lines.is_empty() {
        let path = cfg.data_dir.join("SYSTEM_LOGIN_PASSWORDS.txt");
        std::fs::write(&path, pw_lines)?;
        println!("[acs-server] 系统账户登录密码已写入: {}", path.display());
    }
    Ok(())
}
