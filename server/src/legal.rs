//! 法律文本（使用协议 / 隐私政策）：内容实现位于 acs-core::legal（Server 与 Client 共享）。
//! - 个人用户（Individual）使用协议 —— 简称《个人使用协议》
//! - 企业 / 成员实体（Company / Country / 系统）使用协议 —— 简称《企业使用协议》
//! - 隐私政策（通用）
//! 客户端注册 / 登录前须同意（个人与企业的使用协议不同），未同意服务端拒绝。

pub use acs_core::legal::doc_html;
