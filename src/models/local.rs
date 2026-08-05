//! 本地模型（B1）
//! 对应源：Models/Local/LocalVersionInfo.cs + VersionManifestCacheInfo.cs
//! 注意：models_b1 中 Local/ServerEntry 等另在后续批（模型映射表 models 段），此处不包含

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// 表示本地已安装版本的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalVersionInfo {
    /// 版本 ID
    pub id: String,
    /// 该版本包含的 Mod 加载器列表
    pub r#type: Vec<ModloaderInfo>,
    /// 发布时间（源为 DateTimeOffset，暂用原始字符串保真，类型决策见日志 ⚠️ UNMAPPED）
    pub release_time: String,
    /// 资源文件是否完整
    pub is_complete: bool,
    /// 版本目录路径
    pub version_path: String,
    /// 对应的原版版本号
    pub vanilla_version: String,
    /// 版本总大小（字节）
    pub total_size: i64,
}

/// 表示一个 Mod 加载器信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModloaderInfo {
    /// 加载器类型
    pub r#type: ModloaderType,
    /// 加载器版本
    pub version: String,
}

/// Mod 加载器类型
/// 源为 enum，所在 context 无 UseStringEnumConverter → 数字序列化，值按源声明顺序
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum ModloaderType {
    /// 未知
    Unknown = 0,
    /// 原版
    Vanilla = 1,
    /// Forge
    Forge = 2,
    /// NeoForge
    NeoForge = 3,
    /// Cleanroom
    Cleanroom = 4,
    /// Legacy Fabric
    LegacyFabric = 5,
    /// Babric
    Babric = 6,
    /// Fabric
    Fabric = 7,
    /// Quilt
    Quilt = 8,
    /// LiteLoader
    LiteLoader = 9,
    /// OptiFine
    OptiFine = 10,
}

/// 表示版本清单缓存信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionManifestCacheInfo {
    /// 缓存时间（源为 DateTime，暂用原始字符串保真，类型决策见日志 ⚠️ UNMAPPED）
    pub cached_time: String,
    /// 缓存中的版本数量
    pub version_count: i32,
    /// 最新正式版
    pub latest_release: String,
    /// 最新快照版
    pub latest_snapshot: String,
}
