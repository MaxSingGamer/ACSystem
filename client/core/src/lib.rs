//! A€（Alpha Coin）客户端业务核心（v3.3.0 #11 前置）。
//!
//! 原 `acs-client` 里的业务代码（钱包 / 账本 / 网络 / gpg / 更新）抽到这里，
//! 产出 `acs_client_core.dll`：QT 客户端（C++）通过 [`ffi`] 的 **C ABI** 调用（D12），
//! Tauri 客户端（当前临时形态）与 CLI 直接按 Rust crate 依赖。
//!
//! 分工：
//! - **本 crate**：全部业务逻辑（不含任何界面框架；不依赖 tauri）
//! - `acs-client`：Tauri 薄壳（command 层）
//! - 将来的 QT 客户端：C++/QML 薄壳（通过 C ABI 调用本 crate）
//!
//! 边界硬性要求（D12）：C ABI 只传 UTF-8 JSON；返回值由 Rust 分配、调用方 `acs_string_free` 释放；
//! 错误结构化返回、不 panic 穿透；`acs_abi_version()` 供调用方校验 DLL 与 exe 版本匹配。

pub mod client_api;
pub mod ffi;
pub mod sync;
pub mod txn;
pub mod update;
pub mod wallet;
pub mod web_helpers;

/// 产品版本（DLL 与宿主 exe 可能不同步，调用方应同时校验 `acs_abi_version()`）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
