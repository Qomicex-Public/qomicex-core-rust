//! 下载校验辅助（B6）
//!
//! 对应源实现：Utils/FileHelper.cs 的 `ValidateFileHash`（SHA1 文件校验，
//! MAPPING_TABLE utils 指向本模块）。
//!
//! 源语义逐条保留：
//! - 文件不存在或期望哈希为空 → 返回 false；
//! - SHA1 计算（`Convert.ToHexString(hash).ToLower()` → 小写十六进制）；
//! - 与期望值 `StringComparison.OrdinalIgnoreCase` 比较（大小写不敏感）。
//!
//! 差异说明（B6 定案）：C# 文件读取失败抛运行时异常；本模块返回
//! `Err(Error::DownloadFailed)`，由调用方（retry.rs）决定是否重试，语义等价。


use sha1::{Digest, Sha1};


/// 校验文件 SHA1（源：`FileHelper.ValidateFileHash`）。
///
/// - 文件不存在或 `expected_sha1` 为空 → `Ok(false)`（源同，不视为错误）；
/// - 实际哈希取小写十六进制，与期望值大小写不敏感比较；

/// 计算字节数据的 SHA1 小写十六进制（源：`Convert.ToHexString(hash).ToLower()`）。
///
/// 不引入 hex crate（Cargo.toml 依赖不可改），逐字节 `{:02x}` 格式化；
/// 期望哈希均为 ASCII 十六进制，`eq_ignore_ascii_case` 与
/// `StringComparison.OrdinalIgnoreCase` 语义一致。
pub(crate) fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    let mut s = String::with_capacity(40);
    for byte in digest {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}


