//! 网页服务：从磁盘 static/ 提供多文件（HTML/CSS/JS）。
//! 登录后按角色跳转独立页面（/root、/finance），页面访问由服务端拦截。

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
        .route("/finance", get(finance_page))
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

/// 金融部页面壳。
async fn finance_page() -> Html<String> {
    Html(read_html("finance.html"))
}

fn read_html(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{}", static_dir(), name))
        .unwrap_or_else(|_| format!("<h1>缺失页面: {name}</h1>"))
}
