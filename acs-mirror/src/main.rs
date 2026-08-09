//! acs-mirror：A€ 只读镜像。
//!
//! - `config --server <url> --apikey <key>`：配置中心与镜像凭证（可选 --central-pubkey 开启签名校验）
//! - `sync`：拉取中心增量账本与账户快照
//! - `status`：显示同步状态
//! - `serve --port 9090`：启动只读 HTTP 查询服务

mod http;
mod pull;
mod store;

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

use store::Store;

#[derive(Parser)]
#[command(name = "acs-mirror", version, about = "A€（Alpha Coin）只读镜像")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 配置中心地址与镜像 apikey
    Config {
        #[arg(long)]
        server: String,
        #[arg(long)]
        apikey: String,
        #[arg(long, help = "中心公钥（armored），配置后 sync 将校验快照签名")]
        central_pubkey: Option<String>,
    },
    /// 拉取中心增量并写入本地镜像库
    Sync,
    /// 显示同步状态
    Status,
    /// 启动只读 HTTP 服务
    Serve {
        #[arg(long, default_value = "9090")]
        port: u16,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Config { server, apikey, central_pubkey } => cmd_config(&server, &apikey, central_pubkey.as_deref()),
        Cmd::Sync => cmd_sync(),
        Cmd::Status => cmd_status(),
        Cmd::Serve { port } => cmd_serve(port),
    }
}

fn cmd_config(server: &str, apikey: &str, central_pubkey: Option<&str>) -> Result<()> {
    let mut s = Store::open()?;
    let server = server.trim().trim_end_matches('/');
    if !server.starts_with("http://") && !server.starts_with("https://") {
        return Err(anyhow!("server 需以 http:// 或 https:// 开头"));
    }
    s.set_config(server, apikey)?;
    if let Some(pk) = central_pubkey {
        store::meta_set(&s.conn, "central_pubkey", pk.trim())?;
        s.info.central_pubkey = pk.trim().to_string();
        println!("已配置中心公钥（将校验快照签名）");
    }
    println!("镜像配置完成：server={server}");
    println!("数据目录：{}", store::data_dir().display());
    Ok(())
}

fn cmd_sync() -> Result<()> {
    let mut s = Store::open()?;
    let r = pull::pull(&mut s)?;
    println!("同步完成：新增交易 {} · 账户快照 {}", r.txs, r.accounts);
    println!("  快照哈希：{}", &r.hash[..r.hash.len().min(24)]);
    println!(
        "  中心签名：{}",
        if r.central_sig.is_some() { "已附带" } else { "无" }
    );
    if !s.info.central_pubkey.is_empty() {
        println!("  签名校验：已启用并通过");
    }
    println!("  当前账户 {} · 交易 {}", s.account_count(), s.tx_count());
    Ok(())
}

fn cmd_status() -> Result<()> {
    let s = Store::open()?;
    println!("Alpha Coin · 只读镜像");
    println!("  中心地址  : {}", if s.info.server_url.is_empty() { "(未配置)" } else { &s.info.server_url });
    println!("  镜像 apikey: {}", if s.info.apikey.is_empty() { "(未配置)" } else { "已配置" });
    println!("  上次同步  : {}", if s.info.last_sync > 0 { ts(s.info.last_sync) } else { "从未".into() });
    println!("  快照哈希  : {}", if s.info.last_hash.is_empty() { "-".into() } else { s.info.last_hash[..s.info.last_hash.len().min(24)].to_string() });
    println!("  账户快照  : {}", s.account_count());
    println!("  交易记录  : {}", s.tx_count());
    println!("  数据目录  : {}", store::data_dir().display());
    Ok(())
}

#[tokio::main]
async fn cmd_serve(port: u16) -> Result<()> {
    let s = Store::open()?;
    let state = http::HttpState {
        store: Arc::new(Mutex::new(s)),
    };
    let app = http::router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("acs-mirror 只读服务已启动: http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn ts(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}
