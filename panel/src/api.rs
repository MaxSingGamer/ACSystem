//! 面板 REST API（v4.0.0）。
//!
//! 安全说明（D40）：面板默认只监听 `127.0.0.1`（见 `registry::panel_bind`），
//! 且 v4.0.0 起**必须登录**才能做任何操作（Panel #1）。
//! 它能启停/删除服务，因此**不应**直接暴露公网；如需远程访问请用反向代理。

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use acs_core::config;

use crate::auth;
use crate::package;
use crate::passkey;
use crate::registry;
use crate::service::{self, NewServiceReq, UpdateServiceReq};

/// 面板共享状态（会话是内存态，面板重启即失效）。
#[derive(Clone)]
pub struct PanelState {
    pub version: String,
    pub auth: auth::AuthState,
}

pub fn routes() -> Router<PanelState> {
    // 登录/账户安全路由：请求体上限收紧到 64KB（防超大 body 的内存/解析放大）
    let auth_routes: Router<PanelState> = Router::new()
        // ---------- 登录 / 账户安全（Panel #1） ----------
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/account", get(auth::account_info))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/verify", post(auth::verify))
        .route("/api/auth/change-credentials", post(auth::change_credentials))
        .route("/api/auth/totp/begin", post(auth::totp_begin))
        .route("/api/auth/totp/confirm", post(auth::totp_confirm))
        .route("/api/auth/totp/remove", post(auth::totp_remove))
        .route("/api/auth/passkey/login/begin", post(passkey::login_begin))
        .route("/api/auth/passkey/login/finish", post(passkey::login_finish))
        .route("/api/auth/passkey/register/begin", post(passkey::register_begin))
        .route("/api/auth/passkey/register/finish", post(passkey::register_finish))
        .route("/api/auth/passkey/remove", post(passkey::remove))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024));
    Router::new()
        .merge(auth_routes)
        // ---------- 面板 / 服务（需登录且已完成强制改密） ----------
        .route("/api/panel", get(get_panel).post(set_panel))
        .route("/api/services", get(list_services).post(create_service))
        .route(
            "/api/services/{name}",
            put(update_service).delete(delete_service),
        )
        .route("/api/services/{name}/start", post(start_service))
        .route("/api/services/{name}/stop", post(stop_service))
        .route("/api/services/{name}/resolve-dir", post(resolve_dir))
        .route("/api/services/{name}/logs", get(list_logs))
        .route("/api/services/{name}/logs/{file}", get(tail_log))
        .route("/api/services/{name}/export", get(export_package))
        .route("/api/services/{name}/package-info", get(package_info))
        .route("/api/import", post(import_package))
}

// ------------------------------- 面板配置 -------------------------------

async fn get_panel(_user: auth::PanelUser, State(st): State<PanelState>) -> Json<serde_json::Value> {
    let cfg = registry::load_panel_config();
    Json(json!({
        "domain": cfg.domain,
        "port": cfg.port,
        "bind": registry::panel_bind(),
        "url": registry::panel_url(&cfg),
        "configured": cfg.configured,
        "root": registry::server_root().display().to_string(),
        "version": st.version,
    }))
}

#[derive(Deserialize)]
struct PanelCfgReq {
    domain: String,
    port: u16,
}

/// 保存面板域名/端口 → 自动重启面板（**不重启任何服务**，D38/第 9 条）。
async fn set_panel(
    _user: auth::PanelUser,
    Json(req): Json<PanelCfgReq>,
) -> Response {
    if let Err(e) = registry::save_panel_config(&req.domain, req.port) {
        return err(StatusCode::BAD_REQUEST, e.to_string());
    }
    schedule_restart();
    Json(json!({ "ok": true, "restarting": true, "message": "面板配置已保存，正在自动重启面板（各服务不受影响；重启后需重新登录）" }))
        .into_response()
}

/// 延迟重启自身：先让当前响应写回浏览器，再拉起新进程并退出。
///
/// D38：面板与服务是独立进程，重启面板**不会**影响正在运行的服务
/// （服务进程与其内存中的主密钥解锁态都不受影响）。
fn schedule_restart() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(std::env::args().skip(1))
                .spawn();
        }
        std::process::exit(0);
    });
}

// ------------------------------- 服务列表 -------------------------------

async fn list_services(_user: auth::PanelUser) -> Response {
    let reg = match registry::load_registry() {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let panel = registry::load_panel_config();
    let items: Vec<serde_json::Value> = reg
        .servers
        .iter()
        .map(|s| {
            let (state, pid) = service::service_state(s);
            json!({
                "name": s.name,
                "api_domain": s.api_domain,
                "admin_domain": s.admin_domain,
                "api_port": s.api_port,
                "admin_port": s.admin_port,
                "dir": s.dir,
                "dir_mismatch": s.dir_mismatch(),
                "state": state,
                "pid": pid,
                "url": format!("https://{}", s.api_domain),
                // 后台入口用「后台管理域名」（Panel #3/#6）
                "admin_url": format!("https://{}/login", s.admin_domain),
                "created_at": s.created_at,
            })
        })
        .collect();
    Json(json!({
        "panel": {
            "domain": panel.domain,
            "port": panel.port,
            "bind": registry::panel_bind(),
            "url": registry::panel_url(&panel),
            "root": registry::server_root().display().to_string(),
        },
        "services": items,
    }))
    .into_response()
}

async fn create_service(_user: auth::PanelUser, Json(req): Json<NewServiceReq>) -> Response {
    match service::create_service(req) {
        Ok(rec) => Json(json!({ "ok": true, "service": rec })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn update_service(
    _user: auth::PanelUser,
    Path(name): Path<String>,
    Json(req): Json<UpdateServiceReq>,
) -> Response {
    match service::update_service(&name, req) {
        Ok((rec, restarted)) => Json(json!({
            "ok": true,
            "service": rec,
            "restarted": restarted,
            "message": if restarted {
                "已更新；服务原先在运行，已自动停止并重启以释放/重建文件夹"
            } else {
                "已更新"
            }
        }))
        .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn delete_service(_user: auth::PanelUser, Path(name): Path<String>) -> Response {
    match service::delete_service(&name) {
        Ok(()) => Json(json!({ "ok": true, "message": format!("服务已删除：{name}") })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn start_service(_user: auth::PanelUser, Path(name): Path<String>) -> Response {
    match service::start_service(&name) {
        Ok(pid) => Json(json!({ "ok": true, "pid": pid, "message": "服务已启动（首次启动会创建账户种子，可能稍慢）" }))
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn stop_service(_user: auth::PanelUser, Path(name): Path<String>) -> Response {
    match service::stop_service(&name) {
        Ok(()) => Json(json!({ "ok": true, "message": "服务已停止" })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct ResolveDirReq {
    /// "domain" = 用服务接口域名（重命名文件夹）；"dir" = 用文件夹名（改接口域名）
    choice: String,
}

/// Panel #4：文件夹名与服务接口域名不一致时，由用户选择以哪一个为准。
async fn resolve_dir(
    _user: auth::PanelUser,
    Path(name): Path<String>,
    Json(req): Json<ResolveDirReq>,
) -> Response {
    match service::resolve_dir_mismatch(&name, req.choice.trim()) {
        Ok(rec) => Json(json!({ "ok": true, "service": rec, "message": "已按所选命名统一" })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

// ------------------------------- 日志 -------------------------------

async fn list_logs(_user: auth::PanelUser, Path(name): Path<String>) -> Response {
    let Ok(reg) = registry::load_registry() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "读取注册表失败");
    };
    let Some(rec) = registry::find_service(&reg, &name) else {
        return err(StatusCode::NOT_FOUND, "服务不存在");
    };
    match service::list_logs(rec) {
        Ok(files) => Json(json!({ "ok": true, "files": files })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct TailQuery {
    #[serde(default = "default_lines")]
    lines: usize,
}
fn default_lines() -> usize {
    300
}

async fn tail_log(
    _user: auth::PanelUser,
    Path((name, file)): Path<(String, String)>,
    Query(q): Query<TailQuery>,
) -> Response {
    let Ok(reg) = registry::load_registry() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "读取注册表失败");
    };
    let Some(rec) = registry::find_service(&reg, &name) else {
        return err(StatusCode::NOT_FOUND, "服务不存在");
    };
    match service::tail_log(rec, &file, q.lines) {
        Ok(text) => Json(json!({ "ok": true, "file": file, "text": text })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e.to_string()),
    }
}

// --------------------------- AlphaTar 服务包 ---------------------------

#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default)]
    password: String,
}

/// 导出 AlphaTar 服务包（D36）：`<服务接口域名>.gz.alphatar`，可选密码加密。
async fn export_package(
    _user: auth::PanelUser,
    Path(name): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Response {
    let Ok(reg) = registry::load_registry() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "读取注册表失败");
    };
    let Some(rec) = registry::find_service(&reg, &name) else {
        return err(StatusCode::NOT_FOUND, "服务不存在");
    };
    if service::port_open(rec.api_port) || service::port_open(rec.admin_port) {
        return err(
            StatusCode::CONFLICT,
            "服务正在运行：请先停止服务再导出（保证数据库快照一致）",
        );
    }
    let dir = registry::service_dir(rec);
    let pwd = q.password.trim();
    let data = match package::export_package(&dir, &rec.dir, if pwd.is_empty() { None } else { Some(pwd) }) {
        Ok(d) => d,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let filename = format!("{}.gz.alphatar", rec.api_domain);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        data,
    )
        .into_response()
}

async fn package_info(_user: auth::PanelUser, Path(name): Path<String>) -> Response {
    let Ok(reg) = registry::load_registry() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "读取注册表失败");
    };
    let Some(rec) = registry::find_service(&reg, &name) else {
        return err(StatusCode::NOT_FOUND, "服务不存在");
    };
    let dir = registry::service_dir(rec);
    let size = dir_size(&dir);
    Json(json!({
        "ok": true,
        "dir": dir.display().to_string(),
        "size_bytes": size,
        "suggested_name": format!("{}.gz.alphatar", rec.api_domain),
        "hint": registry::NO_EDIT_HINT,
    }))
    .into_response()
}

fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for e in rd.flatten() {
        if let Ok(md) = e.metadata() {
            if md.is_file() {
                total += md.len();
            } else if md.is_dir() {
                total += dir_size(&e.path());
            }
        }
    }
    total
}

#[derive(Deserialize)]
struct ImportQuery {
    /// 导入方设置的**服务名称**（保证唯一；包内不含名称）
    name: String,
    #[serde(default)]
    password: String,
    /// Panel #4：文件夹名与包内服务接口域名不一致时，用户的选择（domain / dir）
    #[serde(default)]
    dir_choice: String,
}

/// 导入 AlphaTar 服务包：
/// 1) 解析包内容 → 2) 从包内 `.env` 读「服务接口域名 / 后台管理域名 / 端口」 →
/// 3) 若包内顶层文件夹名与接口域名不一致，返回 409 让用户选择以哪一个为准（Panel #4）→
/// 4) 校验名称/域名/端口冲突 → 5) 解包并统一到所选名字 → 6) 写入 `servers.json`。
///
/// 名称在包内不存在（第 5 条：打包不打包服务名称），由导入方提供。
async fn import_package(
    _user: auth::PanelUser,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Response {
    let name = q.name.trim().to_string();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "请填写服务名称");
    }
    if body.is_empty() {
        return err(StatusCode::BAD_REQUEST, "请上传 .gz.alphatar 服务包");
    }
    let entries = match package::import_package(&body, if q.password.trim().is_empty() {
        None
    } else {
        Some(q.password.trim())
    }) {
        Ok(e) => e,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let tops = crate::tarx::top_level(&entries);
    if tops.len() != 1 {
        return err(StatusCode::BAD_REQUEST, "服务包结构异常：顶层应只有一个目录");
    }
    let top = tops[0].clone();
    let env = read_env_from_entries(&entries, &top);
    let env_domain = env
        .as_ref()
        .map(|e| domain_of(package::env_get(e, "ACS_PUBLIC_URL").unwrap_or(&top)))
        .unwrap_or_else(|| top.clone());
    let api_domain = if env_domain.trim().is_empty() { top.clone() } else { env_domain.clone() };
    let admin_domain = env
        .as_ref()
        .and_then(|e| package::env_get(e, "ACS_ADMIN_URL"))
        .map(domain_of)
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| registry::default_admin_domain(&api_domain));
    let api_port: u16 = env
        .as_ref()
        .and_then(|e| package::env_get(e, "ACS_PUBLIC_PORT"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let admin_port: u16 = env
        .as_ref()
        .and_then(|e| package::env_get(e, "ACS_ADMIN_PORT"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if api_port == 0 || admin_port == 0 {
        return err(StatusCode::BAD_REQUEST, "包内 .env 缺少有效的 ACS_PUBLIC_PORT / ACS_ADMIN_PORT");
    }
    // Panel #4：文件夹名 ≠ 服务接口域名 → 要求选择
    let mismatch = top != api_domain;
    let choice = q.dir_choice.trim();
    if mismatch && choice != "domain" && choice != "dir" {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!(
                    "文件夹名与包内服务接口域名不一致：文件夹为 `{top}`，接口域名为 `{api_domain}`。请选择以哪一个为准。"
                ),
                "needs_dir_choice": true,
                "folder": top,
                "api_domain": api_domain,
                "options": [
                    { "value": "domain", "label": format!("使用服务接口域名 {api_domain}（重命名文件夹）") },
                    { "value": "dir", "label": format!("使用文件夹名 {top}（把服务接口域名改为 {top}）") }
                ]
            })),
        )
            .into_response();
    }
    let api_domain = if mismatch && choice == "dir" { top.clone() } else { api_domain };
    let admin_domain = if mismatch && choice == "dir" && admin_domain == registry::default_admin_domain(&env_domain) {
        registry::default_admin_domain(&api_domain)
    } else {
        admin_domain
    };
    let rec = registry::ServiceRec {
        name: name.clone(),
        api_domain: api_domain.clone(),
        admin_domain,
        api_port,
        admin_port,
        dir: api_domain.clone(),
        created_at: chrono::Utc::now().timestamp(),
        pid: None,
    };
    let reg = match registry::load_registry() {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = registry::validate_no_conflict(&reg, &rec, None) {
        return err(StatusCode::CONFLICT, format!("{e}（请修改服务名称/域名后再导入）"));
    }
    let root = registry::server_root();
    if root.join(&rec.dir).exists() {
        return err(StatusCode::CONFLICT, format!("目录已存在：{}", rec.dir));
    }
    if let Err(e) = crate::package::extract_package(&entries, &root) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    // 解包后目录名统一为所选名字
    let extracted = root.join(&top);
    let target = root.join(&rec.dir);
    if extracted != target && extracted.exists() {
        if target.exists() {
            let _ = std::fs::remove_dir_all(&target);
        }
        if let Err(e) = std::fs::rename(&extracted, &target) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("重命名解包目录失败：{e}"));
        }
    }
    // 同步 .env：域名/端口以注册表为准（选择「用文件夹名」时接口域名已改变）
    let env_path = target.join(".env");
    if env_path.exists() {
        if let Err(e) = service::set_env_keys(
            &env_path,
            &[
                ("ACS_PUBLIC_URL", format!("https://{}", rec.api_domain)),
                ("ACS_PUBLIC_PORT", rec.api_port.to_string()),
                ("ACS_ADMIN_URL", format!("https://{}", rec.admin_domain)),
                ("ACS_ADMIN_PORT", rec.admin_port.to_string()),
            ],
        ) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    let mut reg = reg;
    reg.servers.push(rec.clone());
    if let Err(e) = registry::save_registry(&reg) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    Json(json!({
        "ok": true,
        "service": rec,
        "message": "导入完成；如需换端口/域名，可在面板内修改（仅域名与端口可改）"
    }))
    .into_response()
}

fn read_env_from_entries(
    entries: &[crate::tarx::Entry],
    top: &str,
) -> Option<Vec<(String, String)>> {
    let target = format!("{top}/.env");
    let e = entries.iter().find(|e| e.path == target)?;
    let text = String::from_utf8_lossy(&e.data);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Some(out)
}

/// 从 URL 提取域名；非法时返回空串。
fn domain_of(url: &str) -> String {
    if url.trim().is_empty() {
        return String::new();
    }
    let key = config::domain_key(url);
    if key == "default" {
        String::new()
    } else {
        key
    }
}

// ------------------------------- 工具 -------------------------------

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

/// 面板版本号（用于 UI 页脚）。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 供 main 复用的共享状态。
pub fn state() -> PanelState {
    PanelState { version: version().to_string(), auth: auth::AuthState::default() }
}
