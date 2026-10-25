//! 后台管理员登录、改密、密钥管理（AES 加密 key passphrase）。

use axum::extract::{FromRequestParts, State};
use axum::http::header;
use axum::http::request::Parts;
use axum::Json;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use acs_core::errors::Result as CoreResult;
use acs_core::gpg::GpgUtil;
use acs_core::models::{AdminRole, GeneratedKey};

use crate::api::{ApiErr, ApiResult};
use crate::password;
use crate::state::{self, AppState, LoginFail, Session};

/// 口令哈希与校验统一走 [`crate::password`]：
/// PBKDF2-HMAC-SHA256（60 万轮 + 随机盐），旧版单轮 SHA-256 仍可校验并在登录成功后自动升级。
///
/// 登录失败次数上限（达到后锁定账户）。
const MAX_LOGIN_FAILS: u32 = 5;
/// 锁定基础时长（秒）；每多超一次倍增，封顶 LOCK_MAX_SECS。
const LOCK_BASE_SECS: i64 = 30;
const LOCK_MAX_SECS: i64 = 300;

/// 记录一次管理员登录失败（**持久化**到 admins 表，重启不会清零）。
/// 达到上限后指数退避锁定，封顶 `LOCK_MAX_SECS`。
fn register_admin_fail(st: &AppState, id: i64, uid: &str, now: i64) {
    let conn = state::db_lock(&st.db);
    let cnt: i64 = conn
        .query_row(
            "SELECT fail_count+1 FROM admins WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(1);
    let locked = if cnt >= MAX_LOGIN_FAILS as i64 {
        let over = cnt - MAX_LOGIN_FAILS as i64 + 1;
        now + (LOCK_BASE_SECS * over).min(LOCK_MAX_SECS)
    } else {
        0
    };
    let _ = conn.execute(
        "UPDATE admins SET fail_count=?1, locked_until=?2 WHERE id=?3",
        params![cnt, locked, id],
    );
    if locked > now {
        crate::log::err(format!(
            "管理员口令连续失败已锁定：{uid}（{} 秒）",
            locked - now
        ));
    }
}

/// 记录一次登录失败：达到上限后对账户设置锁定截止时间（指数退避）。
pub(crate) fn register_fail(st: &AppState, uid: &str, now: i64) {
    let mut fails = state::login_fails_lock(&st);
    let e = fails
        .entry(uid.to_string())
        .or_insert(LoginFail { count: 0, locked_until: 0 });
    e.count += 1;
    if e.count >= MAX_LOGIN_FAILS {
        let over = (e.count - MAX_LOGIN_FAILS + 1) as i64;
        let secs = (LOCK_BASE_SECS * over).min(LOCK_MAX_SECS);
        e.locked_until = now + secs;
        // 锁定事件写文件日志：便于运维发现「有人在盯这个账号」
        crate::log::err(format!("口令连续失败已锁定：{uid}（{secs} 秒）"));
    }
    // 防止表无限膨胀：超过阈值时清理已过期的记录。
    if fails.len() > 10_000 {
        fails.retain(|_, f| f.locked_until > now);
    }
}

/// 若 `key` 处于锁定中，返回剩余秒数（供登录与系统账本代管登录共用）。
pub(crate) fn lock_wait(st: &AppState, key: &str, now: i64) -> Option<i64> {
    let fails = state::login_fails_lock(&st);
    fails
        .get(key)
        .filter(|f| f.locked_until > now)
        .map(|f| f.locked_until - now)
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub uid: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AdminInfo {
    pub id: i64,
    pub uid: String,
    pub role: String,
    pub must_change_password: bool,
}

#[derive(Serialize)]
pub struct LoginResp {
    pub token: String,
    pub admin: AdminInfo,
}

pub async fn login(
    State(st): State<AppState>,
    Json(req): Json<LoginReq>,
) -> ApiResult<Json<LoginResp>> {
    let now = Utc::now().timestamp();
    // 读账户行（锁只持有一瞬）：下面的 PBKDF2 校验在锁外做，
    // 否则单次登录会把全局库锁占住 0.2–0.5 秒，阻塞所有其他请求。
    // 失败计数/锁定时间取自数据库（持久化，重启不清零）。
    let row: Option<(i64, String, String, bool, i64)> = {
        let conn = state::db_lock(&st.db);
        conn.query_row(
            "SELECT id, role, password_hash, must_change_password, locked_until FROM admins \
             WHERE uid=?1 AND status='Active'",
            params![req.uid.clone()],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(ApiErr::from_err)?
    };
    let Some((id, role, pw_hash, must_change, locked_until)) = row else {
        // 账号不存在 / 已停用：不回具体原因（防枚举），只记日志
        crate::log::err(format!("后台登录失败（账号不存在或已停用）：uid={}", req.uid));
        return Err(ApiErr::unauthorized("账号或密码错误"));
    };
    if locked_until > now {
        return Err(ApiErr::too_many_requests(format!(
            "尝试过于频繁，请 {} 秒后重试",
            locked_until - now
        )));
    }
    if !password::verify(&req.password, &pw_hash) {
        register_admin_fail(&st, id, &req.uid, now);
        return Err(ApiErr::unauthorized("账号或密码错误"));
    }
    // 登录成功：清零失败记录
    let conn = state::db_lock(&st.db);
    let _ = conn.execute(
        "UPDATE admins SET fail_count=0, locked_until=0 WHERE id=?1",
        params![id],
    );
    // 旧格式（单轮 SHA-256）或迭代次数不足 → 此刻口令明文可得，顺手升级为 PBKDF2
    if password::needs_rehash(&pw_hash) {
        let _ = conn.execute(
            "UPDATE admins SET password_hash=?1 WHERE id=?2",
            params![password::hash(&req.password), id],
        );
        crate::log::info(format!("管理员口令哈希已升级为 PBKDF2：uid={}", req.uid));
    }
    let role = AdminRole::from_str(&role).ok_or_else(|| ApiErr::internal("后台角色配置异常"))?;
    // 记录管理员上次登录时间（复用已持有的 conn：std::sync::Mutex 不可重入，二次 lock 会自死锁）
    let _ = conn.execute(
        "UPDATE admins SET last_login=?1 WHERE id=?2",
        rusqlite::params![Utc::now().timestamp(), id],
    );
    // v3.3.0（D15）：**根管理员登录即解锁主密钥**（用它自己的口令解密其副本，载入内存）。
    // 解锁失败不得阻断登录（例如该 Root 尚无副本/副本损坏）——只记日志。
    if role == AdminRole::Root {
        match unlock_master_with(&conn, id, &req.password) {
            Ok(mk) => {
                *state::master_lock(&st) = Some(mk);
                crate::log::info(format!("主密钥已由根管理员 {} 解锁（仅内存持有）", req.uid));
                let _ = conn.execute(
                    "INSERT INTO audit_log(actor, op, detail, ts) VALUES (?1,'unlock_master',?2,?3)",
                    rusqlite::params![req.uid, format!("admin_id={id}"), Utc::now().timestamp()],
                );
            }
            Err(e) => {
                crate::log::err(format!(
                    "根管理员 {} 登录成功，但主密钥解锁失败（不阻断登录）：{e}",
                    req.uid
                ));
            }
        }
    }

    let token = uuid::Uuid::new_v4().to_string();
    let expires_at = Utc::now().timestamp() + st.token_ttl_secs;
    state::sessions_lock(&st).insert(
        token.clone(),
        Session {
            admin_id: id,
            username: req.uid.clone(),
            role: role.as_str().to_string(),
            must_change_password: must_change,
            // 首次强制改密：短暂持有登录密码，供 change_password 解开既有密钥 passphrase
            pending_pwd: if must_change { Some(req.password.clone()) } else { None },
            expires_at,
        },
    );
    Ok(Json(LoginResp {
        token,
        admin: AdminInfo {
            id,
            uid: req.uid,
            role: role.as_str().to_string(),
            must_change_password: must_change,
        },
    }))
}

pub async fn logout(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    state::sessions_lock(&st).remove(&auth.token);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn me(auth: AuthUser, State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "id": auth.admin_id,
        "uid": auth.username,
        "role": auth.role.as_str(),
        "must_change_password": auth.must_change_password,
        // v3.3.0（D15）：主密钥是否已解锁（未解锁时系统账本相关操作不可用）
        "master_key_unlocked": state::master_lock(&st).is_some(),
    })))
}

/// 用**该根管理员的口令**解密其 `admin_master_key` 副本，返回主密钥明文。
///
/// v3.3.0（D8/D10/D15）：主密钥不落文件；每个 Root 各持一份口令封装的副本。
/// 调用点：
/// - 根管理员登录成功 → 解锁到内存（`state::master_lock`）；
/// - 根管理员改自己密码 → 用旧口令解出、再用新口密封装（见 `change_password`）。
pub(crate) fn unlock_master_with(
    conn: &rusqlite::Connection,
    admin_id: i64,
    login_pwd: &str,
) -> Result<String, String> {
    let wrapped: Option<String> = conn
        .query_row(
            "SELECT wrapped FROM admin_master_key WHERE admin_id=?1",
            params![admin_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(wrapped) = wrapped.filter(|w| !w.is_empty()) else {
        return Err("该根管理员没有主密钥副本（可能未完成初始化）".into());
    };
    crate::crypto::decrypt_secret(&wrapped, login_pwd)
}

/// 为根管理员写入/更新主密钥副本（用其当前口令封装）。
pub(crate) fn upsert_master_copy(
    conn: &rusqlite::Connection,
    admin_id: i64,
    pwd: &str,
    master: &str,
) -> rusqlite::Result<()> {
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT INTO admin_master_key(admin_id, wrapped, created_at, updated_at) VALUES (?1,?2,?3,?3) \
         ON CONFLICT(admin_id) DO UPDATE SET wrapped=excluded.wrapped, updated_at=excluded.updated_at",
        params![admin_id, crate::crypto::encrypt_secret(pwd, master), now],
    )?;
    Ok(())
}

/// 已认证管理员。
pub struct AuthUser {
    pub admin_id: i64,
    pub username: String,
    pub role: AdminRole,
    pub token: String,
    pub must_change_password: bool,
}

impl AuthUser {
    pub fn is_root(&self) -> bool {
        self.role == AdminRole::Root
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiErr;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiErr::unauthorized("缺少令牌"))?;

        let mut sessions = state::sessions_lock(state);
        let s = sessions
            .get(token)
            .cloned()
            .ok_or_else(|| ApiErr::unauthorized("令牌无效或已过期"))?;
        if s.expires_at < Utc::now().timestamp() {
            sessions.remove(token);
            return Err(ApiErr::unauthorized("会话已过期，请重新登录"));
        }
        // 滑动过期：每次操作刷新待机时长
        if let Some(entry) = sessions.get_mut(token) {
            entry.expires_at = Utc::now().timestamp() + state.token_ttl_secs;
        }
        let role = AdminRole::from_str(&s.role).ok_or_else(|| ApiErr::unauthorized("角色无效"))?;
        Ok(AuthUser {
            admin_id: s.admin_id,
            username: s.username,
            role,
            token: token.to_string(),
            must_change_password: s.must_change_password,
        })
    }
}

// ---------- 改密 ----------

#[derive(Deserialize)]
pub struct ChangePwdReq {
    pub old_password: Option<String>,
    pub new_password: String,
    pub target_uid: Option<String>, // root 修改他人时填写
}

/// 改密规则（v3.3.0 D14/D17/D28）：
/// - **改自己密码**：必须校验旧密码（`must_change_password` 首登强制改密时除外，用会话里暂存的密码）。
/// - **Root 强制重置 Admin 密码**：`target_uid` 指定，**无需旧密码**；Admin 无 gpg 密钥，
///   因此只更新口令哈希，不重建/不重封任何密钥材料。
/// - **Root 之间不可互相改密**：Root 账户首启后不可增减，且各自持有独立的 masterseckey 副本，
///   代改会破坏其副本一致性；如需变更由本人登录后改自己密码。
/// - **Admin 不可改任何他人密码**（只能改自己）。
pub async fn change_password(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ChangePwdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.new_password.len() < 8 {
        return Err(ApiErr::bad_request("新密码至少 8 位"));
    }
    let conn = state::db_lock(&st.db);

    // 解析目标：(id, uid, role, pw_hash, key_enc)
    let (target_id, target_uid, target_role, _target_pw_hash, target_key_enc) = match &req.target_uid {
        Some(tuid) => {
            // 修改他人：仅 Root 可重置 **Admin** 的密码
            if !auth.is_root() {
                return Err(ApiErr::forbidden("仅根管理员可重置其他账号密码"));
            }
            let (id, role, pw_hash, key_enc) =
                admin_row(&conn, tuid).ok_or_else(|| ApiErr::not_found("目标账号不存在"))?;
            if role == AdminRole::Root {
                return Err(ApiErr::forbidden(
                    "Root 账户的密码只能由本人修改（Root 之间不可互相重置）",
                ));
            }
            (id, tuid.clone(), role, pw_hash, key_enc)
        }
        None => {
            // 修改自己
            let (id, role, pw_hash, key_enc) =
                admin_row(&conn, &auth.username).ok_or_else(|| ApiErr::unauthorized("账号异常"))?;
            // 首次强制改密：无需旧密码；否则须校验
            if !auth.must_change_password {
                let old = req.old_password.as_deref().unwrap_or("");
                if !verify_hash(&pw_hash, old) {
                    return Err(ApiErr::forbidden("原密码错误"));
                }
            }
            (id, auth.username.clone(), role, pw_hash, key_enc)
        }
    };

    let new_hash = password::hash(&req.new_password);

    // 密钥口令密文联动 —— **仅 Root 有 gpg 密钥**（D28），Admin 的 `key_passphrase_enc` 为空。
    // - Admin（target 为 Admin 或被代改）：只改口令哈希，不碰密钥列。
    // - Root 改自己密码：解出既有 passphrase，用新密码重封。
    // 同时记下「旧口令」，供随后重封主密钥副本（D8/D10）使用。
    let old_pwd_for_master = if auth.must_change_password && req.target_uid.is_none() {
        state::sessions_lock(&st)
            .get(&auth.token)
            .and_then(|s| s.pending_pwd.clone())
    } else {
        req.old_password.clone()
    };
    let new_key_enc = if target_role == AdminRole::Admin || target_key_enc.is_empty() {
        target_key_enc.clone()
    } else {
        let old = if auth.must_change_password {
            // 首次强制改密：会话持有登录（默认）密码，解开既有密钥 passphrase
            state::sessions_lock(&st)
                .get(&auth.token)
                .and_then(|s| s.pending_pwd.clone())
                .ok_or_else(|| ApiErr::forbidden("会话状态异常，请重新登录"))?
        } else {
            req.old_password.as_deref().unwrap_or("").to_string()
        };
        let kp = crate::crypto::decrypt_secret(&target_key_enc, &old)
            .map_err(|_| ApiErr::forbidden("密钥解密失败"))?;
        crate::crypto::encrypt_secret(&req.new_password, &kp)
    };

    conn.execute(
        "UPDATE admins SET password_hash=?1, must_change_password=0, key_passphrase_enc=?2 WHERE id=?3",
        params![new_hash, new_key_enc, target_id],
    )
    .map_err(ApiErr::from_err)?;

    // v3.3.0（D8/D10）：**Root 改自己密码后，必须用新口令重封自己的主密钥副本**，
    // 否则下次冷启动就无法再用新密码解锁主密钥。
    // 用旧口令解出主密钥（不依赖内存态，冷启动也能自愈），再以新口密封装；同时刷新内存态。
    if target_role == AdminRole::Root && req.target_uid.is_none() {
        match old_pwd_for_master {
            Some(old) => match unlock_master_with(&conn, target_id, &old) {
                Ok(mk) => {
                    if let Err(e) = upsert_master_copy(&conn, target_id, &req.new_password, &mk) {
                        crate::log::err(format!("重封主密钥副本失败（admin_id={target_id}）：{e}"));
                    } else {
                        *state::master_lock(&st) = Some(mk);
                        crate::log::info(format!(
                            "根管理员 {} 改密：主密钥副本已用新口令重封",
                            auth.username
                        ));
                    }
                }
                Err(e) => crate::log::err(format!(
                    "根管理员 {} 改密：旧口令无法解出主密钥副本，未能重封：{e}",
                    auth.username
                )),
            },
            None => crate::log::err(format!(
                "根管理员 {} 改密：缺少旧口令，无法重封主密钥副本",
                auth.username
            )),
        }
    }

    // 改密成功后清除会话中暂存的登录密码
    if req.target_uid.is_none() {
        if let Some(s) = state::sessions_lock(&st).get_mut(&auth.token) {
            s.pending_pwd = None;
        }
    }
    crate::api::audit::log_audit(&conn, &auth.username, "change_password", &format!("uid={target_uid}"));
    Ok(Json(serde_json::json!({ "ok": true, "uid": target_uid })))
}

// ---------- 管理员密钥 ----------

/// 为 **Root 管理员**生成 gpg 密钥对并存储（含 AES 加密的 key passphrase）。
///
/// v3.3.0（D28）：**只对 Root 调用**。Admin 不签名、不需要 gpg 密钥，
/// 创建 Admin 时**不得**调用本函数（否则会为其无谓生成密钥对）。
pub fn ensure_admin_keys(conn: &rusqlite::Connection, gpg: &GpgUtil, id: i64, uid: &str, login_pwd: &str) -> CoreResult<()> {
    let existing: String = conn
        .query_row("SELECT pubkey FROM admins WHERE id=?1", params![id], |r| r.get(0))
        .unwrap_or_default();
    if !existing.is_empty() {
        return Ok(());
    }
    let gk: GeneratedKey = gpg
        .generate_key(&acs_core::brand::brand().admin_gpg_uid(uid), login_pwd)?;
    let key_enc = crate::crypto::encrypt_secret(login_pwd, login_pwd);
    conn.execute(
        "UPDATE admins SET pubkey=?1, encrypted_seckey=?2, fingerprint=?3, key_passphrase_enc=?4 WHERE id=?5",
        params![gk.pubkey, gk.encrypted_seckey, gk.fingerprint, key_enc, id],
    )?;
    Ok(())
}

fn admin_row(conn: &rusqlite::Connection, uid: &str) -> Option<(i64, AdminRole, String, String)> {
    conn.query_row(
        "SELECT id, role, password_hash, key_passphrase_enc FROM admins WHERE uid=?1",
        params![uid],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                AdminRole::from_str(&r.get::<_, String>(1)?).unwrap_or(AdminRole::Admin),
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// 校验存量哈希（兼容旧格式）。
fn verify_hash(stored: &str, password: &str) -> bool {
    password::verify(password, stored)
}
