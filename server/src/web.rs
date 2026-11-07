//! 网页服务：从磁盘 static/ 提供多文件（HTML/CSS/JS）。
//! 登录后按角色跳转独立页面（Root → /root，Admin → /admin），页面访问由服务端拦截。

use axum::response::{Html, Redirect};
use axum::routing::get;
use axum::Router;
use tower_http::services::ServeDir;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let dir = static_dir();
    Router::new()
        .route("/", get(index))
        .route("/login", get(login_page))
        .route("/root", get(root_page))
        .route("/admin", get(admin_page))
        .route("/sys", get(sys_page))
        .nest_service("/static", ServeDir::new(dir))
}

/// 静态目录：优先 ACS_STATIC_DIR，其次可执行文件同级 static/，兜底源码目录 static/。
fn static_dir() -> String {
    if let Ok(d) = std::env::var("ACS_STATIC_DIR") {
        return d;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            let cand = p.join("static");
            if cand.exists() {
                return cand.to_string_lossy().into_owned();
            }
        }
    }
    concat!(env!("CARGO_MANIFEST_DIR"), "/static").to_string()
}

/// 服务接口域名（公开端口）根路径：重定向到「后台管理域名」的登录页。
///
/// v4.0.0（Panel #3）：一个服务有两个域名——
/// ① 服务接口域名（API，唯一识别符，同时决定服务文件夹名）；
/// ② 后台管理域名（网页，仅用于把人带到后台登录页）。
/// 因此访问 API 域名根路径时不再返回 404，而是跳到后台登录入口。
pub async fn redirect_to_admin() -> axum::response::Response {
    use axum::response::IntoResponse;
    let target = std::env::var("ACS_ADMIN_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    match target {
        Some(base) => Redirect::to(&format!("{base}/login")).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            "服务接口域名不提供网页入口。请通过后台管理域名（ACS_ADMIN_URL）访问管理后台。",
        )
            .into_response(),
    }
}

async fn index() -> Redirect {
    Redirect::to("/login")
}

async fn login_page() -> Html<String> {
    Html(read_html("login.html"))
}

/// 根管理员页面壳（鉴权与角色跳转由前端 JS 经 /api/admin/me 完成；
/// 页面壳不含敏感数据，数据层 API 均有令牌校验）。
async fn root_page() -> Html<String> {
    Html(read_html("root.html"))
}

/// 管理员（Admin，原金融部）页面壳。
async fn admin_page() -> Html<String> {
    Html(read_html("admin.html"))
}

/// 系统账本账户登录 / 操作页（替代客户端登录 System 账户；功能与客户端钱包一致）。
async fn sys_page() -> Html<String> {
    Html(read_html("sys.html"))
}

fn read_html(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{}", static_dir(), name))
        .unwrap_or_else(|_| format!("<h1>缺失页面: {name}</h1>"))
}
