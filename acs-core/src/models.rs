//! A€（Alpha Coin）核心数据模型。
//!
//! UID 为唯一账户识别符（无昵称/简写）。账户类型分表存储（`AccountType::table_name()`）。
//! 交易为统一总账；除 Mint（铸造，自动确认）外，Transfer/Issue/Redeem 均需双方确认
//! （见 `TxConfirmation`）。

use chrono::{DateTime, Utc};
use std::fmt;

/// 账户类型（决定存储于哪张表，以及 gpg User ID 的后缀）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AccountType {
    Country,
    Company, // 企业账户（银行/企业机构，原 Bank）
    Individual,
    System,
}

impl AccountType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountType::Country => "Country",
            AccountType::Company => "Company",
            AccountType::Individual => "Individual",
            AccountType::System => "System",
        }
    }

    /// 中心库中该类型账户对应的表名（分表存储）。
    pub fn table_name(&self) -> &'static str {
        match self {
            AccountType::Country => "accounts_country",
            AccountType::Company => "accounts_company",
            AccountType::Individual => "accounts_individual",
            AccountType::System => "accounts_system",
        }
    }

    pub fn from_str(s: &str) -> Option<AccountType> {
        match s {
            "Country" => Some(AccountType::Country),
            "Company" => Some(AccountType::Company),
            // 兼容旧客户端/旧数据中的 "Bank"（重命名前的企业账户）
            "Bank" => Some(AccountType::Company),
            "Individual" => Some(AccountType::Individual),
            "System" => Some(AccountType::System),
            _ => None,
        }
    }
}

impl fmt::Display for AccountType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 交易类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TransactionType {
    Transfer, // 转账：账户间 A€ 结算
    Mint,     // 铸造：根管理员铸造，只可打入 PreIssuedAccount（自动确认）
    Issue,    // 发行：PreIssuedAccount → 玩家（商品篮子兑换 A€，需确认）
    Redeem,   // 赎回：玩家 → PreIssuedAccount（A€ 兑换篮子，需确认）
}

impl TransactionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionType::Transfer => "Transfer",
            TransactionType::Mint => "Mint",
            TransactionType::Issue => "Issue",
            TransactionType::Redeem => "Redeem",
        }
    }

    pub fn from_str(s: &str) -> Option<TransactionType> {
        match s {
            "Transfer" => Some(TransactionType::Transfer),
            "Mint" => Some(TransactionType::Mint),
            "Issue" => Some(TransactionType::Issue),
            "Redeem" => Some(TransactionType::Redeem),
            _ => None,
        }
    }
}

impl fmt::Display for TransactionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 账户状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AccountStatus {
    Active,
    Frozen,
    Closed,
    Deleted, // 已注销：账户与账本只读，不可再登录（保留审计）
}

impl AccountStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountStatus::Active => "Active",
            AccountStatus::Frozen => "Frozen",
            AccountStatus::Closed => "Closed",
            AccountStatus::Deleted => "Deleted",
        }
    }

    pub fn from_str(s: &str) -> Option<AccountStatus> {
        match s {
            "Active" => Some(AccountStatus::Active),
            "Frozen" => Some(AccountStatus::Frozen),
            "Closed" => Some(AccountStatus::Closed),
            "Deleted" => Some(AccountStatus::Deleted),
            _ => None,
        }
    }
}

impl fmt::Display for AccountStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 交易状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Rejected,
}

impl TransactionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionStatus::Pending => "Pending",
            TransactionStatus::Confirmed => "Confirmed",
            TransactionStatus::Rejected => "Rejected",
        }
    }

    pub fn from_str(s: &str) -> Option<TransactionStatus> {
        match s {
            "Pending" => Some(TransactionStatus::Pending),
            "Confirmed" => Some(TransactionStatus::Confirmed),
            "Rejected" => Some(TransactionStatus::Rejected),
            _ => None,
        }
    }
}

impl fmt::Display for TransactionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 管理员角色（两级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminRole {
    Root,   // 根管理员（理事长/副理事长，同级同权）
    Finance, // 金融部
}

impl AdminRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AdminRole::Root => "root",
            AdminRole::Finance => "finance",
        }
    }

    pub fn from_str(s: &str) -> Option<AdminRole> {
        match s.to_lowercase().as_str() {
            "root" => Some(AdminRole::Root),
            "finance" => Some(AdminRole::Finance),
            _ => None,
        }
    }
}

impl fmt::Display for AdminRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 账本账户（中心 accounts_* 表）。
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub uid: String,              // 唯一识别符：游戏ID / 国家 / 银行 / 系统账户名
    pub account_type: AccountType,
    pub email: String,            // 同时也是 gpg 密钥的 email
    pub pubkey: String,           // gpg --export 公钥（armored）
    pub encrypted_seckey: String, // gpg 导出、密码上锁的私钥（armored）
    pub balance: i64,             // 仅参考，真实余额以中心账本为准
    pub status: AccountStatus,
    pub last_tx_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub changed_at: DateTime<Utc>,
}

impl Account {
    /// gpg User ID：`"{UID}-{Type}"`。
    pub fn gpg_uid(&self) -> String {
        format!("{}-{}", self.uid, self.account_type.as_str())
    }

    /// 完整的 gpg User ID（含 email）。
    pub fn gpg_user_id(&self) -> String {
        format!("{} <{}>", self.gpg_uid(), self.email)
    }
}

/// 交易记录（中心统一交易总账）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transaction {
    pub tx_id: String,            // UUID v4
    pub tx_type: TransactionType,
    pub sender: String,
    pub sender_type: AccountType,
    pub receiver: String,
    pub receiver_type: AccountType,
    pub amount: i64,
    pub timestamp: i64,
    pub tx_hash: String,             // sha256(规范序列化)
    pub sender_sig: String,          // 发送方 detached 签名（armored）
    pub central_sig: Option<String>, // 铸造=根管理员密钥签名；其余=确认后结算签名
    pub sender_last_hash: Option<String>,
    pub receiver_last_hash: Option<String>,
    pub status: TransactionStatus,
}

impl Transaction {
    pub fn new(
        tx_type: TransactionType,
        sender: String,
        sender_type: AccountType,
        receiver: String,
        receiver_type: AccountType,
        amount: i64,
    ) -> Transaction {
        Transaction {
            tx_id: uuid::Uuid::new_v4().to_string(),
            tx_type,
            sender,
            sender_type,
            receiver,
            receiver_type,
            amount,
            timestamp: Utc::now().timestamp(),
            tx_hash: String::new(),
            sender_sig: String::new(),
            central_sig: None,
            sender_last_hash: None,
            receiver_last_hash: None,
            status: TransactionStatus::Pending,
        }
    }
}

/// AEU 成员国家（client 注册下拉，中心维护）。
#[derive(Debug, Clone, PartialEq)]
pub struct MemberCountry {
    pub id: i64,
    pub name: String,
    pub status: String,
}

/// AEU 成员公司（client 注册下拉，金融部维护）。
#[derive(Debug, Clone, PartialEq)]
pub struct MemberCompany {
    pub id: i64,
    pub name: String,
    pub status: String,
}

/// 后台管理员账户（两级：root / finance）。
/// 所有管理员创建时自动生成 gpg 密钥对并存入本表。
#[derive(Debug, Clone, PartialEq)]
pub struct AdminAccount {
    pub id: i64,
    pub uid: String,
    pub role: AdminRole,
    pub password_hash: String,   // 仅哈希存储
    pub must_change_password: bool, // 首次登录强制改密
    pub pubkey: String,
    pub encrypted_seckey: String, // 根管理员密钥（AES 上锁），铸造签名用
    pub fingerprint: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// 交易确认记录（双方确认机制；Mint 除外）。
#[derive(Debug, Clone, PartialEq)]
pub struct TxConfirmation {
    pub tx_id: String,
    pub confirmed: bool,
    pub reject_reason: Option<String>,
    pub confirmed_at: Option<i64>,
}

/// 邮箱验证码。
#[derive(Debug, Clone, PartialEq)]
pub struct EmailCode {
    pub email: String,
    pub code_hash: String,
    pub purpose: String,
    pub expires_at: i64,
    pub attempts: i32,
    pub verified: bool,
}

/// 镜像 apikey。
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorKey {
    pub apikey: String,
    pub name: String,
    pub status: String,
    pub last_pull_at: Option<i64>,
}

/// 审计日志条目。
#[derive(Debug, Clone, PartialEq)]
pub struct AuditLogEntry {
    pub id: i64,
    pub actor: String,
    pub op: String,
    pub detail: String,
    pub ts: i64,
}

/// 商品篮子储备条目（Issue/Redeem 双向兑换记录）。
#[derive(Debug, Clone, PartialEq)]
pub struct ReserveEntry {
    pub id: i64,
    pub item: String,
    pub qty: f64,
    pub holder: String,
    pub status: String,
    pub ts: i64,
}

/// gpg 生成密钥的产出。
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedKey {
    pub fingerprint: String,
    pub pubkey: String,
    pub encrypted_seckey: String,
}
