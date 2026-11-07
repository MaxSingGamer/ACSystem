//! 管理员口令哈希 —— 已抽取到 [`acs_core::password`]（v4.0.0：面板登录复用同一套 PBKDF2）。
//!
//! 本模块只做转发，保持 server 内既有调用点（`password::hash` / `verify` / ...）不变。

#[allow(unused_imports)]
pub use acs_core::password::{ct_eq, hash, iterations, needs_rehash, verify};
