//! 安装器模型（B1）：对应源文件
//! - Public/Models/ModLoaderResult.cs：ModLoaderResult 记录 + ModLoaderType 枚举
//! - Public/Models/MissFileInfo.cs：MissFileInfo 记录
//! - Models/OptiFineVersionInfo.cs：OptiFineVersionInfo 类
//! 源上下文无 UseStringEnumConverter → 枚举用 serde_repr 数字序列化。

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// 模组加载器类型（源：ModLoaderResult.cs 的 ModLoaderType 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum ModLoaderType {
    All = 0,
    Forge = 1,
    NeoForge = 2,
    Fabric = 3,
    Quilt = 4,
    LiteLoader = 5,
    OptiFine = 6,
    Cleanroom = 7,
    LegacyFabric = 8,
    Babric = 9,
}

/// 模组加载器结果（源：ModLoaderResult.cs 的 ModLoaderResult 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModLoaderResult {
    pub r#type: ModLoaderType,
    pub version: String,
    pub game_version: String,
    pub url: String,
    pub sha1: String,
    pub is_recommand: bool,
    /// 源字段为 `DateTimeOffset`，按 B1 决策用 String 保存原始文本（未引入 chrono）。
    pub release_time: String,
}

/// 缺失文件信息（源：MissFileInfo.cs 的 MissFileInfo 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MissFileInfo {
    pub name: String,
    pub url: String,
    pub sha1: String,
    pub path: String,
}

/// OptiFine 版本信息（源：Models/OptiFineVersionInfo.cs 类，全部字段可空）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OptiFineVersionInfo {
    pub r#type: Option<String>,
    pub patch: Option<String>,
    pub file_name: Option<String>,
    pub mc_version: Option<String>,
}
