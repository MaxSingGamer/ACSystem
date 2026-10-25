//! AlphaCoin 服务中心面板（v3.3.0 #13 / D38）。
//!
//! 独立于 `AlphaCoinServer` 的可执行文件：管理**多个**中心服务实例（启停 / 日志 / 新建 / 导入导出），
//! 自身是 Web 面板（不是 QT，D16）。面板重启不影响正在运行的服务进程（D38）。
//!
//! 数据目录：`~/.alphacoinserver/`
//! - `.env`        面板配置（域名仅记录 + 端口，默认 9600）
//! - `servers.json` 服务注册表
//! - `<域名>/`      各服务数据目录（由 AlphaCoinServer 使用）
//! - `logs/`       面板自身日志

mod api;
mod package;
mod registry;
mod service;
mod tarx;

use std::time::Duration;

use axum::http::header;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;

/// 面板前端（内嵌，单 exe 分发）。
const INDEX_HTML: &str = include_str!("../static/index.html");
const APP_JS: &str = include_str!("../static/app.js");
const APP_CSS: &str = include_str!("../static/app.css");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = registry::server_root();
    std::fs::create_dir_all(root.join("logs"))?;
    let _ = acs_core::log::init(&root.join("logs"));
    acs_core::log::info(format!(
        "AlphaCoinServerPanel 启动 v{}，根目录：{}",
        env!("CARGO_PKG_VERSION"),
        root.display()
    ));

    // 首次启动：写一份默认面板配置（端口 9600、域名空 → UI 会引导配置）
    if !registry::panel_env_path().exists() {
        registry::save_panel_config("", 9600)?;
        println!("[AlphaCoinServerPanel] 首次启动：已生成默认面板配置 {}（端口 9600）", registry::panel_env_path().display());
    }
    let cfg = registry::load_panel_config();
    let bind = registry::panel_bind();
    let port = cfg.port;

    let app = Router::new()
        .route("/", get(index))
        .route(
            "/static/app.js",
            get(|| async { ([("content-type", "application/javascript; charset=utf-8")], APP_JS) }),
        )
        .route(
            "/static/app.css",
            get(|| async { ([("content-type", "text/css; charset=utf-8")], APP_CSS) }),
        )
        .merge(api::routes())
        .with_state(api::state())
        .layer(RequestBodyLimitLayer::new(512 * 1024 * 1024)) // 服务包可能较大
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ));

    // 端口绑定期望重试：面板「改端口后自动重启」时，新进程要比旧进程退出早半分，
    // 否则会撞上 TIME_WAIT / 端口占用。这里最多重试 ~10 秒。
    let listener = bind_with_retry(&bind, port).await?;
    let shown_domain = if cfg.domain.is_empty() { "（未配置）".to_string() } else { cfg.domain.clone() };
    println!("[AlphaCoinServerPanel] 服务面板已就绪：http://{bind}:{port}/   （域名记录：{shown_domain}）");
    println!("[AlphaCoinServerPanel] 服务根目录：{}", root.display());
    if !cfg.configured {
        println!("[AlphaCoinServerPanel] 首次使用：请在浏览器打开上面的地址，按提示配置域名与端口");
        open_browser(&format!("http://127.0.0.1:{port}/"));
    }
    acs_core::log::info(format!("面板监听 http://{bind}:{port}/"));

    axum::serve(listener, app).await?;
    Ok(())
}

async fn bind_with_retry(bind: &str, port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    let mut last_err = None;
    for i in 0..40 {
        match tokio::net::TcpListener::bind((bind, port)).await {
            Ok(l) => return Ok(l),
            Err(e) => {
                last_err = Some(e);
                if i == 0 {
                    println!("[AlphaCoinServerPanel] 端口 {port} 暂不可用（可能是重启切换中），正在重试…");
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    Err(anyhow::anyhow!(
        "无法绑定 {bind}:{port}：{}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// 用系统默认浏览器打开面板（不经过 shell，避免注入）。
fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}
