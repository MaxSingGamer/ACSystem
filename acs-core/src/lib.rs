//! A€（Alpha Coin）核心库：供 acs-server / acs-client / acs-mirror 共用。
//!
//! 同时产出 rlib（Rust crate 依赖）与 cdylib（Windows .dll 动态链接）。
//! 信任模型：中心 > 本地 > 镜像；防假币依赖发送方 ed25519 签名 + 中心签名 +
//! 每账户哈希链 + 发行权收归理事会（中心密钥由理事长 AES 密码加密保管）。

pub mod account;
pub mod config;
pub mod db;
pub mod errors;
pub mod ffi;
pub mod gpg;
pub mod gpg_detect;
pub mod models;
pub mod transaction;

pub use errors::{AcsError, Result};
pub use models::{
    Account, AccountStatus, AccountType, AdminAccount, AdminRole, EmailCode, GeneratedKey,
    MemberCompany, MemberCountry, MirrorKey, ReserveEntry, Transaction, TransactionStatus,
    TransactionType, TxConfirmation,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

