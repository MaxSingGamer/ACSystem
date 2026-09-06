//! acs-client Tauri 后端：将钱包操作暴露为 Tauri command，前端通过 `invoke` 调用。
//! v3.0.0：替代原 axum 本地 HTTP 服务，WebView 由 Tauri 承载。
//! v3.1.0：详细 .alphalog（全操作/调用/通讯/输出/错误）、自动更新、协议同意、退出即登出。

pub mod client_api;
pub mod sync;
pub mod txn;
pub mod update;
pub mod wallet;
pub mod web_helpers;

use std::sync::Mutex;

use acs_core::log;
use acs_core::models::AccountType;
use tauri::Manager;

use crate::wallet::Wallet;

/// 默认中心服务器地址（未配置时使用）。
pub const DEFAULT_SERVER: &str = "https://acsystem.maxshin.top";

/// 跨 command 共享的钱包。
pub struct AppState {
    pub wallet: Mutex<Wallet>,
}

type CmdResult<T> = Result<T, String>;

/// 统一错误出口：先写 .alphalog（ERR）再返回给前端。
fn err(e: impl std::fmt::Display) -> String {
    let s = e.to_string();
    log::err(&s);
    s
}

/// 记录一次调用（输入）并执行；成功记 OUT、失败记 ERR。密码等敏感参数勿传入。
fn run<T>(name: &str, args: &str, f: impl FnOnce() -> CmdResult<T>) -> CmdResult<T> {
    log::call(&format!("{name} {args}"));
    match f() {
        Ok(v) => {
            log::out(&format!("{name} 成功"));
            Ok(v)
        }
        Err(e) => {
            log::err(&format!("{name} 失败：{e}"));
            Err(e)
        }
    }
}

fn mask_uid(s: &str) -> String {
    log::mask(s)
}

// ---------- 基础命令 ----------

/// 当前登录态与账户信息。
/// v3.1.0：不再返回本地登录记录（登录界面不展示历史账户）；仅提供当前登录者信息。
#[tauri::command]
fn state(state: tauri::State<AppState>) -> CmdResult<serde_json::Value> {
    log::call("state");
    let w = state.wallet.lock().map_err(err).unwrap();
    let logged_in = w.info.initialized();
    let txs = crate::txn::list_local_tx(&w, 200);
    let outbox = crate::txn::list_outbox(&w);
    let r = serde_json::json!({
        "logged_in": logged_in,
        "uid": mask_uid(&w.info.uid),
        "atype": w.info.atype.as_str(),
        "email": w.info.email,
        "server_url": w.info.server_url,
        "synced_at": w.info.synced_at,
        "balance": w.mirror_balance(),
        "txs": txs,
        "outbox": outbox,
    });
    log::out(&format!("state logged_in={logged_in}"));
    Ok(r)
}

/// 登录：本地缓存取回或向中心 fetch-key。
/// v3.1.0：前端须勾选同意《使用协议》/《隐私政策》才允许调用；标识随请求上送。
#[tauri::command]
fn login(
    state: tauri::State<AppState>,
    uid: String,
    password: String,
    agree_terms: bool,
    agree_privacy: bool,
) -> CmdResult<serde_json::Value> {
    let u = uid.clone();
    run("login", &format!("uid={}", mask_uid(&u)), move || {
        let mut w = state.wallet.lock().map_err(err).unwrap();
        let acc = crate::web_helpers::login_account(&mut w, &uid, &password, agree_terms, agree_privacy)
            .map_err(err)?;
        Ok(serde_json::json!({ "ok": true, "message": format!("欢迎回来，{acc}") }))
    })
}

/// 注册新账户。v3.1.0：须已同意协议；未同意则前端拦截、服务端也驳回。
#[tauri::command]
fn register(
    state: tauri::State<AppState>,
    server_url: String,
    uid: String,
    atype: String,
    email: String,
    password: String,
    agree_terms: bool,
    agree_privacy: bool,
) -> CmdResult<serde_json::Value> {
    let u = uid.clone();
    run("register", &format!("uid={}", mask_uid(&u)), move || {
        let atype = AccountType::from_str(&atype).ok_or_else(|| "无效账户类型".to_string())?;
        let mut w = state.wallet.lock().map_err(err).unwrap();
        crate::web_helpers::register_account(
            &mut w,
            &server_url,
            &uid,
            atype,
            &email,
            &password,
            agree_terms,
            agree_privacy,
        )
        .map_err(err)?;
        Ok(serde_json::json!({ "ok": true, "message": "注册成功，账户已在中心登记" }))
    })
}

/// 退出登录（清除本地当前登录态）。
#[tauri::command]
fn logout(state: tauri::State<AppState>) -> CmdResult<serde_json::Value> {
    run("logout", "", || {
        let mut w = state.wallet.lock().map_err(err).unwrap();
        w.clear_current().map_err(err)?;
        Ok(serde_json::json!({ "ok": true, "message": "已退出登录" }))
    })
}

/// 同步账本（前端按钮文案为「刷新」，自动同步亦调用此命令）。
#[tauri::command(rename = "sync")]
fn sync_now(state: tauri::State<AppState>) -> CmdResult<serde_json::Value> {
    run("sync", "", || {
        let mut w = state.wallet.lock().map_err(err).unwrap();
        if !w.info.initialized() {
            return Err("未登录，无法同步".to_string());
        }
        let r = crate::sync::pull(&w).map_err(err)?;
        let _ = w.mark_synced(r.server_time, None);
        Ok(serde_json::json!({
            "ok": true,
            "message": format!("已同步：新增交易 {}，账户快照 {}", r.txs, r.accounts),
            "txs": r.txs,
            "accounts": r.accounts,
        }))
    })
}

/// 转账（v3.0.0：签名后直接提交中心，无 outbox 二次确认）。
#[tauri::command]
fn transfer(
    state: tauri::State<AppState>,
    to: String,
    amount: i64,
    password: String,
) -> CmdResult<serde_json::Value> {
    let to_mask = mask_uid(&to);
    run("transfer", &format!("to={} amount={}", to_mask, amount), || {
        let w = state.wallet.lock().map_err(err).unwrap();
        let mut r = to.trim().to_string();
        let mut rtype = AccountType::Individual;
        if let Some(i) = r.find('@') {
            rtype = AccountType::from_str(&r[i + 1..]).unwrap_or(AccountType::Individual);
            r = r[..i].to_string();
        }
        if r.is_empty() {
            return Err("请输入接收方 UID".to_string());
        }
        if amount <= 0 {
            return Err("金额须大于 0".to_string());
        }
        let v = crate::txn::build_and_submit_transfer(&w, &r, rtype, amount, &password).map_err(err)?;
        let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
        let tid = v.get("tx_id").and_then(|x| x.as_str()).unwrap_or("");
        Ok(serde_json::json!({ "ok": true, "message": format!("转账已提交：{tid}（状态 {status}）"), "tx_id": tid }))
    })
}

/// 待确认列表（作为接收方）。
#[tauri::command]
fn pending(state: tauri::State<AppState>) -> CmdResult<serde_json::Value> {
    run("pending", "", || {
        let w = state.wallet.lock().map_err(err).unwrap();
        let list = crate::client_api::list_pending(&w).map_err(err)?;
        Ok(serde_json::json!({
            "items": list
                .into_iter()
                .map(|p| {
                    serde_json::json!({
                        "tx_id": p.tx_id, "tx_type": p.tx_type, "sender": p.sender,
                        "amount": p.amount, "timestamp": p.timestamp,
                    })
                })
                .collect::<Vec<_>>(),
        }))
    })
}

/// 确认收款。
#[tauri::command]
fn confirm(
    state: tauri::State<AppState>,
    tx_id: String,
    password: String,
) -> CmdResult<serde_json::Value> {
    let tid = tx_id.clone();
    run("confirm", &format!("tx={:.8}…", tid), || {
        let w = state.wallet.lock().map_err(err).unwrap();
        let r = crate::client_api::confirm_tx(&w, &tx_id, &password, None).map_err(err)?;
        let s = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
        Ok(serde_json::json!({ "ok": true, "message": format!("交易 {:.8}… 状态 → {s}", tx_id) }))
    })
}

/// 拒收交易。
#[tauri::command]
fn reject(
    state: tauri::State<AppState>,
    tx_id: String,
    password: String,
    reason: Option<String>,
) -> CmdResult<serde_json::Value> {
    let tid = tx_id.clone();
    let why = reason.clone().unwrap_or_default();
    run("reject", &format!("tx={:.8}… reason={}", tid, log::mask(&why)), || {
        let w = state.wallet.lock().map_err(err).unwrap();
        let r = crate::client_api::confirm_tx(
            &w,
            &tx_id,
            &password,
            if reason.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
                Some(reason.as_deref().unwrap_or("").trim())
            } else {
                None
            },
        )
        .map_err(err)?;
        let s = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
        Ok(serde_json::json!({ "ok": true, "message": format!("交易 {:.8}… 状态 → {s}", tx_id) }))
    })
}

/// 设置中心地址。
#[tauri::command]
fn set_server(state: tauri::State<AppState>, url: String) -> CmdResult<serde_json::Value> {
    run("set_server", "", || {
        let mut w = state.wallet.lock().map_err(err).unwrap();
        let raw = url.trim().to_string();
        if raw.is_empty() {
            return Err("请输入中心地址".to_string());
        }
        let u = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw
        } else {
            format!("http://{raw}")
        };
        w.set_server_url(&u).map_err(err)?;
        Ok(serde_json::json!({ "ok": true, "message": format!("中心地址已保存：{u}") }))
    })
}

/// 注销账户。
#[tauri::command]
fn delete_account(state: tauri::State<AppState>, password: String) -> CmdResult<serde_json::Value> {
    run("delete_account", "", || {
        let mut w = state.wallet.lock().map_err(err).unwrap();
        let uid = w.info.uid.clone();
        let atype = w.info.atype;
        crate::client_api::close_account(&w, &password).map_err(err)?;
        let _ = w.delete_local_account(&uid, atype);
        let _ = w.clear_current();
        Ok(serde_json::json!({
            "ok": true,
            "message": format!("账户 {uid} 已注销：中心状态已改 Deleted（账本只读保留供审计），本机记录已删除"),
        }))
    })
}

/// 获取已认定成员列表。
#[tauri::command]
fn members(state: tauri::State<AppState>) -> CmdResult<serde_json::Value> {
    run("members", "", || {
        let w = state.wallet.lock().map_err(err).unwrap();
        crate::client_api::fetch_members(&w).map_err(err)
    })
}

// ---------- 协议 / 隐私（本地共享文本，离线可读） ----------

/// 返回《使用协议》/《隐私政策》HTML。doc: individual-terms / enterprise-terms / privacy
#[tauri::command]
fn legal_doc(doc: String) -> CmdResult<String> {
    log::call(&format!("legal_doc doc={}", log::mask(&doc)));
    acs_core::legal::doc_html(&doc)
        .ok_or_else(|| "文档不存在".to_string())
}

// ---------- 自动更新 ----------

/// 检查更新：优先 GitHub，不可达/慢则服务器源。返回统一信息结构。
#[tauri::command]
async fn check_update(state: tauri::State<'_, AppState>) -> CmdResult<serde_json::Value> {
    log::call("check_update");
    let server_url = state.wallet.lock().map_err(err)?.info.server_url.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> CmdResult<serde_json::Value> {
        // 在线程中独立打开钱包（不跨线程持有共享锁），沿用已配置的中心地址
        let mut w2 = Wallet::open().map_err(|e| e.to_string())?;
        if !server_url.is_empty() {
            let _ = w2.set_server_url(&server_url);
        }
        crate::update::check(&w2).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("更新检查线程异常：{e}"))?;
    match &res {
        Ok(_) => log::out("check_update 完成"),
        Err(e) => log::err(&format!("check_update: {e}")),
    }
    res
}

/// 下载更新安装包到系统下载目录；返回 { path, filename, bytes, source }。
#[tauri::command]
async fn download_update(
    state: tauri::State<'_, AppState>,
    source: String,
    version: String,
) -> CmdResult<serde_json::Value> {
    log::call(&format!("download_update source={} version={}", log::mask(&source), log::mask(&version)));
    let server_url = state.wallet.lock().map_err(err)?.info.server_url.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> CmdResult<serde_json::Value> {
        let mut w2 = Wallet::open().map_err(|e| e.to_string())?;
        if !server_url.is_empty() {
            let _ = w2.set_server_url(&server_url);
        }
        crate::update::download(&w2, &source, &version).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("下载线程异常：{e}"))?;
    match &res {
        Ok(_) => log::out("download_update 完成"),
        Err(e) => log::err(&format!("download_update: {e}")),
    }
    res
}

/// 打开安装包所在目录（下载完成后引导用户安装）。
#[tauri::command]
fn reveal(path: String) -> CmdResult<serde_json::Value> {
    log::call(&format!("reveal path={}", log::mask(&path)));
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err("文件不存在".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&path)
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(dir) = p.parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
    Ok(serde_json::json!({ "ok": true }))
}

// ---------- 入口 ----------

/// 启动 Tauri 应用（入口）。
pub fn app_main() {
    tauri::Builder::default()
        .manage(AppState {
            wallet: Mutex::new(Wallet::open().expect("打开钱包失败")),
        })
        .invoke_handler(tauri::generate_handler![
            state, login, register, logout, sync_now, transfer, pending, confirm, reject,
            set_server, delete_account, members, legal_doc, check_update, download_update,
            reveal, quit,
        ])
        .build(tauri::generate_context!())
        .expect("启动 Tauri 应用失败")
        .run(|app, event| {
            // 退出（关窗 / 退出程序）即退出登录：立即清除本地当前登录态
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(st) = app.try_state::<AppState>() {
                    if let Ok(mut w) = st.wallet.lock() {
                        let _ = w.clear_current();
                    }
                }
                log::info("应用退出，已退出登录");
            }
        });
}

/// 退出程序：先退出登录（清当前登录态），再结束进程。
#[tauri::command]
fn quit(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> CmdResult<serde_json::Value> {
    log::call("quit");
    let mut w = state.wallet.lock().map_err(err).unwrap();
    let _ = w.clear_current();
    drop(w);
    app.exit(0);
    Ok(serde_json::json!({ "ok": true, "message": "已退出" }))
}
