//! 统一错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcsError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("GPG 错误: {0}")]
    Gpg(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("账户不存在: {0}")]
    AccountNotFound(String),
    #[error("账户已存在: {0}")]
    AccountExists(String),
    #[error("账户未激活（冻结或关闭）")]
    AccountNotActive,
    #[error("余额不足")]
    InsufficientBalance,
    #[error("签名无效")]
    SignatureInvalid,
    #[error("交易哈希链断裂: {0}")]
    HashMismatch(String),
    #[error("验证码无效或过期")]
    InvalidCode,
    #[error("非法参数: {0}")]
    InvalidArgument(String),
    #[error("未授权: {0}")]
    Unauthorized(String),
    #[error("中心密钥未解锁")]
    KeyLocked,
    #[error("{0}")]
    Message(String),
}

impl AcsError {
    pub fn gpg(msg: impl Into<String>) -> Self {
        AcsError::Gpg(msg.into())
    }

    pub fn message(msg: impl Into<String>) -> Self {
        AcsError::Message(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, AcsError>;
