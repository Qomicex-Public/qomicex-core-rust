//! 本地模型（B1）
//! 对应源：Models/Local/LocalVersionInfo.cs + VersionManifestCacheInfo.cs
//! 追加段（p20 批次）：Services/Options/ServiceTypes.cs 的游戏版本/选项/服务器条目类型，见文件尾部
//! 注意：Models/Local 中 Local/ServerEntry 等另在后续批（模型映射表 models 段），此处不包含

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::HashMap;

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

// ================================================================
// p20 批次追加：Services/Options/ServiceTypes.cs（游戏版本/游戏选项/服务器条目）
// 映射依据：MAPPING_TABLE models 段（Local/ServerEntry, ServerState, GameOption -> models::local）
// ================================================================

/// 游戏版本定义
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GameVersion {
    /// 版本号
    pub version: String,
    /// 发布类型（正式版/快照版等）
    pub release_type: String,
    /// 发布日期（源为 DateTime，B1 定案暂用原始字符串保真，chrono 决策推迟到 B6）
    pub release_date: String,
}

impl Default for GameVersion {
    fn default() -> Self {
        Self {
            version: String::new(),
            release_type: String::new(),
            release_date: String::new(),
        }
    }
}

/// 游戏选项（名称-值 键值对）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GameOption {
    /// 选项名称
    pub option_name: String,
    /// 选项值
    pub option_value: String,
}

impl Default for GameOption {
    fn default() -> Self {
        Self {
            option_name: String::new(),
            option_value: String::new(),
        }
    }
}

/// 游戏选项快照
/// 源含默认构造函数（空字典）+ 带参构造函数
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GameOptionsSnapshot {
    /// 选项值字典
    pub values: HashMap<String, String>,
}

impl GameOptionsSnapshot {
    /// 使用给定选项值构造快照
    pub fn new(values: HashMap<String, String>) -> Self {
        Self { values }
    }
}

impl Default for GameOptionsSnapshot {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
        }
    }
}

/// Minecraft 选项定义（JSON 字段名显式小写，不做 rename_all）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MinecraftOption {
    /// 选项名称
    #[serde(rename = "name")]
    pub name: String,
    /// 默认值
    #[serde(rename = "defaultValue")]
    pub default_value: String,
    /// 有效值（原始字符串）
    #[serde(rename = "validValues")]
    pub valid_values: String,
    /// 引入版本（显示文本）
    #[serde(rename = "introducedVersion")]
    pub introduced_version: String,
    /// 引入版本（原始文本）
    #[serde(rename = "introducedVersionRaw")]
    pub introduced_version_raw: String,
    /// 选项分类
    #[serde(rename = "category")]
    pub category: String,
}

impl Default for MinecraftOption {
    fn default() -> Self {
        Self {
            name: String::new(),
            default_value: String::new(),
            valid_values: String::new(),
            introduced_version: String::new(),
            introduced_version_raw: String::new(),
            category: String::new(),
        }
    }
}

/// 选项定义（含当前值）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OptionDefinition {
    /// 选项名称
    pub name: String,
    /// 默认值
    pub default_value: String,
    /// 当前值
    pub current_value: String,
    /// 描述（源默认 "(无描述)"）
    pub description: String,
    /// 有效值（原始字符串）
    pub valid_values_raw: String,
    /// 引入版本
    pub introduced_version: String,
    /// 当前版本是否可用
    pub is_available_in_current_version: bool,
    /// 值类型（源默认 "Text"）
    pub value_kind: String,
}

impl Default for OptionDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            default_value: String::new(),
            current_value: String::new(),
            description: "(无描述)".to_string(),
            valid_values_raw: String::new(),
            introduced_version: String::new(),
            is_available_in_current_version: false,
            value_kind: "Text".to_string(),
        }
    }
}

/// 选项视图条目（与 OptionDefinition 同构，供 UI 展示）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OptionViewItem {
    /// 选项名称
    pub name: String,
    /// 默认值
    pub default_value: String,
    /// 当前值
    pub current_value: String,
    /// 描述（源默认 "(无描述)"）
    pub description: String,
    /// 有效值（原始字符串）
    pub valid_values_raw: String,
    /// 引入版本
    pub introduced_version: String,
    /// 当前版本是否可用
    pub is_available_in_current_version: bool,
    /// 值类型（源默认 "Text"）
    pub value_kind: String,
}

impl Default for OptionViewItem {
    fn default() -> Self {
        Self {
            name: String::new(),
            default_value: String::new(),
            current_value: String::new(),
            description: "(无描述)".to_string(),
            valid_values_raw: String::new(),
            introduced_version: String::new(),
            is_available_in_current_version: false,
            value_kind: "Text".to_string(),
        }
    }
}

/// 服务器条目
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    /// 服务器名称
    pub name: String,
    /// 服务器地址
    pub address: String,
    /// 图标（Base64 字符串）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_base64: Option<String>,
    /// 是否接受资源包
    pub accept_textures: bool,
    /// 是否隐藏
    pub hidden: bool,
}

impl Default for ServerEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            icon_base64: None,
            accept_textures: false,
            hidden: false,
        }
    }
}

/// 服务器状态
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    /// 服务器名称
    pub name: String,
    /// 服务器地址
    pub address: String,
    /// 是否在线
    pub is_online: bool,
    /// 延迟（毫秒，源 long）
    pub ping: i64,
    /// 在线玩家数
    pub online_players: i32,
    /// 最大玩家数
    pub max_players: i32,
    /// 服务器版本
    pub version: String,
    /// 服务器描述
    pub description: String,
    /// 错误消息
    pub error_message: String,
    /// 图标（Base64 字符串）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_base64: Option<String>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            is_online: false,
            ping: 0,
            online_players: 0,
            max_players: 0,
            version: String::new(),
            description: String::new(),
            error_message: String::new(),
            icon_base64: None,
        }
    }
}

/// 局域网服务器条目
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanServerEntry {
    /// 服务器 MOTD
    pub motd: String,
    /// 服务器地址
    pub address: String,
    /// 端口
    pub port: i32,
    /// 展示地址
    pub display_address: String,
}

impl Default for LanServerEntry {
    fn default() -> Self {
        Self {
            motd: String::new(),
            address: String::new(),
            port: 0,
            display_address: String::new(),
        }
    }
}
