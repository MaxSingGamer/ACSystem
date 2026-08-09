//! cdylib（Windows .dll 动态链接）消费的最小 C ABI 封装。
//!
//! 若 server/client 以 Rust crate 依赖 rlib，则无需走 FFI；
//! 此处仅保证 cdylib 产物可用且提供基础工具函数。

use std::ffi::c_char;
use std::os::raw::c_void;

/// 返回库版本（静态 C 字符串，调用方不得释放）。
#[unsafe(no_mangle)]
pub extern "C" fn acs_core_version() -> *const c_char {
    b"0.1.0\0".as_ptr() as *const c_char
}

/// 计算 sha256 十六进制字符串。
///
/// - `input`: 输入数据指针
/// - `len`: 输入长度
/// - `out`: 输出缓冲（须 >= 65 字节，64 字节 hex + 结尾 NUL）
/// 返回 0 成功，-1 参数非法。
#[unsafe(no_mangle)]
pub extern "C" fn acs_core_sha256_hex(input: *const c_void, len: usize, out: *mut c_char) -> i32 {
    if out.is_null() {
        return -1;
    }
    use sha2::{Digest, Sha256};
    let data = if input.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: 调用方保证 input 指向 len 字节可读内存
        unsafe { std::slice::from_raw_parts(input as *const u8, len) }
    };
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hex::encode(hasher.finalize());
    let bytes = digest.as_bytes();
    // SAFETY: 调用方保证 out 至少 65 字节可写
    let dst = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, 65) };
    dst[..64].copy_from_slice(&bytes[..64]);
    dst[64] = 0;
    0
}
