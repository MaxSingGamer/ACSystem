//! acs-client：A€（Alpha Coin）钱包客户端。
//!
//! - 默认进入 TUI（参考 OpenCode：顶部状态栏 / 导航 / 底部命令栏 / 常驻帮助）
//! - 首次启动检测 → 引导（Onboarding）
//! - 非交互子命令：`status` / `sync` / `new`（便于脚本与测试）

mod app;
mod client_api;
mod sync;
mod txn;
mod ui;
mod wallet;

use std::io;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::App;
use crate::wallet::Wallet;

#[derive(Parser)]
#[command(name = "acs-client", version, about = "A€（Alpha Coin）钱包客户端 —— CLI / TUI")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 打印钱包状态
    Status,
    /// 从中心镜像拉取一次并打印结果
    Sync,
    /// 创建钱包（首次使用）。全部参数可选；缺省时交互输入。
    New {
        #[arg(long)]
        uid: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long, help = "钱包口令（≥8 位）")]
        pass: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        apikey: Option<String>,
        #[arg(long, default_value = "Individual")]
        typ: String,
    },
    /// 构建并本地签名一笔转账（写入 outbox 待提交）
    Send {
        /// 接收方 UID（可带 @类型，如 AlphaEU@System）
        receiver: String,
        /// 金额（A€）
        amount: i64,
        #[arg(long, help = "钱包口令")]
        pass: String,
    },
    /// 在中心开立本钱包账户（上传公钥）
    Open,
    /// 将 outbox 中待提交交易提交到中心（可指定 tx_id）
    Submit {
        #[arg(long)]
        tx_id: Option<String>,
    },
    /// 确认/拒绝中心的待确认交易（作为接收方，需钱包口令签名）
    Confirm {
        #[arg(long, help = "待确认交易 tx_id（缺省确认第一笔）")]
        tx_id: Option<String>,
        #[arg(long, help = "钱包口令")]
        pass: String,
        #[arg(long, help = "填写则拒绝（附理由）")]
        reject: Option<String>,
    },
    /// 运行期修改中心地址 / 镜像 apikey
    Config {
        #[arg(long, help = "中心地址，如 http://host:9600")]
        server: Option<String>,
        #[arg(long)]
        apikey: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Status) => cmd_status(),
        Some(Cmd::Sync) => cmd_sync(),
        Some(Cmd::New { uid, email, pass, server, apikey, typ }) => {
            cmd_new(uid, email, pass, server, apikey, typ)
        }
        Some(Cmd::Send { receiver, amount, pass }) => cmd_send(&receiver, amount, &pass),
        Some(Cmd::Open) => cmd_open(),
        Some(Cmd::Submit { tx_id }) => cmd_submit(tx_id.as_deref()),
        Some(Cmd::Confirm { tx_id, pass, reject }) => cmd_confirm(tx_id.as_deref(), &pass, reject.as_deref()),
        Some(Cmd::Config { server, apikey }) => cmd_config(server.as_deref(), apikey.as_deref()),
        None => run_tui(),
    }
}

/// TUI 主循环。
fn run_tui() -> Result<()> {
    let wallet = Wallet::open()?;
    let mut app = App::new(wallet);
    if app.mode == app::Mode::Main {
        app.refresh_view();
    }
    let mut terminal = ratatui::init();
    // 启用鼠标捕获（点击菜单/按钮）
    crossterm::execute!(std::io::stdout(), event::EnableMouseCapture)
        .map_err(|e| anyhow!("启用鼠标失败：{e}"))?;
    let res = event_loop(&mut terminal, &mut app);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    ratatui::restore();
    res.map_err(|e| anyhow!("终端错误：{e}"))
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while app.running {
        app.tick();
        let size = terminal.size()?;
        app.collect_hits(size);
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => app.handle_key(k),
                Event::Mouse(m) => {
                    if m.kind
                        == crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
                    {
                        app.handle_mouse(m.column, m.row);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// ---------------- 非交互子命令 ----------------

fn cmd_status() -> Result<()> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。运行 `acs-client` 进入引导，或 `acs-client new --uid <UID> --email <邮箱> --pass <口令>` 创建。");
        return Ok(());
    }
    println!("Alpha Wallet");
    println!("  UID       : {}", w.info.uid);
    println!("  类型      : {}", w.info.atype.as_str());
    println!("  邮箱      : {}", w.info.email);
    println!("  中心地址  : {}", if w.info.server_url.is_empty() { "(未配置)" } else { &w.info.server_url });
    println!("  镜像 apikey: {}", if w.info.mirror_apikey.is_empty() { "(未配置)" } else { "已配置" });
    println!("  创建时间  : {}", ts(w.info.created_at));
    println!("  上次同步  : {}", if w.info.synced_at > 0 { ts(w.info.synced_at) } else { "从未".into() });
    println!("  余额(镜像): {} A€", w.mirror_balance());
    Ok(())
}

fn cmd_sync() -> Result<()> {
    let mut w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。先创建钱包后再同步。");
        return Ok(());
    }
    let r = sync::pull(&w)?;
    w.mark_synced(r.server_time, None)?;
    println!("同步完成：新增交易 {} · 账户快照 {} · 快照哈希 {}",
        r.txs, r.accounts, &r.hash[..r.hash.len().min(16)]);
    if let Some(s) = &r.central_sig {
        println!("中心签名：已附加（{} 字节）", s.len());
    }
    println!("本账户余额（镜像口径）：{} A€", w.mirror_balance());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_new(
    uid: Option<String>,
    email: Option<String>,
    pass: Option<String>,
    server: Option<String>,
    apikey: Option<String>,
    typ: String,
) -> Result<()> {
    let mut w = Wallet::open()?;
    if w.info.initialized() {
        println!("已存在钱包（UID={}）。如需重建请删除 ~/.alpha_dir 后重试。", w.info.uid);
        return Ok(());
    }
    let uid = match uid {
        Some(u) => u,
        None => readline("UID（游戏 ID / 用户名）: ")?,
    };
    let email = match email {
        Some(e) => e,
        None => readline("邮箱（如 user@aeu.org）: ")?,
    };
    let pass = match pass {
        Some(p) if p.len() >= 8 => p,
        Some(_) => return Err(anyhow!("口令至少 8 位")),
        None => {
            let p = readline("钱包口令（≥8 位，用于本地签名）: ")?;
            if p.len() < 8 {
                return Err(anyhow!("口令至少 8 位"));
            }
            p
        }
    };
    let atype = acs_core::models::AccountType::from_str(&typ)
        .ok_or_else(|| anyhow!("无效账户类型：{typ}（Individual/Bank/Country）"))?;

    // 中心服务器地址：必填（缺省时交互输入），自动补全协议前缀
    let server = match server {
        Some(s) => s,
        None => {
            let s = readline("中心服务器地址（必填，如 http://localhost:8080）: ")?;
            if s.trim().is_empty() {
                return Err(anyhow!("中心服务器地址为必填项"));
            }
            s
        }
    };
    let server = if server.starts_with("http://") || server.starts_with("https://") {
        server
    } else {
        format!("http://{server}")
    };

    println!("正在生成 ed25519 密钥（gpg）…");
    let gk = w.create_key(&uid, &email, &pass)?;
    println!("  指纹：{}", gk.fingerprint);
    w.set_server_url(&server)?;
    if let Some(k) = apikey {
        w.set_mirror_apikey(&k)?;
    }
    w.init_wallet(&uid, atype, &email)?;
    println!("钱包创建完成：{uid} · {}", atype.as_str());
    println!("数据目录：{}", wallet::data_dir_str().display());
    println!("运行 `acs-client` 进入界面，或 `acs-client sync` 同步账本。");
    Ok(())
}

fn cmd_send(receiver: &str, amount: i64, pass: &str) -> Result<()> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。先创建钱包。");
        return Ok(());
    }
    let mut r = receiver.to_string();
    let mut rtype = acs_core::models::AccountType::Individual;
    if let Some(idx) = r.find('@') {
        let ty = r[idx + 1..].to_string();
        r = r[..idx].to_string();
        rtype = acs_core::models::AccountType::from_str(&ty).unwrap_or(acs_core::models::AccountType::Individual);
    }
    let (tx_id, tx_hash) = txn::build_and_sign_transfer(&w, &r, rtype, amount, pass)?;
    println!("已签名转账 → outbox");
    println!("  tx_id : {tx_id}");
    println!("  tx_hash: {tx_hash}");
    println!("  接收方: {} · {}", r, rtype.as_str());
    println!("  金额  : {amount} A€");
    Ok(())
}

fn cmd_config(server: Option<&str>, apikey: Option<&str>) -> Result<()> {
    let mut w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。");
        return Ok(());
    }
    if let Some(s) = server {
        let mut u = s.trim().trim_end_matches('/').to_string();
        if !u.starts_with("http://") && !u.starts_with("https://") {
            u = format!("http://{u}");
        }
        w.set_server_url(&u)?;
        println!("中心地址已更新：{u}");
    }
    if let Some(k) = apikey {
        w.set_mirror_apikey(k.trim())?;
        println!("镜像 apikey 已更新");
    }
    if server.is_none() && apikey.is_none() {
        println!("用法：acs-client config --server <地址> [--apikey <key>]");
    }
    Ok(())
}

fn cmd_open() -> Result<()> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。先创建钱包。");
        return Ok(());
    }
    // 加密私钥取本地缓存；CLI 模式无密码，不启用登录取回（password_hash 留空）
    let sek = w.encrypted_seckey().unwrap_or_default();
    let r = client_api::open_account(&w, &sek, "")?;
    println!("账户开立完成：{uid} · {ty}（余额 {bal} A€）",
        uid = r.get("uid").and_then(|v| v.as_str()).unwrap_or(""),
        ty = r.get("type").and_then(|v| v.as_str()).unwrap_or(""),
        bal = r.get("balance").and_then(|v| v.as_i64()).unwrap_or(0));
    if let Some(fp) = r.get("fingerprint").and_then(|v| v.as_str()) {
        println!("中心记录指纹：{fp}");
    }
    Ok(())
}

fn cmd_submit(tx_id: Option<&str>) -> Result<()> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。");
        return Ok(());
    }
    let results = client_api::submit_outbox(&w, tx_id)?;
    if results.is_empty() {
        println!("outbox 中没有待提交交易。");
        return Ok(());
    }
    for (id, res) in &results {
        println!("  {id}  →  {res}");
    }
    Ok(())
}

fn cmd_confirm(tx_id: Option<&str>, pass: &str, reject: Option<&str>) -> Result<()> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。");
        return Ok(());
    }
    let tid = match tx_id {
        Some(t) => t.to_string(),
        None => {
            let pending = client_api::list_pending(&w)?;
            match pending.first() {
                Some(p) => {
                    println!("待确认交易 {n} 笔，取第一笔 {id}（{sender} → 我，{amt} A€）",
                        n = pending.len(), id = p.tx_id, sender = p.sender, amt = p.amount);
                    p.tx_id.clone()
                }
                None => {
                    println!("没有待确认交易。");
                    return Ok(());
                }
            }
        }
    };
    let r = client_api::confirm_tx(&w, &tid, pass, reject)?;
    println!(
        "交易 {id} 状态 → {status}",
        id = r.get("tx_id").and_then(|v| v.as_str()).unwrap_or(""),
        status = r.get("status").and_then(|v| v.as_str()).unwrap_or("")
    );
    Ok(())
}

fn readline(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{prompt}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn ts(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}
