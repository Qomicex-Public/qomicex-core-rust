//! MurmurHash2 指纹（CurseForge 反查）（B2）
//!
//! 对应源：Services/Expansion/Local/LocalResourceBase.cs 的
//! `CurseForgeFingerprint(byte[])` 与 `MurmurHash2(byte[], uint seed = 1)`。
//!
//! 特殊兼容语义（详见 p13 翻译日志）：
//! - 端序：源 `BitConverter.ToUInt32(data, i)` 在 x64 默认**小端** →
//!   Rust 用 `u32::from_le_bytes`
//! - 溢出：源 uint 算术为 unchecked 环绕 → Rust `wrapping_mul`
//! - 返回值：源 `long` 由 `uint h` 隐式转换（**无符号零扩展**，非位模式）→
//!   Rust `h as i64`，结果域为 [0, 2^32)
//! - 算法常量保留：m = 0x5bd1e995、r = 24、seed = 1（CurseForge 指纹）

/// MurmurHash2 算法常量 m
const M: u32 = 0x5bd1e995;
/// MurmurHash2 算法常量 r
const R: u32 = 24;

/// CurseForge 文件指纹（对应源 CurseForgeFingerprint）：
/// 先过滤 0x09(TAB)/0x0A(LF)/0x0D(CR)/0x20(空格) 四个字节，再以 seed=1 计算 MurmurHash2
pub fn curse_forge_fingerprint(data: &[u8]) -> i64 {
    let filtered: Vec<u8> = data
        .iter()
        .copied()
        .filter(|&b| b != 0x09 && b != 0x0A && b != 0x0D && b != 0x20)
        .collect();
    murmur_hash2(&filtered, 1)
}

/// MurmurHash2（对应源 MurmurHash2，seed 默认值 1 由调用方显式传入）。
/// 返回 i64，语义为源 uint h 零扩展转 long（结果域 [0, 2^32)，恒非负）
pub fn murmur_hash2(data: &[u8], seed: u32) -> i64 {
    let mut len = data.len() as u32;
    let mut h = seed ^ len;
    let mut i = 0usize;

    while len >= 4 {
        // 源 BitConverter.ToUInt32(data, i)：x64 小端 → u32::from_le_bytes
        let mut k = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M);
        h ^= k;
        i += 4;
        len -= 4;
    }

    // 对应源 switch(len) 的 goto 穿透：case 3 → case 2 → case 1 依次累积异或，
    // 且 h *= m 仅在 len>=1 时执行一次（源 case 1 的语句）。
    // 异或运算可交换，顺序不影响结果，但三个分支是累积而非互斥。
    if len >= 1 {
        h ^= data[i] as u32;
    }
    if len >= 2 {
        h ^= (data[i + 1] as u32) << 8;
    }
    if len >= 3 {
        h ^= (data[i + 2] as u32) << 16;
    }
    if len >= 1 {
        h = h.wrapping_mul(M);
    }

    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    // 源 `return h;`（uint 隐式转 long = 无符号零扩展）
    h as i64
}
