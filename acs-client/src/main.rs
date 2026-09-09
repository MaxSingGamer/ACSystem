//! acs-client：A€（Alpha Coin）钱包客户端（Tauri 桌面应用）。
//!
//! - 默认启动 Tauri WebView 窗口（v3.0.0 弃用 axum 本地 Web 服务）
//! - 非交互子命令：`status` / `sync` / `new` / `open` / `send` / `submit` / `confirm`（便于脚本与测试）

// release 以 Windows GUI 子系统链接：双击启动时系统根本不会分配黑色控制台窗口。
// （CLI 子命令 / --debug 等带参数从终端启动时，见 attach_cli_console() 附加父控制台。）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

use acs_client::{client_api, sync, txn, wallet};
use acs_client::wallet::Wallet;

#[derive(Parser)]
#[command(name = "acs-client", version, about = "A€（Alpha Coin）钱包客户端 —— Tauri / CLI")]
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
    // release 为 GUI 子系统（默认无控制台）。凡带参数启动（子命令 / --debug / --help 等）
    // 视为从终端调用，先附加父进程控制台，保证 clap 帮助与 println 输出可见。
    #[cfg(all(target_os = "windows", not(debug_assertions)))]
    if std::env::args_os().count() > 1 {
        attach_cli_console();
    }

    // --debug 为“透传标记”，在交给 clap 前先过滤掉，避免被当作未知参数。
    #[cfg(target_os = "windows")]
    let debug = std::env::args().any(|a| a == "--debug");
    let args: Vec<String> = std::env::args().filter(|a| a != "--debug").collect();
    let cli = Cli::parse_from(args);
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
        None => {
            // 除非以 --debug 启动，否则隐藏黑色后端控制台窗口（仅写 .alphalog）。
            #[cfg(target_os = "windows")]
            if !debug {
                hide_console();
            }
            run_tauri()
        }
    }
}

/// 隐藏控制台窗口（Windows）。GUI 启动时调用，避免闪现黑色后端窗口。
/// （release 已以 GUI 子系统编译、系统不分配控制台，此函数主要兜底 debug 构建与从已有控制台启动的情形。）
#[cfg(target_os = "windows")]
fn hide_console() {
    use std::os::raw::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut c_void;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hWnd: *mut c_void, nCmdShow: i32) -> i32;
    }
    unsafe {
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, 0); // SW_HIDE
        }
    }
}

/// （release 专用）GUI 子系统进程默认无控制台。带参数从终端启动时，
/// 附加到父进程控制台并重定向标准句柄，使 CLI / --debug 的 println 输出可见。
#[cfg(all(target_os = "windows", not(debug_assertions)))]
fn attach_cli_console() {
    use std::os::windows::io::AsRawHandle;
    use std::os::raw::c_void;
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6; // -10
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // -11
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // -12
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(dwProcessId: u32) -> i32;
        fn GetConsoleWindow() -> *mut c_void;
        fn SetStdHandle(nStdHandle: u32, hHandle: *mut c_void) -> i32;
    }
    unsafe {
        if !GetConsoleWindow().is_null() {
            return; // 已有控制台，无需附加
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // 无父控制台（非终端启动），保持无窗口
        }
        // 打开并绑定 CONOUT$/CONIN$。句柄需长驻，故泄漏（进程生命周期内有效）。
        // 说明：Rust 标准输出在首次 println 时才取句柄，先 SetStdHandle 即可生效。
        let out = std::fs::OpenOptions::new().write(true).open("CONOUT$").ok();
        let err = std::fs::OpenOptions::new().write(true).open("CONOUT$").ok();
        let inn = std::fs::OpenOptions::new().read(true).open("CONIN$").ok();
        if let Some(f) = out {
            let h = f.as_raw_handle();
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            std::mem::forget(f);
        }
        if let Some(f) = err {
            let h = f.as_raw_handle();
            SetStdHandle(STD_ERROR_HANDLE, h);
            std::mem::forget(f);
        }
        if let Some(f) = inn {
            let h = f.as_raw_handle();
            SetStdHandle(STD_INPUT_HANDLE, h);
            std::mem::forget(f);
        }
    }
}

/// 启动 Tauri 桌面应用（v3.0.0）。
fn run_tauri() -> Result<()> {
    acs_client::app_main();
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
        .ok_or_else(|| anyhow!("无效账户类型：{typ}（Individual/Company/Country）"))?;

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
    // 写入本地账户清单（登录界面可见，支持多账户登录 / 跨设备登录取回）
    let _ = w.save_local_account(&uid, atype, &email, &gk.encrypted_seckey);
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
    let resp = txn::build_and_submit_transfer(&w, &r, rtype, amount, pass)?;
    let tid = resp.get("tx_id").and_then(|v| v.as_str()).unwrap_or("");
    let status = resp.get("status").and_then(|v| v.as_str()).unwrap_or("");
    println!("转账已提交（v3.0.0 直接发送，无二次确认）");
    println!("  tx_id : {tid}");
    println!("  状态  : {status}");
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
    // CLI 为本地运维工具：以操作员身份明示同意（GUI 端已强制勾选才可自助注册）
    let r = client_api::open_account(&w, &sek, "", true, true)?;
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
