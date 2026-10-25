//! 管理员口令哈希（PBKDF2-HMAC-SHA256）。
//!
//! 为什么不用单轮 SHA-256：单卡 GPU 每秒可尝试约 10^10 次 SHA-256，8 位纯小写口令
//! （26^8 ≈ 2×10^11）几十秒即可穷尽；词库口令几乎瞬间命中。管理中心端口已对公网开放，
//! 一旦库文件泄露（备份/主机被入侵）后果不可接受。
//!
//! 存储格式（自描述，盐与迭代次数随哈希一起存）：
//! ```text
//! pbkdf2-sha256$<迭代次数>$<盐 hex>$<派生密钥 hex>
//! ```
//! 旧格式 `sha256("salt:password")` 存为 `<盐>$<哈希 hex>`：仍可校验，且在登录成功时
//! 自动重哈希升级（见 `auth::login`）。
//!
//! 注：手写 PBKDF2 以避免引入新依赖（本仓库仅用 sha2 已有能力）。

use sha2::{Digest, Sha256};

/// 默认迭代次数（可用 `ACS_PBKDF2_ITERATIONS` 定制；下限 1 万，防止被配成无意义的低轮数）。
const DEFAULT_ITERATIONS: u32 = 600_000;
const MIN_ITERATIONS: u32 = 10_000;

/// 当前迭代次数（env `ACS_PBKDF2_ITERATIONS` → 默认 60 万）。
pub fn iterations() -> u32 {
    std::env::var("ACS_PBKDF2_ITERATIONS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map(|n| n.max(MIN_ITERATIONS))
        .unwrap_or(DEFAULT_ITERATIONS)
}
/// 盐长度（字节）。
const SALT_LEN: usize = 16;
/// 派生密钥长度（字节）。
const DK_LEN: usize = 32;
/// 算法标识前缀。
const PREFIX: &str = "pbkdf2-sha256$";
const HMAC_BLOCK: usize = 64;

/// 生成口令哈希（随机盐）。
pub fn hash(password: &str) -> String {
    let iters = iterations();
    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt);
    let dk = pbkdf2_sha256(password.as_bytes(), &salt, iters, DK_LEN);
    format!("{PREFIX}{iters}${}${}", hex::encode(salt), hex::encode(dk))
}

/// 校验口令。支持新格式与旧版单轮 SHA-256 格式；未知格式一律返回 false。
pub fn verify(password: &str, stored: &str) -> bool {
    if let Some(rest) = stored.strip_prefix(PREFIX) {
        // <iters>$<salt hex>$<dk hex>
        let mut it = rest.split('$');
        let (Some(iters), Some(salt_hex), Some(dk_hex)) = (it.next(), it.next(), it.next()) else {
            return false;
        };
        let (Ok(iters), Ok(salt)) = (iters.parse::<u32>(), hex::decode(salt_hex)) else {
            return false;
        };
        if iters == 0 || salt.is_empty() || dk_hex.is_empty() {
            return false;
        }
        let dk = pbkdf2_sha256(password.as_bytes(), &salt, iters, dk_hex.len() / 2);
        return ct_eq(&hex::encode(dk), dk_hex);
    }
    // 旧格式：<salt>$sha256(salt:password)
    if let Some((salt, hash_hex)) = stored.split_once('$') {
        return ct_eq(&legacy_sha256(password, salt), hash_hex);
    }
    false
}

/// 是否需要重新哈希（旧格式或迭代次数低于当前标准）→ 登录成功时自动升级。
pub fn needs_rehash(stored: &str) -> bool {
    match stored.strip_prefix(PREFIX).and_then(|r| r.split('$').next()) {
        Some(iters) => iters
            .parse::<u32>()
            .map(|n| n < iterations())
            .unwrap_or(true),
        None => true,
    }
}

/// 旧格式（仅用于兼容校验）：`sha256("salt:password")` 十六进制。
fn legacy_sha256(password: &str, salt: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(password.as_bytes());
    hex::encode(h.finalize())
}

/// 常量时间比较（等长逐字节异或，不因首个差异提前返回）。
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// 取 `n` 字节随机盐（用 uuid v4 的 CSPRNG 字节流填充，避免额外依赖；盐只要求唯一，不要求保密）。
fn fill_random(buf: &mut [u8]) {
    let mut filled = 0;
    while filled < buf.len() {
        let u = uuid::Uuid::new_v4();
        let b = u.as_bytes();
        let n = (buf.len() - filled).min(b.len());
        buf[filled..filled + n].copy_from_slice(&b[..n]);
        filled += n;
    }
}

/// HMAC-SHA256（RFC 2104）。
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; HMAC_BLOCK];
    if key.len() > HMAC_BLOCK {
        let mut h = Sha256::new();
        h.update(key);
        let d = h.finalize();
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; HMAC_BLOCK];
    let mut opad = [0x5cu8; HMAC_BLOCK];
    for i in 0..HMAC_BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner_d = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_d);
    let out = outer.finalize();
    let mut r = [0u8; 32];
    r.copy_from_slice(&out);
    r
}

/// PBKDF2-HMAC-SHA256（RFC 2898 §5.2）。
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len + 32);
    let mut block: u32 = 1;
    while out.len() < out_len {
        let mut msg = Vec::with_capacity(salt.len() + 4);
        msg.extend_from_slice(salt);
        msg.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac_sha256(password, &msg);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for i in 0..32 {
                t[i] ^= u[i];
            }
        }
        out.extend_from_slice(&t);
        block += 1;
    }
    out.truncate(out_len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_timing() {
        let t0 = std::time::Instant::now();
        let h = hash("Correct-Horse-Battery-9");
        let elapsed = t0.elapsed();
        // 注：debug 构建下 PBKDF2 会比 release 慢约 20 倍，故这里只记录不强制断言。
        println!(
            "PBKDF2 {} 轮耗时: {:?}（当前构建模式）",
            iterations(),
            elapsed
        );
        assert!(verify("Correct-Horse-Battery-9", &h));
        assert!(!verify("wrong-password", &h));
        assert!(!needs_rehash(&h));
        // 盐随机：同一口令两次哈希不同
        assert_ne!(h, hash("Correct-Horse-Battery-9"));
    }

    #[test]
    fn legacy_format_still_verifies_and_upgrades() {
        // 旧格式：<salt>$sha256(salt:password)
        let salt = "d3f1a0c2-legacy";
        let legacy = format!("{salt}${}", legacy_sha256("old-passphrase", salt));
        assert!(verify("old-passphrase", &legacy));
        assert!(!verify("nope", &legacy));
        assert!(needs_rehash(&legacy), "旧格式应触发重哈希升级");
    }

    #[test]
    fn malformed_inputs_are_rejected() {
        for bad in ["", "$", "pbkdf2-sha256$", "pbkdf2-sha256$x$y$z", "abc$def$ghi"] {
            assert!(!verify("whatever", bad), "畸形哈希应校验失败: {bad}");
        }
        assert!(needs_rehash("pbkdf2-sha256$")); // 无法解析 → 需要重哈希
    }
}
