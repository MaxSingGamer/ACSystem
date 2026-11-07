//! AlphaCoinClientCli：A€（Alpha Coin）命令行客户端（运维 / 自动化）。
//!
//! 图形界面客户端为 `client/qt` 的 AlphaWallet / AlphaCompanyWallet（QT 6），
//! 本程序只提供非交互子命令：`status` / `sync` / `new` / `open` / `send` / `submit` / `confirm` / `config` / `repair`。
//! **不随 Release 发布**，仅本地构建用于运维与端到端测试。
//!
//! 注：v3.3.0 起已删除 Tauri 薄壳，CLI 为普通控制台程序。

use std::io;

use anyhow::{anyhow, Result};
use clap::{CommandFactory, Parser, Subcommand};

use acs_client_core::{client_api, sync, txn, wallet};
use acs_client_core::wallet::Wallet;

#[derive(Parser)]
#[command(name = "AlphaCoinClientCli", version, about = "A€（Alpha Coin）命令行客户端（GUI 见 AlphaWallet / AlphaCompanyWallet）")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 打印钱包状态
    Status,
    /// 从中心拉取一次并打印结果（v4.0.0：同步需账户签名，必须提供钱包口令）
    Sync {
        #[arg(long, help = "钱包口令（用于对同步请求签名）")]
        pass: String,
    },
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
    /// 国家账户简称 2 位大写字母、企业账户简称 4 位大写字母；两类均需 --fullname（全称）
    Open {
        #[arg(long, default_value = "", help = "全称（国家 / 企业必填，同类型内唯一；个人账户留空）")]
        fullname: String,
    },
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
    /// 修复本地 gpg 环境（信任库损坏 / 陈旧锁 / 注册失败残留的重复密钥）
    Repair {
        #[arg(long, help = "同时清理同一 UID 的重复密钥，只保留最新一把")]
        dedup_keys: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Status) => cmd_status(),
        Some(Cmd::Sync { pass }) => cmd_sync(&pass),
        Some(Cmd::New { uid, email, pass, server, apikey, typ }) => {
            cmd_new(uid, email, pass, server, apikey, typ)
        }
        Some(Cmd::Send { receiver, amount, pass }) => cmd_send(&receiver, amount, &pass),
        Some(Cmd::Open { fullname }) => cmd_open(&fullname),
        Some(Cmd::Submit { tx_id }) => cmd_submit(tx_id.as_deref()),
        Some(Cmd::Confirm { tx_id, pass, reject }) => cmd_confirm(tx_id.as_deref(), &pass, reject.as_deref()),
        Some(Cmd::Config { server, apikey }) => cmd_config(server.as_deref(), apikey.as_deref()),
        Some(Cmd::Repair { dedup_keys }) => cmd_repair(dedup_keys),
        None => {
            // 无子命令：打印帮助（图形界面请用 AlphaWallet / AlphaCompanyWallet）
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

// ---------------- 非交互子命令 ----------------

/// 改变状态的子命令统一前置：钱包必须已初始化，否则按**失败**返回。
///
/// 历史上这些分支是 `println!("钱包尚未初始化。"); return Ok(())`，退出码为 0，
/// 导致脚本/自动化把“什么都没做”当成成功（例如钱包损坏时 `open` 看起来注册成功）。
fn wallet_ready() -> Result<Wallet> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        return Err(anyhow!(
            "钱包尚未初始化：先运行 `AlphaCoinClientCli new --uid <UID> --email <邮箱> --pass <口令>` 创建钱包"
        ));
    }
    Ok(w)
}

fn cmd_status() -> Result<()> {
    let w = Wallet::open()?;
    if !w.info.initialized() {
        println!("钱包尚未初始化。运行 `AlphaCoinClientCli` 进入引导，或 `AlphaCoinClientCli new --uid <UID> --email <邮箱> --pass <口令>` 创建。");
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

fn cmd_sync(pass: &str) -> Result<()> {
    // 未初始化时按失败返回：否则“什么都没同步”会被自动化当成成功。
    // （`status` 是纯查询，仍允许在未初始化状态下打印状态。）
    let mut w = wallet_ready()?;
    // v4.0.0（F4）：/api/sync 需要账户签名；本进程内缓存口令用于签名（仅内存）。
    w.set_session_passphrase(pass);
    let r = sync::pull(&w)?;
    if r.accounts == 0 {
        return Err(anyhow!("中心未返回账户快照，本次同步未生效（同步水位未推进）"));
    }
    // 水位取中心时间（服务端按 ts 过滤增量）；用本机时钟会漏掉已确认交易
    w.mark_synced(r.server_time, None)?;
    w.reload_info();
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
        // 不返回成功：“什么都没做”对脚本/自动化是误导
        return Err(anyhow!(
            "已存在钱包（UID={}）：如需重建请删除 `~/.alphawallet/<中心域名>/` 后重试",
            w.info.uid
        ));
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

    // v3.3.0（#9）：**先切到该中心的数据目录，再生成密钥**（否则密钥会落在旧目录里）
    println!("正在准备中心数据目录…");
    w.set_server_url(&server)?;
    if let Some(k) = apikey {
        w.set_mirror_apikey(&k)?;
    }

    println!("正在生成 ed25519 密钥（gpg）…");
    let gk = w.create_key(&uid, &email, &pass)?;
    println!("  指纹：{}", gk.fingerprint);
    w.init_wallet(&uid, atype, &email)?;
    // 写入本地账户清单（登录界面可见，支持多账户登录 / 跨设备登录取回）
    let _ = w.save_local_account(&uid, atype, &email, &gk.pubkey, &gk.encrypted_seckey);
    println!("钱包创建完成：{uid} · {}", atype.as_str());
    println!("数据目录：{}", wallet::data_dir_str().display());
    println!("运行 `AlphaCoinClientCli` 进入界面，或 `AlphaCoinClientCli sync` 同步账本。");
    Ok(())
}

fn cmd_send(receiver: &str, amount: i64, pass: &str) -> Result<()> {
    let w = wallet_ready()?;
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
        // v3.3.0 #7：设置中心后拉取该中心下发的配置并缓存
        acs_client_core::web_helpers::refresh_client_config(&mut w);
        println!("中心地址已更新：{u}");
    }
    if let Some(k) = apikey {
        w.set_mirror_apikey(k.trim())?;
        println!("镜像 apikey 已更新");
    }
    // v3.3.0 #7：无参数时展示当前中心与该中心下发的客户端配置（来自本地缓存）
    if server.is_none() && apikey.is_none() {
        let center = if w.info.server_url.trim().is_empty() { "（未配置）" } else { w.info.server_url.as_str() };
        println!("中心地址 : {center}");
        match w.client_config() {
            Some(c) => println!("服务端配置: {}", serde_json::to_string_pretty(&c)?),
            None => println!("服务端配置: （无缓存；设置中心地址或登录后自动拉取）"),
        }
        println!("用法：AlphaCoinClientCli config --server <地址> [--apikey <key>]");
    }
    Ok(())
}

fn cmd_open(fullname: &str) -> Result<()> {
    let w = wallet_ready()?;
    // 加密私钥取本地缓存；CLI 为本地运维工具，不校验口令（服务端亦不存任何口令材料）
    // 缺密文时必须失败：否则会在中心留下一个“无法取回私钥”的账户
    let sek = w.encrypted_seckey().ok_or_else(|| {
        anyhow!("本地缺少加密私钥缓存，无法开立账户；请重建钱包或先登录一次取得密钥")
    })?;
    // CLI 为本地运维工具：以操作员身份明示同意（GUI 端已强制勾选才可自助注册）
    let mut w = w;
    let r = client_api::open_account(&w, &sek, fullname, true, true)?;
    acs_client_core::web_helpers::refresh_client_config(&mut w);
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
    let w = wallet_ready()?;
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
    let mut w = wallet_ready()?;
    // CLI 每次都是新进程：把本次提供的口令设为会话口令，供待收箱查询的 nonce 签名复用
    w.set_session_passphrase(pass);
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

/// 修复本地 gpg 环境：注册/登录报「生成钱包密钥失败」时的现场急救。
///
/// 做三件事（都不动账户与账本，只修 gpg 目录）：
/// 1) 探测信任库是否损坏 —— 损坏则转存备份并重建（本机自有密钥不会丢）；
/// 2) 清理陈旧锁与中断残留的 keybox 临时文件；
/// 3) 可选 `--dedup-keys`：同一 User ID 存在多把密钥时（注册反复失败会留下），只保留最新一把。
fn cmd_repair(dedup_keys: bool) -> Result<()> {
    let w = Wallet::open()?;
    let home = w.gpg.homedir_display();

    println!("修复本地 gpg 环境");
    println!("  数据目录: {}", wallet::data_dir_str().display());
    println!("  gpg homedir: {home}");

    // 1) 信任库
    let trustdb = format!("{home}/trustdb.gpg");
    if std::path::Path::new(&trustdb).exists() {
        if w.gpg.repair_trustdb() {
            println!("  [OK] 信任库已重建（原损坏文件转存为 trustdb.gpg.corrupt-<时间戳>）");
        } else {
            println!("  [--] 信任库正常，无需重建");
        }
    } else {
        println!("  [--] 未发现信任库（下次使用 gpg 时自动创建）");
    }

    // 2) 陈旧锁 / 残留临时文件
    if w.gpg.clean_stale_locks() {
        println!("  [OK] 已清理陈旧锁 / 残留临时文件");
    } else {
        println!("  [--] 无需清理的锁或临时文件");
    }

    // 3) 重复密钥
    if dedup_keys {
        let removed = w.dedup_duplicate_keys()?;
        println!("  [OK] 清理重复密钥 {} 把（同一 UID 只保留最新）", removed);
    }

    // 4) 自检：能否列出密钥
    match w.gpg.list_fingerprints() {
        Ok(list) if list.is_empty() => println!("  [OK] gpg 可用（本机暂无密钥）"),
        Ok(list) => println!("  [OK] gpg 可用，本机密钥 {} 把：{}", list.len(), list.join(", ")),
        Err(e) => {
            println!("  [ERR] gpg 仍不可用：{e}");
            return Err(anyhow!("gpg 环境修复后仍不可用：{e}"));
        }
    }
    println!("修复完成，可重新尝试注册 / 登录。");
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
