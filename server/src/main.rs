//! AlphaCoinServer（crate `acs-server`）：A€ 中心服务器（网页后台管理）。
//!
//! 启动：迁移旧库 → 按密码策略种子管理员/系统账户（无 .env 时默认 admin；
//! 有 .env 时按文件定义创建，密码只存哈希）→ 启动 Web。

mod api;
mod auth;
mod crypto;
mod legal;
mod password;
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
use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};

use state::AppState;
use tower_http::{
    limit::RequestBodyLimitLayer,
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
};

/// 读取命令行参数（形如 `-homedir X` 或 `--homedir=X`）。
fn arg_value(flags: &[&str]) -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        for f in flags {
            if a == f {
                if let Some(v) = args.get(i + 1) {
                    return Some(v.clone());
                }
            }
            if let Some(v) = a.strip_prefix(&format!("{f}=")) {
                return Some(v.to_string());
            }
        }
        i += 1;
    }
    None
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // v3.3.0（#9）：服务端数据目录优先级
    //   1) 命令行 `-homedir <路径>`（面板启动各服务时传入，见 §13）
    //   2) 环境变量 `ACS_DATA_DIR`（运维 / 测试显式指定）
    //   3) `~/.alphacoinserver/<服务域名>`（域名取自 ACS_PUBLIC_URL；未知则 `default`）
    let data_dir = match arg_value(&["-homedir", "--homedir"]) {
        Some(p) => PathBuf::from(p),
        None => match std::env::var("ACS_DATA_DIR") {
            Ok(d) if !d.trim().is_empty() => PathBuf::from(d),
            _ => {
                let key = std::env::var("ACS_PUBLIC_URL")
                    .ok()
                    .map(|u| acs_core::config::domain_key(&u))
                    .unwrap_or_else(|| "default".to_string());
                acs_core::config::server_root().join(key)
            }
        },
    };
    let cfg = CoreConfig::server_default(data_dir.clone());
    cfg.ensure_dirs()?;
    // 把数据目录 .env 注入进程环境：使端口 / 品牌 / 迭代次数 / 更新源等
    // 全部可由 .env 定制（已存在的真实环境变量优先，不被覆盖）。
    // 必须在读取任何配置之前调用（见 config::load_env_file 的调用时机说明）。
    let env_loaded = acs_core::config::load_env_file(&cfg.data_dir);
    // 初始化日志（v3.3.0：`<数据目录>/logs/*.log`，按服务域名分目录）
    let _ = log::init(&cfg.data_dir.join("logs"));
    log::info(format!("AlphaCoinServer 启动 v{} 数据目录: {}", acs_core::VERSION, cfg.data_dir.display()));
    println!("[AlphaCoinServer] 数据目录: {}", cfg.data_dir.display());
    if env_loaded > 0 {
        println!("[AlphaCoinServer] 已从 .env 载入 {env_loaded} 项配置");
    }

    let (gpg_bin, gpg_src) = acs_core::gpg_detect::ensure_gpg()?;
    println!("[AlphaCoinServer] gpg 来源: {:?} -> {}", gpg_src, gpg_bin.display());
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
    // 服务名称：面板以 `-name <名称>` 传入（D26）；不写入 .env / 服务包
    let service_name = arg_value(&["-name", "--name"]).unwrap_or_default();
    let state = AppState::new(conn, gpg, cfg.data_dir.clone(), pre_issued, service_name);
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

    println!("[AlphaCoinServer] 公开 API（client）: http://{public_bind}:{public_port}");
    println!("[AlphaCoinServer] 后台管理: http://{admin_bind}:{admin_port}");
    println!("[AlphaCoinServer] 账户种子：见 {}/.env（Root 账户由 ACS_ROOT_ACCOUNTS 定义，首登须改密）", cfg.data_dir.display());

    let pub_handle = tokio::spawn(async move { axum::serve(public_listener, public_app).await });
    let adm_handle = tokio::spawn(async move { axum::serve(admin_listener, admin_app).await });
    pub_handle.await.map_err(|e| anyhow::anyhow!("公开服务异常：{e}"))??;
    adm_handle.await.map_err(|e| anyhow::anyhow!("管理服务异常：{e}"))??;
    Ok(())
}

/// 账户种子（v2.1.0 密码策略）

/// .env 定义的 **Root 账户**种子（v3.3.0 D18/D29：只有一个角色，不再带 role 字段）。
///
/// 格式：`ACS_ROOT_ACCOUNTS=uid1:密码1,uid2:密码2`
/// - 密码在第一个 `:` 之后的部分，**可以包含冒号**，但不能包含逗号（逗号是条目分隔符）。
/// - 首启后 Root 账户**不可增减**（D17），本配置只用于首次建库。
struct RootSeed {
    uid: String,
    pwd: String,
}

/// .env 定义的系统账户种子。
struct SystemSeed {
    uid: String,
    pwd: String,
}

/// 账户配置（来自服务端数据目录 .env，如 ~/.alphacoinserver/<服务域名>/.env）。
struct EnvConfig {
    roots: Vec<RootSeed>,
    systems: Vec<SystemSeed>,
    /// 发行账户（.env 的 PRE_ISSUED_ACCOUNT），必须为 systems 中唯一一员。
    pre_issued: Option<String>,
}

/// .env 文件路径：服务端数据目录（~/.alphacoinserver/<服务域名>/.env）。
fn data_dir_env_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".env")
}

/// 读取各端数据目录下的 .env（服务端数据目录/.env）中的账户定义。无文件返回 None。
fn load_env_config(data_dir: &Path) -> Option<EnvConfig> {
    let content = std::fs::read_to_string(data_dir_env_path(data_dir)).ok()?;
    let mut roots = Vec::new();
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
            // v3.3.0（D18）：ACS_ROOT_ACCOUNTS = uid:密码，多个用逗号分隔（密码中不可含逗号）
            "ACS_ROOT_ACCOUNTS" => {
                for item in value.split(',') {
                    if let Some((u, p)) = item.split_once(':') {
                        let (u, p) = (u.trim(), p.trim());
                        if !u.is_empty() && !p.is_empty() {
                            roots.push(RootSeed {
                                uid: u.to_string(),
                                pwd: p.to_string(),
                            });
                        }
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
    if roots.is_empty() && systems.is_empty() {
        None
    } else {
        Some(EnvConfig { roots, systems, pre_issued })
    }
}

/// 统一种子入口（v3.3.0 D18/D29）：
/// - **必须有 `.env` 且配置了 `ACS_ROOT_ACCOUNTS`（至少一个 Root）**，否则**拒绝启动**；
///   不再创建任何默认管理员账号。
/// - 按 `.env` 创建 **Root 账户** + 系统账户（密码只存哈希、首登强制改密、不输出 txt）；
///   并校验 PRE_ISSUED_ACCOUNT（须存在、唯一、且为 ACS_SYSTEM_ACCOUNTS 之一）。
/// - **Admin（原 Finance）不再由 .env 创建**，只能由 Root 在管理后台创建（D18）。
/// 返回发行账户 uid（用于 mint/展示）。
fn seed_from_config(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    cfg: &CoreConfig,
) -> anyhow::Result<String> {
    let env_path = data_dir_env_path(&cfg.data_dir);
    let conf = match load_env_config(&cfg.data_dir) {
        Some(c) => c,
        None => {
            log::err(format!(
                "未找到账户配置：{} 不存在或未配置 ACS_ROOT_ACCOUNTS，拒绝启动",
                env_path.display()
            ));
            anyhow::bail!(
                "必须配置 ACS_ROOT_ACCOUNTS（至少一个根管理员）后才能启动：\
                 请在 {} 中写入 ACS_ROOT_ACCOUNTS=uid:密码",
                env_path.display()
            );
        }
    };
    if conf.roots.is_empty() {
        log::err(format!("ACS_ROOT_ACCOUNTS 为空：{}，拒绝启动", env_path.display()));
        anyhow::bail!(
            "ACS_ROOT_ACCOUNTS 为空，至少需要一个根管理员：请在 {} 中配置 uid:密码",
            env_path.display()
        );
    }
    validate_pre_issued(&conf)?;
    seed_roots_from_env(conn, gpg, &conf.roots)?;
    // 主密钥：首次建库时生成并按 Root 口令封装副本；此后冷启动保持「未解锁」（D15）
    let master = ensure_master_key(conn, &conf.roots)?;
    seed_systems_from_env(conn, gpg, cfg, &conf.systems, master.as_deref())?;
    Ok(conf.pre_issued.clone().unwrap_or_default())
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
    let title: Vec<u16> = acs_core::brand::brand()
        .server_app
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetConsoleTitleW(title.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
fn set_console_title() {}

/// 按 .env 的 `ACS_ROOT_ACCOUNTS` 创建 **Root 账户**（已存在则跳过）。
///
/// v3.3.0（D17/D18）：Root 只在此处（首次建库）创建，此后**不可增减**。
fn seed_roots_from_env(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    roots: &[RootSeed],
) -> anyhow::Result<()> {
    for a in roots {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM admins WHERE uid=?1",
            rusqlite::params![a.uid],
            |r| r.get(0),
        )?;
        if exists > 0 {
            continue;
        }
        create_root_admin(conn, gpg, &a.uid, &a.pwd)?;
        println!("[AlphaCoinServer] 根管理员 {} 已创建（密码只存哈希，首登须改密）", a.uid);
    }
    Ok(())
}

/// 创建 **Root 账户**（密码只存哈希，首登强制改密，生成 gpg 密钥对）。
///
/// v3.3.0（D28）：只有 Root 需要 gpg 密钥（用于 Mint / 创建 Admin 时签名）；
/// Admin 由管理后台创建且**不生成密钥**（见 `api/admins.rs`）。
fn create_root_admin(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    uid: &str,
    pwd: &str,
) -> anyhow::Result<()> {
    let pw_hash = password::hash(pwd);
    conn.execute(
        "INSERT INTO admins(uid, role, password_hash, must_change_password, pubkey, encrypted_seckey, fingerprint, key_passphrase_enc, status, created_at) \
         VALUES (?1,'root',?2,1,'','','','','Active',?3)",
        rusqlite::params![uid, pw_hash, chrono::Utc::now().timestamp()],
    )?;
    let id: i64 = conn
        .query_row("SELECT id FROM admins WHERE uid=?1", rusqlite::params![uid], |r| r.get(0))?;
    auth::ensure_admin_keys(conn, gpg, id, uid, pwd)?;
    Ok(())
}

/// 有配置：按 .env 创建系统账户（密码=私钥口令，登录凭证只存哈希，不输出 txt）。
///
/// `master` 为本次启动新生成的主密钥（首次建库时 `Some`）。若主密钥已存在但未解锁
/// （`None`，冷启动）且仍有系统账户待创建，则无法封存其口令 → 记错误并跳过，
/// 需由根管理员登录解锁后重建这些账户（正常流程下首次建库时即已全部创建）。
fn seed_systems_from_env(
    conn: &rusqlite::Connection,
    gpg: &GpgUtil,
    cfg: &CoreConfig,
    systems: &[SystemSeed],
    master: Option<&str>,
) -> anyhow::Result<()> {
    let alpha_dir = cfg.data_dir.clone();
    std::fs::create_dir_all(&alpha_dir)?;
    for s in systems {
        if account::account_exists(conn, &s.uid, AccountType::System)? {
            continue;
        }
        let Some(master_key) = master else {
            log::err(format!(
                "系统账户 {} 尚未创建，但主密钥当前未解锁，无法封存其口令；请先由根管理员登录后再处理",
                s.uid
            ));
            continue;
        };
        let gk = gpg
            .generate_key(&acs_core::brand::brand().system_gpg_uid(&s.uid), &s.pwd)
            .map_err(|e| anyhow::anyhow!("生成系统账户密钥失败 {}: {e}", s.uid))?;
        // 不再向数据目录导出任何密钥文件（.asc / .key）：
        //   加密私钥 → 入库（accounts_system.encrypted_seckey）
        //   口令密文 → 入库（accounts_system.key_passphrase_enc，AES-GCM，密钥为主密钥）
        let key_enc = crypto::encrypt_secret(master_key, &s.pwd);
        // 账本访问口令：与私钥口令分开封存（首次登录必须修改）
        let ledger_enc = crypto::encrypt_secret(master_key, &s.pwd);
        // 创建账户
        account::create_account(
            conn,
            &Account {
                uid: s.uid.clone(),
                account_type: AccountType::System,
                fullname: String::new(),
                email: acs_core::brand::brand().system_email(&s.uid),
                pubkey: gk.pubkey,
                encrypted_seckey: gk.encrypted_seckey,
                balance: 0,
                status: AccountStatus::Active,
                // 链头初值必须与客户端侧一致（创世种子），否则首笔交易会被判「链头不一致」
                last_tx_hash: Some(acs_core::transaction::account_chain_seed(
                    &s.uid,
                    AccountType::System,
                )),
                created_at: chrono::Utc::now(),
                changed_at: chrono::Utc::now(),
            },
        )?;
        conn.execute(
            "UPDATE accounts_system SET key_passphrase_enc=?2, ledger_pw_enc=?3, must_change_password=1 WHERE uid=?1",
            rusqlite::params![s.uid, key_enc, ledger_enc],
        )?;
        println!(
            "[AlphaCoinServer] 系统账户已创建: {}（密钥材料仅入库，不导出文件；客户端不可登录；首次登录须修改账本口令）",
            s.uid
        );
    }
    Ok(())
}

/// 主密钥初始化（v3.3.0 D8/D10）：**不再落任何明文文件**。
///
/// 规则：
/// - 若 `admin_master_key` 已有副本 → 主密钥已初始化，**不载入内存**，返回 `None`
///   （D15：冷启动后必须由根管理员登录解锁，见 `auth::try_unlock_master`）。
/// - 若无副本（首次建库）→ 生成 32 字节级随机主密钥，为 `.env` 里每个 Root 各封装一份副本
///   （此时握有其初始明文口令），返回 `Some(key)` 供本进程首启播种系统账户使用。
fn ensure_master_key(
    conn: &rusqlite::Connection,
    roots: &[RootSeed],
) -> anyhow::Result<Option<String>> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM admin_master_key", [], |r| r.get(0))
        .unwrap_or(0);
    if n > 0 {
        return Ok(None);
    }
    let key = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let mut wrapped = 0usize;
    for r in roots {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM admins WHERE uid=?1",
                rusqlite::params![r.uid],
                |x| x.get(0),
            )
            .optional()?;
        let Some(id) = id else { continue };
        let w = crypto::encrypt_secret(&r.pwd, &key);
        conn.execute(
            "INSERT OR REPLACE INTO admin_master_key(admin_id, wrapped, created_at, updated_at) \
             VALUES (?1,?2,?3,?3)",
            rusqlite::params![id, w, now],
        )?;
        wrapped += 1;
    }
    if wrapped == 0 {
        log::err("主密钥已生成，但没有可封装副本的根管理员；主密钥将不可用");
    } else {
        println!(
            "[AlphaCoinServer] 主密钥已生成，并为 {wrapped} 个根管理员创建了口令封装副本（不落任何明文文件）"
        );
    }
    Ok(Some(key))
}
