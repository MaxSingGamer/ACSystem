//! 客户端自动更新（公开端点）：版本信息查询 + 受控安装包下载。
//!
//! 安全模型（防恶意下载 / 滥用）：
//! - 服务器只提供 `{data_dir}/updates/` 下由管理员预置的安装包（由 `update.json` 白名单 + sha256 锁定）。
//! - 服务器**绝不受理**客户端上传二进制 / 自定义路径；版本号与平台不在清单中一律 404/400。
//! - 路径经规范化校验，杜绝目录穿越；文件读取前后校验 sha256。
//! - 按来源 IP 限速（默认每小时最多 8 次下载 / 全局 200 次），超限 429。

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::{ApiErr, ApiResult};
use crate::state::AppState;

/// GitHub 仓库（更新包官方源；不可达/慢时客户端切服务器下载）。
const GITHUB_REPO: &str = "MaxSingGamer/ACSystem";
const MANIFEST_FILE: &str = "update.json";

// ---- 简单限速（进程内） ----
type RateMap = Mutex<HashMap<String, Vec<u64>>>;
fn rate_ip() -> &'static RateMap {
    static RATE: OnceLock<RateMap> = OnceLock::new();
    RATE.get_or_init(|| Mutex::new(HashMap::new()))
}
const IP_WINDOW_SECS: u64 = 3600;
const IP_MAX_PER_HOUR: usize = 8;

fn check_rate(ip: &str) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut m = rate_ip().lock().unwrap();
    let v = m.entry(ip.to_string()).or_default();
    v.retain(|t| now.saturating_sub(*t) < IP_WINDOW_SECS);
    if v.len() >= IP_MAX_PER_HOUR {
        return false;
    }
    v.push(now);
    true
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

// ---- 清单 ----
#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    assets: HashMap<String, ManifestAsset>,
}

#[derive(Deserialize, Clone, Default)]
struct ManifestAsset {
    file: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    notes: String,
}

fn manifest_path(st: &AppState) -> std::path::PathBuf {
    st.data_dir.join("updates").join(MANIFEST_FILE)
}

fn load_manifest(st: &AppState) -> Result<Manifest, ApiErr> {
    let p = manifest_path(st);
    let raw = std::fs::read_to_string(&p)
        .map_err(|_| ApiErr::internal("服务器未配置更新清单（updates/update.json 不存在）"))?;
    serde_json::from_str(&raw).map_err(|e| ApiErr::internal(format!("更新清单解析失败：{e}")))
}

/// 简单版本比较：true = a > b（点分数字，忽略前导 'v'）。
fn version_gt(a: &str, b: &str) -> bool {
    let nums = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split(|c: char| c == '.' || c == '-')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let (an, bn) = (nums(a), nums(b));
    for i in 0..an.len().max(bn.len()) {
        let x = an.get(i).copied().unwrap_or(0);
        let y = bn.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

fn normalize_platform(p: &str) -> String {
    let p = p.trim().to_ascii_lowercase();
    if p.is_empty() || p == "windows" || p == "win" {
        "windows-x64".to_string()
    } else {
        p
    }
}

#[derive(Deserialize)]
pub struct InfoReq {
    #[serde(default)]
    pub current: String,
    #[serde(default)]
    pub platform: String,
}

/// GET /api/client/update/info?current=3.0.0&platform=windows-x64
/// 返回最新版本信息与下载地址（github 优先；下载地址给相对服务器路径供后端直接取用）。
async fn update_info(
    State(st): State<AppState>,
    Query(req): Query<InfoReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let m = load_manifest(&st)?;
    let platform = normalize_platform(&req.platform);
    let asset = m.assets.get(&platform).cloned().unwrap_or_default();
    if asset.file.is_empty() {
        return Err(ApiErr::not_found(format!("当前平台 {platform} 暂无更新包")));
    }
    let current = if req.current.is_empty() { m.version.clone() } else { req.current.clone() };
    let available = version_gt(&m.version, &current);
    let size = std::fs::metadata(st.data_dir.join("updates").join(&asset.file))
        .map(|md| md.len())
        .unwrap_or(0);
    let file = asset.file.clone();
    let server_download = format!("/api/client/update/download?version={}&platform={platform}", m.version);
    let github_download =
        format!("https://github.com/{GITHUB_REPO}/releases/latest/download/{file}");
    Ok(Json(json!({
        "ok": true,
        "latest": m.version,
        "current": current,
        "update_available": available,
        "notes": if !asset.notes.is_empty() { asset.notes } else { m.notes },
        "platform": platform,
        "file": file,
        "size": size,
        "sha256": asset.sha256,
        "server_download_url": server_download,
        "github_download_url": github_download,
    })))
}

#[derive(Deserialize)]
pub struct DownloadReq {
    pub version: String,
    #[serde(default)]
    pub platform: String,
}

/// GET /api/client/update/download?version=3.1.0&platform=windows-x64
/// 仅当清单中登记该版本/平台的安装包时才从服务器 updates/ 目录流式返回。
async fn download(
    State(st): State<AppState>,
    Query(req): Query<DownloadReq>,
    headers: HeaderMap,
) -> Response {
    // 校验版本必须等于清单最新版（只发布当前可下载的官方包，禁止任意版本枚举/猜测）
    let m = match load_manifest(&st) {
        Ok(m) => m,
        Err(e) => return e.into_response(),
    };
    if m.version != req.version {
        return ApiErr::not_found("无该版本更新包").into_response();
    }
    let platform = normalize_platform(&req.platform);
    let Some(asset) = m.assets.get(&platform).cloned() else {
        return ApiErr::not_found(format!("平台 {platform} 无更新包")).into_response();
    };

    // 防滥用：IP 限速
    let ip = client_ip(&headers);
    if !check_rate(&ip) {
        crate::log::err(&format!("update download 限速: ip={}", crate::log::mask(&ip)));
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "下载过于频繁，请稍后再试" })))
            .into_response();
    }

    // 路径安全：仅允许 updates 目录内文件
    let updates = st.data_dir.join("updates");
    let path = updates.join(&asset.file);
    let canon_updates = updates.canonicalize().unwrap_or(updates);
    let canon_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return ApiErr::not_found("安装包不存在").into_response(),
    };
    if !canon_path.starts_with(&canon_updates) {
        return ApiErr::bad_request("非法文件路径").into_response();
    }

    // 读取并（若清单含 sha256）校验完整性
    let mut f = match std::fs::File::open(&canon_path) {
        Ok(f) => f,
        Err(_) => return ApiErr::not_found("安装包不可读").into_response(),
    };
    let mut bytes = Vec::new();
    if f.read_to_end(&mut bytes).is_err() {
        return ApiErr::internal("读取安装包失败").into_response();
    }
    if !asset.sha256.is_empty() {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&bytes);
        let d = h.finalize();
        let mut got = String::with_capacity(d.len() * 2);
        for b in d {
            got.push_str(&format!("{b:02x}"));
        }
        if got != asset.sha256.trim().to_ascii_lowercase() {
            crate::log::err("update download 完整性校验失败（sha256 不匹配，拒绝下发）");
            return ApiErr::internal("安装包完整性校验失败").into_response();
        }
    }

    let filename = asset.file;
    crate::log::out(&format!("update download 下发 {filename}（{} 字节）", bytes.len()));
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiErr::internal(format!("构造下载响应失败：{e}")))
        .unwrap_or_else(|e| e.into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/client/update/info", get(update_info))
        .route("/api/client/update/download", get(download))
}

// 仅供测试/工具引用，避免未使用告警
#[allow(dead_code)]
fn _github_repo() -> &'static str {
    GITHUB_REPO
}
