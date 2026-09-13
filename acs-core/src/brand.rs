//! 品牌与部署定制：**所有可定制元素集中在这里，统一由环境变量覆盖**。
//!
//! 目的：同一份代码可用于不同联盟 / 不同域名 / 不同品牌的部署，无需改源码。
//! 读取策略：进程内只解析一次（`OnceLock`），未设置或为空时回退到下表的默认值。
//!
//! | 环境变量 | 含义 | 默认值 |
//! |---|---|---|
//! | `ACS_BRAND_NAME` | 品牌名 | `Alpha Coin` |
//! | `ACS_BRAND_CURRENCY` | 货币符号 | `A€` |
//! | `ACS_BRAND_UNION_NAME` | 组织名（英文） | `Alpha Economy Union` |
//! | `ACS_BRAND_UNION_NAME_CN` | 组织名（中文） | `阿尔法经济联盟` |
//! | `ACS_BRAND_UNION_ABBR` | 组织缩写 | `AEU` |
//! | `ACS_BRAND_SYSTEM_NAME` | 系统全称 | `Alpha Coin System` |
//! | `ACS_SERVER_APP_NAME` | 服务端应用名（窗口标题等） | `Alpha Coin Central Server` |
//! | `ACS_CLIENT_APP_NAME` | 客户端应用名 | `Alpha Wallet` |
//! | `ACS_PUBLIC_URL` | 公开域名（客户端默认中心地址） | `https://acsystem.maxshin.top` |
//! | `ACS_ADMIN_URL` | 管理后台域名（展示/文档用） | `https://acsmanage.maxshin.top` |
//! | `ACS_SYSTEM_EMAIL_DOMAIN` | 系统账户邮箱域名 | `maxshin.top` |
//! | `ACS_ADMIN_EMAIL_DOMAIN` | 管理员 gpg 邮箱域名 | `aeu.admin` |
//! | `ACS_MINT_AUTHORITY` | 铸造交易的发出方标识 | `MintAuthority` |
//! | `ACS_UPDATE_REPO` | 自动更新源（`owner/repo`） | `MaxSingGamer/ACSystem` |
//! | `ACS_PBKDF2_ITERATIONS` | 管理员口令哈希迭代次数（见 acs-server/password.rs） | `600000` |

use std::sync::OnceLock;

/// 取环境变量；未设置或全空白则用默认值。
fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    }
}

/// 三级取值：运行时环境变量 → 构建时环境变量（`option_env!`，打包机可烘焙）→ 默认值。
///
/// 说明：构建时值由 Cargo 记录，改变后需重新构建 `acs-core`（`cargo clean -p acs-core`）生效。
macro_rules! env_or_baked {
    ($key:literal, $default:literal) => {{
        let runtime = env_or($key, "");
        if !runtime.is_empty() {
            runtime
        } else {
            option_env!($key).unwrap_or($default).trim().to_string()
        }
    }};
}

/// 品牌与部署配置。
#[derive(Debug, Clone)]
pub struct Brand {
    /// 品牌名，如 `Alpha Coin`。
    pub name: String,
    /// 货币符号，如 `A€`。
    pub currency: String,
    /// 组织名（英文）。
    pub union_name: String,
    /// 组织名（中文）。
    pub union_name_cn: String,
    /// 组织缩写，如 `AEU`。
    pub union_abbr: String,
    /// 系统全称，如 `Alpha Coin System`。
    pub system_name: String,
    /// 服务端应用名。
    pub server_app: String,
    /// 客户端应用名。
    pub client_app: String,
    /// 公开 API 域名（客户端默认中心地址）。
    pub public_url: String,
    /// 管理后台域名。
    pub admin_url: String,
    /// 系统账户邮箱域名（系统账户 gpg uid 形如 `uid <uid@域名>`）。
    pub system_email_domain: String,
    /// 管理员 gpg 邮箱域名。
    pub admin_email_domain: String,
    /// 铸造交易发出方标识（非真实账户）。
    pub mint_authority: String,
    /// 自动更新源（`owner/repo`）。
    pub update_repo: String,
}

impl Brand {
    fn from_env() -> Brand {
        Brand {
            name: env_or_baked!("ACS_BRAND_NAME", "Alpha Coin"),
            currency: env_or_baked!("ACS_BRAND_CURRENCY", "A€"),
            union_name: env_or_baked!("ACS_BRAND_UNION_NAME", "Alpha Economy Union"),
            union_name_cn: env_or_baked!("ACS_BRAND_UNION_NAME_CN", "阿尔法经济联盟"),
            union_abbr: env_or_baked!("ACS_BRAND_UNION_ABBR", "AEU"),
            system_name: env_or_baked!("ACS_BRAND_SYSTEM_NAME", "Alpha Coin System"),
            server_app: env_or_baked!("ACS_SERVER_APP_NAME", "Alpha Coin Central Server"),
            client_app: env_or_baked!("ACS_CLIENT_APP_NAME", "Alpha Wallet"),
            public_url: env_or_baked!("ACS_PUBLIC_URL", "https://acsystem.maxshin.top"),
            admin_url: env_or_baked!("ACS_ADMIN_URL", "https://acsmanage.maxshin.top"),
            system_email_domain: env_or_baked!("ACS_SYSTEM_EMAIL_DOMAIN", "maxshin.top"),
            admin_email_domain: env_or_baked!("ACS_ADMIN_EMAIL_DOMAIN", "aeu.admin"),
            mint_authority: env_or_baked!("ACS_MINT_AUTHORITY", "MintAuthority"),
            update_repo: env_or_baked!("ACS_UPDATE_REPO", "MaxSingGamer/ACSystem"),
        }
    }

    /// 系统账户邮箱：`{uid}@{system_email_domain}`。
    pub fn system_email(&self, uid: &str) -> String {
        format!("{uid}@{}", self.system_email_domain)
    }

    /// 管理员 gpg uid：`{uid} <{uid}@{admin_email_domain}>`。
    pub fn admin_gpg_uid(&self, uid: &str) -> String {
        format!("{uid} <{uid}@{}>", self.admin_email_domain)
    }

    /// 系统账户 gpg uid：`{uid} <{uid}@{system_email_domain}>`。
    pub fn system_gpg_uid(&self, uid: &str) -> String {
        format!("{uid} <{}>", self.system_email(uid))
    }

    /// 把协议 / 隐私等静态文本里的品牌写成当前配置。
    ///
    /// 这样法律文本只需维护一份，换品牌时改 `.env` 即可（**长串优先**，避免部分替换）。
    pub fn substitute(&self, text: &str) -> String {
        text.replace("Alpha Coin System (ACSystem)", &format!("{} (ACSystem)", self.system_name))
            .replace("Alpha Coin Central Server", &self.server_app)
            .replace("Alpha Wallet", &self.client_app)
            .replace("Alpha Economy Union", &self.union_name)
            .replace("Alpha Coin System", &self.system_name)
            .replace("Alpha Coin", &self.name)
            .replace("阿尔法经济联盟", &self.union_name_cn)
            .replace("AEU", &self.union_abbr)
            .replace("A€", &self.currency)
    }
}

/// 全局品牌配置（进程内解析一次）。
pub fn brand() -> &'static Brand {
    static B: OnceLock<Brand> = OnceLock::new();
    B.get_or_init(Brand::from_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_stable() {
        let b = brand();
        assert!(!b.name.is_empty());
        assert!(!b.currency.is_empty());
        assert_eq!(b.system_email("AESystem"), format!("AESystem@{}", b.system_email_domain));
        assert_eq!(b.admin_gpg_uid("root"), format!("root <root@{}>", b.admin_email_domain));
        assert_eq!(b.system_gpg_uid("AESystem"), format!("AESystem <AESystem@{}>", b.system_email_domain));
    }

    #[test]
    fn substitute_uses_custom_brand() {
        // 用自定义品牌验证替换顺序（长串优先，避免"Alpha Coin" 抢走 "Alpha Coin System"）
        let mut b = brand().clone();
        b.name = "Nova Coin".into();
        b.currency = "N¥".into();
        b.union_name = "Nova Union".into();
        b.union_abbr = "NU".into();
        b.system_name = "Nova System".into();
        b.client_app = "Nova Wallet".into();
        let out = b.substitute("Alpha Coin System (ACSystem) / Alpha Coin / Alpha Wallet / AEU / A€");
        assert!(out.contains("Nova System (ACSystem)"), "全文: {out}");
        assert!(out.contains("Nova Coin"), "全文: {out}");
        assert!(out.contains("Nova Wallet"), "全文: {out}");
        assert!(out.contains("NU"), "全文: {out}");
        assert!(out.contains("N¥"), "全文: {out}");
        assert!(!out.contains("Alpha"), "不应残留默认品牌: {out}");
    }
}
