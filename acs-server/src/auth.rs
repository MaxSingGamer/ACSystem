//! 后台管理员登录、改密、密钥管理（AES 加密 key passphrase）。

use axum::extract::{FromRequestParts, State};
use axum::http::header;
use axum::http::request::Parts;
use axum::Json;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use acs_core::errors::Result as CoreResult;
use acs_core::gpg::GpgUtil;
use acs_core::models::{AdminRole, GeneratedKey};

use crate::api::{ApiErr, ApiResult};
use crate::state::{AppState, Session};

/// sha256(salt:password) 十六进制（仅存哈希）。
pub fn hash_password(password: &str, salt: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(password.as_bytes());
    hex::encode(h.finalize())
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
    let conn = st.db.lock().unwrap();
    let row: Option<(i64, String, String, bool)> = conn
        .query_row(
            "SELECT id, role, password_hash, must_change_password FROM admins \
             WHERE uid=?1 AND status='Active'",
            params![req.uid.clone()],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()
        .map_err(ApiErr::from_err)?;
    let Some((id, role, pw_hash, must_change)) = row else {
        return Err(ApiErr::unauthorized("账号或密码错误"));
    };
    let (salt, stored) = pw_hash.split_once('$').unwrap_or(("", &pw_hash));
    if hash_password(&req.password, salt) != stored {
        return Err(ApiErr::unauthorized("账号或密码错误"));
    }
    let role = AdminRole::from_str(&role).ok_or_else(|| ApiErr::internal("后台角色配置异常"))?;

    let token = uuid::Uuid::new_v4().to_string();
    let expires_at = Utc::now().timestamp() + st.token_ttl_secs;
    st.sessions.lock().unwrap().insert(
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
    st.sessions.lock().unwrap().remove(&auth.token);
    st.audit_unlocked.lock().unwrap().remove(&auth.token);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn me(auth: AuthUser) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "id": auth.admin_id,
        "uid": auth.username,
        "role": auth.role.as_str(),
        "must_change_password": auth.must_change_password,
    })))
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

        let mut sessions = state.sessions.lock().unwrap();
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

/// 改密规则：
/// - 可改自己密码（old_password 必填）
/// - 同级别互不可改
/// - 下级不可改上级（finance 不可改 root）
/// - root 可改 finance 的密码（target_uid 指定，无需旧密码）
pub async fn change_password(
    State(st): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ChangePwdReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.new_password.len() < 8 {
        return Err(ApiErr::bad_request("新密码至少 8 位"));
    }
    let conn = st.db.lock().unwrap();

    // 解析目标：(id, uid, role, pw_hash, key_enc)
    let (target_id, target_uid, _target_role, _target_pw_hash, target_key_enc) = match &req.target_uid {
        Some(tuid) => {
            // 修改他人：仅 root 可修改 finance
            if !auth.is_root() {
                return Err(ApiErr::forbidden("仅根管理员可重置其他账号密码"));
            }
            let (id, role, pw_hash, key_enc) =
                admin_row(&conn, tuid).ok_or_else(|| ApiErr::not_found("目标账号不存在"))?;
            if role == AdminRole::Root {
                return Err(ApiErr::forbidden("同级别（根管理员）不可互相修改密码"));
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

    let salt = uuid::Uuid::new_v4().to_string();
    let new_hash = format!("{salt}${}", hash_password(&req.new_password, &salt));

    // 密钥 passphrase 联动
    // - root 重置他人：旧 passphrase 不可知 → 用新密码重建密钥
    // - 首次强制改密 / 普通改自己：解密既有 passphrase，用新密码重加密
    let regenerate = req.target_uid.is_some();
    let new_key_enc = if regenerate {
        let gk = st
            .gpg
            .generate_key(&format!("{target_uid} <{target_uid}@aeu.admin>"), &req.new_password)
            .map_err(ApiErr::from)?;
        conn.execute(
            "UPDATE admins SET pubkey=?1, encrypted_seckey=?2, fingerprint=?3, key_passphrase_enc=?4 WHERE id=?5",
            params![gk.pubkey, gk.encrypted_seckey, gk.fingerprint,
                    crate::crypto::encrypt_secret(&req.new_password, &req.new_password), target_id],
        )
        .map_err(ApiErr::from_err)?;
        crate::crypto::encrypt_secret(&req.new_password, &req.new_password)
    } else {
        let old = if auth.must_change_password {
            // 首次强制改密：会话持有登录（默认）密码，解开既有密钥 passphrase
            st.sessions
                .lock()
                .unwrap()
                .get(&auth.token)
                .and_then(|s| s.pending_pwd.clone())
                .ok_or_else(|| ApiErr::forbidden("会话状态异常，请重新登录"))?
        } else {
            req.old_password.as_deref().unwrap_or("").to_string()
        };
        let kp = if target_key_enc.is_empty() {
            old.clone()
        } else {
            crate::crypto::decrypt_secret(&target_key_enc, &old)
                .map_err(|_| ApiErr::forbidden("密钥解密失败"))?
        };
        crate::crypto::encrypt_secret(&req.new_password, &kp)
    };

    conn.execute(
        "UPDATE admins SET password_hash=?1, must_change_password=0, key_passphrase_enc=?2 WHERE id=?3",
        params![new_hash, new_key_enc, target_id],
    )
    .map_err(ApiErr::from_err)?;
    // 改密成功后清除会话中暂存的登录密码
    if req.target_uid.is_none() {
        if let Some(s) = st.sessions.lock().unwrap().get_mut(&auth.token) {
            s.pending_pwd = None;
        }
    }
    crate::api::audit::log_audit(&conn, &auth.username, "change_password", &format!("uid={target_uid}"));
    Ok(Json(serde_json::json!({ "ok": true, "uid": target_uid })))
}

// ---------- 管理员密钥 ----------

/// 为管理员生成 gpg 密钥对并存储（含 AES 加密的 key passphrase）。
pub fn ensure_admin_keys(conn: &rusqlite::Connection, gpg: &GpgUtil, id: i64, uid: &str, login_pwd: &str) -> CoreResult<()> {
    let existing: String = conn
        .query_row("SELECT pubkey FROM admins WHERE id=?1", params![id], |r| r.get(0))
        .unwrap_or_default();
    if !existing.is_empty() {
        return Ok(());
    }
    let gk: GeneratedKey = gpg
        .generate_key(&format!("{uid} <{uid}@aeu.admin>"), login_pwd)?;
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
                AdminRole::from_str(&r.get::<_, String>(1)?).unwrap_or(AdminRole::Finance),
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn verify_hash(stored: &str, password: &str) -> bool {
    let (salt, hash) = stored.split_once('$').unwrap_or(("", stored));
    hash_password(password, salt) == hash
}
