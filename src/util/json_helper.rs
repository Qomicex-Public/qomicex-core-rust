//! JSON 工具（B2）
//!
//! 对应源：Utils/JsonHelper.cs（System.Text.Json SourceGen 序列化入口）
//! + Utils/MinecraftDateTimeConverter.cs（Fabric 版本 JSON 时间的自定义转换器）。
//!
//! 语义要点：
//! - 源的 CombinedJsonContext 使用 CamelCase 策略 + WhenWritingNull 忽略；
//!   Rust 模型已通过 #[serde(rename_all = "camelCase")] /
//!   #[serde(skip_serializing_if = "Option::is_none")] 固化（见 MAPPING_TABLE B1 规则）
//! - 反序列化返回 Option：源对 null JSON 返回 null，serde_json 的 Option<T> 语义等价
//! - MinecraftDateTimeConverter：B1 已定案时间字段用 String 保真（chrono 决策推迟 B6），
//!   故本模块实现为 String ⇄ String 的 parse/format 函数（不经 chrono）

use crate::models::version_manifest::VersionManifestRoot;
use crate::models::version_metadata::CompleteVersionMetadata;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// 序列化版本清单根对象（对应源 Serialize(VersionManifestRoot)）
pub fn serialize_version_manifest(obj: &VersionManifestRoot) -> Result<String, serde_json::Error> {
    serde_json::to_string(obj)
}

/// 反序列化版本清单根对象（对应源 DeserializeVersionManifest；
/// JSON 为 null 时返回 None，同源返回 null）
pub fn deserialize_version_manifest(
    json: &str,
) -> Result<Option<VersionManifestRoot>, serde_json::Error> {
    serde_json::from_str(json)
}

/// 序列化版本元数据对象（对应源 Serialize(CompleteVersionMetadata)）
pub fn serialize_version_metadata(obj: &CompleteVersionMetadata) -> Result<String, serde_json::Error> {
    serde_json::to_string(obj)
}

/// 反序列化版本元数据对象（对应源 DeserializeVersionMetadata；
/// JSON 为 null 时返回 None，同源返回 null）
pub fn deserialize_version_metadata(
    json: &str,
) -> Result<Option<CompleteVersionMetadata>, serde_json::Error> {
    serde_json::from_str(json)
}

/// JsonNode → T 扩展方法（对应源 `ToObject<T>(this JsonNode, JsonTypeInfo<T>)`；
/// 源实现是节点转 JSON 串再反序列化，serde_json::from_value 数据等价）
pub fn to_object<T: DeserializeOwned>(node: &Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(node.clone())
}

/// 解析后的 Minecraft 时间（对应源 DateTimeOffset 的组成成分；
/// 不经 chrono 的轻量表示，时区以分钟偏移记录）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinecraftDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 时区偏移（分钟，向东为正；UTC 为 0）
    pub offset_minutes: i32,
}

/// 对应源 MinecraftDateTimeConverter.Read（Fabric 神秘 Time 的解析）：
/// - null/空字符串 → 错误 "Invalid datetime"（源 JsonException）
/// - 末尾 4 位数字时区偏移（如 "+0800"）自动补冒号（"yyyy-MM-ddTHH:mm:ss+0800" →
///   "+08:00"）后重试
/// - 解析失败 → 错误 "Cannot parse datetime: {raw}"（raw 为修正后的字符串，同源）
///
/// ⚠️ UNMAPPED：源 DateTimeOffset.TryParse(InvariantCulture) 接受 .NET 全格式矩阵
/// （含月份英文名、无秒时间等）。本实现覆盖 Minecraft 实际数据形态
/// （RFC3339/ISO-8601 子集），范围外输入报错而非源可接受的宽容解析。
/// ⚠️ UNMAPPED：源按真实日历校验（如闰年、2 月 30 日拒绝），本实现仅做基本范围校验。
pub fn parse_minecraft_datetime(raw: &str) -> Result<MinecraftDateTime, String> {
    if raw.is_empty() {
        return Err("Invalid datetime".to_string());
    }

    // 非 ASCII 输入（无效时间）直接报错，同源 TryParse 失败抛 JsonException 的语义；
    // 同时避免多字节字符下的字节索引越界 panic
    if !raw.is_ascii() {
        return Err(format!("Cannot parse datetime: {raw}"));
    }

    // 源 Read 的修正分支：raw.Length >= 6 且 raw[raw.Length - 5] 为 +/- 时，
    // 在偏移时与分之间插入 ':'（"…+0800" → "…+08:00"）
    let mut s = raw.to_string();
    if s.len() >= 6 {
        let idx = s.len() - 5;
        let c = s.as_bytes()[idx];
        if c == b'+' || c == b'-' {
            s.insert(idx + 3, ':');
        }
    }

    let bytes = s.as_bytes();
    let mut pos = 0usize;
    let err = || format!("Cannot parse datetime: {s}");

    // 日期：yyyy-MM-dd
    let year = parse_digits(&s, &mut pos, 4).ok_or_else(err)?;
    if !consume(&s, &mut pos, b'-') {
        return Err(err());
    }
    let month = parse_digits(&s, &mut pos, 2).ok_or_else(err)?;
    if !consume(&s, &mut pos, b'-') {
        return Err(err());
    }
    let day = parse_digits(&s, &mut pos, 2).ok_or_else(err)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(err());
    }

    // 时间：T 或空格分隔（源 TryParse 两者都接受）；纯日期（无时间）按 00:00:00
    let (hour, minute, second) = if pos >= bytes.len() {
        (0, 0, 0)
    } else {
        let sep = bytes[pos];
        if sep != b'T' && sep != b' ' {
            return Err(err());
        }
        pos += 1;
        let hour = parse_digits(&s, &mut pos, 2).ok_or_else(err)?;
        if !consume(&s, &mut pos, b':') {
            return Err(err());
        }
        let minute = parse_digits(&s, &mut pos, 2).ok_or_else(err)?;
        // 秒可选（源 TryParse 接受 "HH:mm"），缺省 00
        let second = if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
            parse_digits(&s, &mut pos, 2).ok_or_else(err)?
        } else {
            0
        };
        // 小数秒（源接受且序列化时截断，"ss" 格式不输出小数）
        if pos < bytes.len() && bytes[pos] == b'.' {
            pos += 1;
            let start = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            if pos == start {
                return Err(err());
            }
        }
        if hour > 23 || minute > 59 || second > 59 {
            return Err(err());
        }
        (hour, minute, second)
    };

    // 时区偏移：缺省 0（源 TryParse 无偏移按本地…本实现按 +00:00，见日志 ⚠️ UNMAPPED）
    let offset_minutes = if pos >= bytes.len() {
        0
    } else if bytes[pos] == b'Z' {
        pos += 1;
        0
    } else if bytes[pos] == b'+' || bytes[pos] == b'-' {
        let negative = bytes[pos] == b'-';
        pos += 1;
        // 时 1-2 位（源 TryParse 接受 "+8" / "+08"）
        let start = pos;
        while pos < bytes.len() && pos - start < 2 && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos == start {
            return Err(err());
        }
        let off_h = parse_int(&s[start..pos]).ok_or_else(err)?;
        // 分：可选 ":" + 2 位（源修正分支已把 "HHMM" 补成 "HH:MM"）
        let off_min = if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
            parse_digits(&s, &mut pos, 2).ok_or_else(err)?
        } else {
            0
        };
        // 偏移范围校验（.NET 限 ±14 小时）
        if off_h > 14 || (off_h == 14 && off_min > 0) || off_min > 59 {
            return Err(err());
        }
        let minutes = off_h * 60 + off_min;
        if negative { -minutes } else { minutes }
    } else {
        return Err(err());
    };

    if pos != bytes.len() {
        return Err(err());
    }

    Ok(MinecraftDateTime {
        year,
        month: month as u8,
        day: day as u8,
        hour: hour as u8,
        minute: minute as u8,
        second: second as u8,
        offset_minutes,
    })
}

/// 对应源 MinecraftDateTimeConverter.Write：`yyyy-MM-dd'T'HH:mm:ssK`。
/// 注意：.NET DateTimeOffset 的 "K" **恒**输出 ±HH:MM 偏移（零偏移输出 "+00:00"，
/// 只有 DateTime(Utc Kind) 才输出 "Z"）——本实现与 DateTimeOffset 语义一致
pub fn format_minecraft_datetime(value: &MinecraftDateTime) -> String {
    let sign = if value.offset_minutes < 0 { '-' } else { '+' };
    let abs = value.offset_minutes.abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        value.year,
        value.month,
        value.day,
        value.hour,
        value.minute,
        value.second,
        sign,
        abs / 60,
        abs % 60
    )
}

/// 读取恰好 `count` 位 ASCII 数字
fn parse_digits(s: &str, pos: &mut usize, count: usize) -> Option<i32> {
    let end = *pos + count;
    if end > s.len() {
        return None;
    }
    let slice = &s[*pos..end];
    if !slice.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    *pos = end;
    parse_int(slice)
}

/// 期望下一个字节为给定字符，消费之
fn consume(s: &str, pos: &mut usize, expected: u8) -> bool {
    if *pos < s.len() && s.as_bytes()[*pos] == expected {
        *pos += 1;
        true
    } else {
        false
    }
}

/// 解析纯数字子串（仅 ASCII 数字，非空）
fn parse_int(s: &str) -> Option<i32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
