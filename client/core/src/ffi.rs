//! C ABI（D12）：供 QT / C++ 客户端通过 `acs_client_core.dll` 调用。
//!
//! # 调用约定
//!
//! ```c
//! uint32_t acs_abi_version(void);                       // 不匹配就别调后面的
//! const char* acs_version(void);                        // 静态字符串，**不要 free**
//! char* acs_call(const char* method, const char* req);  // 返回值需 acs_string_free
//! void  acs_string_free(char* p);
//! ```
//!
//! `acs_call` 的返回永远是 UTF-8 JSON 信封：
//! - 成功：`{"ok":true,"data":...}`
//! - 失败：`{"ok":false,"error":"...","code":"invalid_json|unknown_method|internal"}`
//!
//! 设计约束（D12）：
//! 1. **不得跨边界传明文密钥/口令缓存**：口令只在单次调用的入参里出现，库内即用即弃，
//!    不写入任何文件、不在返回值里回显（日志也经 `mask` 打码）；
//! 2. **不得 panic 穿透**：每个导出函数都 `catch_unwind`，panic 转成 `ok:false`；
//! 3. 字符串所有权：返回值由 Rust 用 `CString` 分配，调用方用完必须 `acs_string_free`；
//!    入参指针必须是以 NUL 结尾的 UTF-8 字符串，可为 NULL（按空串处理）；
//! 4. 每个方法都有 Rust 侧单测（正常 + 错误路径）。
//!
//! # 方法（method → request JSON）
//!
//! | method | 请求 | 返回 |
//! |---|---|---|
//! | `info` | `{}` | `{version, abi_version}` |
//! | `brand` | `{}` | 品牌/部署信息（来自服务端配置缓存） |
//! | `state` | `{}` | 登录态、账户、余额、outbox |
//! | `set_server` | `{url}` | `{ok}`（会拉取并缓存客户端配置） |
//! | `login` | `{uid,password,agree_terms,agree_privacy}` | `{message}` |
//! | `register` | `{server_url,uid,type,fullname,email,password,agree_terms,agree_privacy}` | `{message}` |
//! | `logout` | `{}` | `{message}` |
//! | `sync` | `{}` | `{txs,accounts,server_time}` |
//! | `ledger` | `{}` | `{txs}` |
//! | `pending` | `{password?}` | `{items}` |
//! | `transfer` | `{to,type,amount,password}` | 提交结果 |
//! | `confirm` | `{tx_id,password}` | `{message}` |
//! | `reject` | `{tx_id,password,reason?}` | `{message}` |
//! | `delete_account` | `{password}` | `{message}` |
//! | `legal_doc` | `{doc}` | `{html}` |
//! | `fetch_config` | `{}` | 服务端 `/api/client/config` 原文（并缓存） |

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use acs_core::models::AccountType;

use crate::wallet::Wallet;

/// ABI 版本：导出接口不兼容变更时 +1（宿主应校验）。
pub const ABI_VERSION: u32 = 1;

/// 进程内单例钱包（QT 侧一个进程一套钱包；多中心切换由 set_server 负责）。
static WALLET: Mutex<Option<Wallet>> = Mutex::new(None);

fn wallet_with<T>(f: impl FnOnce(&mut Wallet) -> Result<T>) -> Result<T> {
    let mut guard = WALLET.lock().map_err(|_| anyhow!("钱包状态锁损坏"))?;
    if guard.is_none() {
        *guard = Some(Wallet::open()?);
    }
    f(guard.as_mut().expect("wallet initialized"))
}

fn envelope_ok(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn envelope_err(code: &str, msg: impl std::fmt::Display) -> Value {
    json!({ "ok": false, "code": code, "error": msg.to_string() })
}

/// 解析入参：NULL / 空串 → `{}`；非法 JSON → 错误。
fn parse_req(req: Option<&str>) -> Result<Value> {
    match req.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => Ok(json!({})),
        Some(s) => serde_json::from_str::<Value>(s).map_err(|e| anyhow!("请求 JSON 非法：{e}")),
    }
}

fn s_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn s_field_req(v: &Value, key: &str) -> Result<String> {
    let s = s_field(v, key);
    if s.trim().is_empty() {
        Err(anyhow!("缺少参数 {key}"))
    } else {
        Ok(s)
    }
}
fn b_field(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(false)
}

/// 方法分发（可在 Rust 侧单测；C 入口只做指针/JSON 包装）。
pub fn dispatch(method: &str, req: &Value) -> Value {
    match method {
        "info" => envelope_ok(json!({
            "version": crate::VERSION,
            "abi_version": ABI_VERSION,
        })),
        "brand" => match wallet_with(|w| Ok(crate::web_helpers::brand_payload(w))) {
            Ok(v) => envelope_ok(v),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        "state" => match wallet_with(|w| {
            let accounts: Vec<Value> = w
                .list_local_accounts()
                .into_iter()
                .map(|a| {
                    json!({
                        "uid": a.uid,
                        "type": a.atype.as_str(),
                        "email": a.email,
                        "last_login": a.last_login,
                    })
                })
                .collect();
            Ok(json!({
                "logged_in": w.info.initialized(),
                "uid": w.info.uid,
                "type": w.info.atype.as_str(),
                "email": w.info.email,
                "server_url": w.info.server_url,
                "synced_at": w.info.synced_at,
                "balance": w.mirror_balance(),
                "status": w.mirror_status(),
                "outbox": crate::txn::list_outbox(w),
                "accounts": accounts,
            }))
        }) {
            Ok(v) => envelope_ok(v),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        "set_server" => {
            let url = match s_field_req(req, "url") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let mut u = url.trim().trim_end_matches('/').to_string();
            if !u.starts_with("http://") && !u.starts_with("https://") {
                u = format!("http://{u}");
            }
            match wallet_with(|w| {
                w.set_server_url(&u)?;
                crate::web_helpers::refresh_client_config(w);
                Ok(())
            }) {
                Ok(()) => envelope_ok(json!({ "server_url": u })),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "login" => {
            let uid = match s_field_req(req, "uid") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let pwd = s_field(req, "password");
            let agree_terms = b_field(req, "agree_terms");
            let agree_privacy = b_field(req, "agree_privacy");
            match wallet_with(|w| {
                let acc = crate::web_helpers::login_account(w, &uid, &pwd, agree_terms, agree_privacy)?;
                // 会话内缓存口令（仅内存）；出参不回显
                w.set_session_passphrase(&pwd);
                Ok(acc)
            }) {
                Ok(acc) => envelope_ok(json!({ "message": format!("欢迎回来，{acc}") })),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "register" => {
            let uid = match s_field_req(req, "uid") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let atype = match AccountType::from_str(&s_field(req, "type")) {
                Some(t) => t,
                None => return envelope_err("invalid_json", "无效账户类型"),
            };
            let agree_terms = b_field(req, "agree_terms");
            let agree_privacy = b_field(req, "agree_privacy");
            match wallet_with(|w| {
                crate::web_helpers::register_account(
                    w,
                    &s_field(req, "server_url"),
                    &uid,
                    atype,
                    &s_field(req, "fullname"),
                    &s_field(req, "email"),
                    &s_field(req, "password"),
                    agree_terms,
                    agree_privacy,
                )?;
                Ok(())
            }) {
                Ok(()) => envelope_ok(json!({ "message": "注册成功" })),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "logout" => match wallet_with(|w| {
            w.clear_session_passphrase();
            w.clear_current()?;
            Ok(())
        }) {
            Ok(()) => envelope_ok(json!({ "message": "已退出登录" })),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        "sync" => match wallet_with(|w| {
            if !w.info.initialized() {
                return Err(anyhow!("未登录，无法同步"));
            }
            let r = crate::sync::pull(w)?;
            if r.accounts == 0 {
                return Err(anyhow!("中心未返回账户快照，本次同步未生效"));
            }
            w.mark_synced(r.server_time, None)?;
            w.reload_info();
            Ok(r)
        }) {
            Ok(r) => envelope_ok(json!({
                "txs": r.txs, "accounts": r.accounts, "server_time": r.server_time,
            })),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        "ledger" => match wallet_with(|w| crate::sync::fetch_ledger(w)) {
            Ok(txs) => envelope_ok(json!({ "txs": txs })),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        "pending" => {
            let pwd = s_field(req, "password");
            match wallet_with(|w| {
                if !pwd.is_empty() {
                    w.set_session_passphrase(&pwd);
                }
                let list = crate::client_api::list_pending(w)?;
                Ok(list
                    .into_iter()
                    .map(|p| {
                        json!({
                            "tx_id": p.tx_id, "tx_type": p.tx_type, "sender": p.sender,
                            "amount": p.amount, "timestamp": p.timestamp,
                        })
                    })
                    .collect::<Vec<_>>())
            }) {
                Ok(items) => envelope_ok(json!({ "items": items })),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "transfer" => {
            let to = match s_field_req(req, "to").or_else(|_| s_field_req(req, "receiver")) {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let pwd = match s_field_req(req, "password") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let amount = req.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
            if amount <= 0 {
                return envelope_err("invalid_json", "金额必须为正整数");
            }
            let atype = match AccountType::from_str(&s_field(req, "type")) {
                Some(t) => t,
                None => AccountType::Individual,
            };
            match wallet_with(|w| crate::txn::build_and_submit_transfer(w, &to, atype, amount, &pwd)) {
                Ok(v) => envelope_ok(v),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "confirm" | "reject" => {
            let tx_id = match s_field_req(req, "tx_id") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let pwd = match s_field_req(req, "password") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            let reason = if method == "reject" && !s_field(req, "reason").is_empty() {
                Some(s_field(req, "reason"))
            } else {
                None
            };
            match wallet_with(|w| {
                let r = crate::client_api::confirm_tx(w, &tx_id, &pwd, reason.as_deref())?;
                w.set_session_passphrase(&pwd);
                Ok(r)
            }) {
                Ok(r) => envelope_ok(r),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "delete_account" => {
            let pwd = match s_field_req(req, "password") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            match wallet_with(|w| {
                crate::client_api::close_account(w, &pwd)?;
                Ok(())
            }) {
                Ok(()) => envelope_ok(json!({ "message": "账户已注销" })),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "legal_doc" => {
            let doc = s_field(req, "doc");
            match acs_core::legal::doc_html(&doc) {
                Some(html) => envelope_ok(json!({ "html": html })),
                None => envelope_err("invalid_json", format!("文档不存在：{doc}")),
            }
        }
        // v3.3.0 #8/#15：启动页的中心列表（含服务名称）与删除历史
        "centers" => match crate::wallet::list_centers() {
            Ok(items) => envelope_ok(json!({
                "items": items,
                "current": crate::wallet::current_center_domain(),
            })),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        "center_remove" => {
            let domain = match s_field_req(req, "domain") {
                Ok(v) => v,
                Err(e) => return envelope_err("invalid_json", e),
            };
            match crate::wallet::remove_center(&domain) {
                Ok(()) => envelope_ok(json!({ "ok": true })),
                Err(e) => envelope_err("internal", format!("{e:#}")),
            }
        }
        "fetch_config" => match wallet_with(|w| {
            let cfg = crate::client_api::fetch_config(w)?;
            w.set_client_config(&cfg)?;
            Ok(cfg)
        }) {
            Ok(cfg) => envelope_ok(cfg),
            Err(e) => envelope_err("internal", format!("{e:#}")),
        },
        other => envelope_err("unknown_method", format!("未知方法：{other}")),
    }
}

// ---------------------------------------------------------------------------
// C ABI 导出（QT 侧唯一需要链接的 4 个函数）
// ---------------------------------------------------------------------------

/// ABI 版本（宿主启动时先校验；与 `acs_version()` 一起判断 DLL 是否匹配）。
#[unsafe(no_mangle)]
pub extern "C" fn acs_abi_version() -> u32 {
    ABI_VERSION
}

/// 产品版本（静态字符串，**调用方不要 free**）。
#[unsafe(no_mangle)]
pub extern "C" fn acs_version() -> *const c_char {
    // 用 OnceLock 保证指针在整个进程生命周期有效（静态变量也可，但 CString 不能用 const）
    static VERSION_C: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    VERSION_C
        .get_or_init(|| CString::new(crate::VERSION).unwrap_or_else(|_| CString::new("?").unwrap()))
        .as_ptr()
}

/// 调用一个方法；返回 UTF-8 JSON（见模块文档），用完必须 `acs_string_free`。
#[unsafe(no_mangle)]
pub extern "C" fn acs_call(method: *const c_char, req: *const c_char) -> *mut c_char {
    // 1) 指针 → &str（非法 UTF-8 也转成错误信封，而不是 panic）
    let method_s = unsafe { cstr_to_string(method) };
    let req_s = unsafe { cstr_to_string(req) };
    let m = match method_s {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            return to_c_ptr(&envelope_err("invalid_json", "method 不能为空").to_string());
        }
    };
    let payload = match parse_req(req_s.as_deref()) {
        Ok(v) => v,
        Err(e) => return to_c_ptr(&envelope_err("invalid_json", e).to_string()),
    };
    // 2) 分发（panic 不得穿透 FFI：捕获后转错误信封）
    let result = catch_unwind(AssertUnwindSafe(|| dispatch(&m, &payload)));
    let out = match result {
        Ok(v) => v,
        Err(_) => envelope_err("internal", "内部 panic（已捕获，未跨越 FFI 边界）"),
    };
    to_c_ptr(&out.to_string())
}

/// 释放 `acs_call` 返回的字符串（传 NULL 安全）。
#[unsafe(no_mangle)]
pub extern "C" fn acs_string_free(p: *mut c_char) {
    if p.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(p));
    }
}

unsafe fn cstr_to_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let c = unsafe { CStr::from_ptr(p) };
    Some(c.to_string_lossy().into_owned())
}

fn to_c_ptr(s: &str) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new(r#"{"ok":false,"code":"internal","error":"响应含 NUL"}"#)
            .expect("static")
            .into_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_json(method: &str, req: &str) -> Value {
        let m = CString::new(method).unwrap();
        let r = CString::new(req).unwrap();
        let p = acs_call(m.as_ptr(), r.as_ptr());
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        acs_string_free(p);
        serde_json::from_str(&s).expect("响应必须是合法 JSON")
    }

    /// D12 要求：ABI 版本与产品版本都可查询。
    #[test]
    fn info_reports_versions() {
        let v = call_json("info", "{}");
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["abi_version"], serde_json::json!(ABI_VERSION));
        assert_eq!(v["data"]["version"], serde_json::json!(crate::VERSION));
        assert_eq!(acs_abi_version(), ABI_VERSION);
        let vp = acs_version();
        assert!(!vp.is_null());
        assert_eq!(unsafe { CStr::from_ptr(vp) }.to_string_lossy(), crate::VERSION);
    }

    /// 错误路径：未知方法 / 非法 JSON / 空方法都不 panic，且带 code。
    #[test]
    fn errors_are_structured() {
        let v = call_json("no_such_method", "{}");
        assert_eq!(v["ok"], false);
        assert_eq!(v["code"], "unknown_method");

        let v = call_json("info", "{not json");
        assert_eq!(v["code"], "invalid_json");

        let v = call_json("", "{}");
        assert_eq!(v["code"], "invalid_json");
    }

    /// 缺参校验（login 无 uid）：返回 invalid_json，而不是 mock 网络。
    #[test]
    fn missing_required_fields() {
        let v = call_json("login", "{}");
        assert_eq!(v["ok"], false);
        assert_eq!(v["code"], "invalid_json");
    }

    /// 纯本地方法：legal_doc 正常/不存在两条路径。
    #[test]
    fn legal_doc_works() {
        let v = call_json("legal_doc", r#"{"doc":"privacy"}"#);
        assert_eq!(v["ok"], true);
        assert!(v["data"]["html"].as_str().unwrap_or("").contains("<html"));

        let v = call_json("legal_doc", r#"{"doc":"nope"}"#);
        assert_eq!(v["ok"], false);
        assert_eq!(v["code"], "invalid_json");
    }

    /// NULL 入参安全：method=NULL 返回错误信封，req=NULL 视为 {}。
    #[test]
    fn null_pointers_are_safe() {
        let p = acs_call(std::ptr::null(), std::ptr::null());
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        acs_string_free(p);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["ok"], false);

        let m = CString::new("info").unwrap();
        let p = acs_call(m.as_ptr(), std::ptr::null());
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        acs_string_free(p);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["ok"], true);

        // free(NULL) 安全
        acs_string_free(std::ptr::null_mut());
    }
}
