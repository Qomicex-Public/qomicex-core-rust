//! Modrinth 模型（B1）：对应源文件
//! - Models/Expansion/Modrinth/ModLoaderType.cs（ModLoaderType 枚举）
//! - Models/Expansion/Modrinth/ModrinthTag.cs（ModrinthTag）
//! - Models/Expansion/Modrinth/ModrinthVersionInfo.cs（ModrinthVersionInfo、ModrinthFile、ModrinthVersionResponse）
//! - Models/Expansion/Modrinth/ProjectInfo.cs（ProjectInfo、GalleryItem）
//! - Models/Expansion/Modrinth/ProjectVersionInfo.cs（ProjectVersionInfo）
//! - Models/Expansion/Modrinth/SearchResult.cs（SearchResult、SearchResultInfo）
//! - Models/Expansion/Modrinth/StaticIndex.cs（Index、ProjectType、SupportType 静态常量类）
//! - Models/Expansion/Modrinth/VersionFilesRequest.cs（VersionFilesRequest）
//! - Models/Expansion/Modrinth/VersionInfo.cs（VersionInfo、VersionFileInfo、FileHashes、DependenciesInfo）
//! 上下文：JsonContext/ModrinthJsonContext.cs（CamelCase + WhenWritingNull + UseStringEnumConverter=true）
//! → 枚举字符串序列化（JSON 值 = C# 变体名原文，如 "neoForge"、"liteLoader"），
//!   ModLoaderType 各变体显式 #[serde(rename)] 对齐；
//!   所有 Option 字段加 skip_serializing_if 对应 WhenWritingNull。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 加载器类型枚举（源：ModLoaderType.cs）
/// 字符串枚举：JSON 值 = C# 变体名原文（全部为小写/驼峰混合），
/// Rust 变体采用 PascalCase 并逐变体显式 rename 对齐。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ModLoaderType {
    #[serde(rename = "minecraft")]
    Minecraft,
    #[serde(rename = "forge")]
    Forge,
    #[serde(rename = "fabric")]
    Fabric,
    #[serde(rename = "quilt")]
    Quilt,
    #[serde(rename = "neoForge")]
    NeoForge,
    #[serde(rename = "rift")]
    Rift,
    #[serde(rename = "liteLoader")]
    LiteLoader,
    #[serde(rename = "modLoader")]
    ModLoader,
    #[serde(rename = "nilloader")]
    Nilloader,
    #[serde(rename = "ornithe")]
    Ornithe,
}

/// Modrinth 标签（源：ModrinthTag）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthTag {
    /// 标签名称
    pub name: String,
    /// 图标 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// 标签描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Modrinth 版本信息（源：ModrinthVersionInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthVersionInfo {
    /// 版本 ID
    pub id: String,
    /// 所属项目 ID
    #[serde(rename = "project_id")]
    pub project_id: String,
    /// 作者用户 ID
    #[serde(rename = "author_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    /// 版本名称
    pub name: String,
    /// 版本号
    #[serde(rename = "version_number")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_number: Option<String>,
    /// 支持的游戏版本列表
    #[serde(rename = "game_versions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_versions: Option<Vec<String>>,
    /// 支持的加载器列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaders: Option<Vec<String>>,
    /// 发布时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "date_published")]
    pub date_published: String,
    /// 文件列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ModrinthFile>>,
}

/// Modrinth 版本文件（源：ModrinthFile）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthFile {
    /// 哈希表（键为哈希算法名）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hashes: Option<HashMap<String, String>>,
    /// 文件名
    pub filename: String,
    /// 下载地址
    pub url: String,
    /// 文件大小（字节）。API 恒返回；C# 源模型缺失（有损），Rust 补上以便
    /// mrpack 导出等场景使用（缺失按 0 处理）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    /// 是否主文件。同上：C# 源模型缺失，Rust 补上（缺失按 false）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
}

/// Modrinth 多版本响应（源：ModrinthVersionResponse，为 Dictionary<string, ModrinthVersionInfo> 子类）
/// JSON 形状为对象映射 { 版本ID: ModrinthVersionInfo }，以类型别名等价映射。
pub type ModrinthVersionResponse = HashMap<String, ModrinthVersionInfo>;

/// Modrinth 项目信息（源：ProjectInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    /// 项目 ID
    pub id: String,
    /// 项目 slug
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// 项目类型（mod / modpack / resourcepack / shader / datapack）
    #[serde(rename = "project_type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_type: Option<String>,
    /// 团队 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// 组织 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// 项目标题
    #[serde(rename = "title")]
    pub name: String,
    /// 项目描述（短）
    pub description: String,
    /// 项目正文（长描述）
    #[serde(rename = "body")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_description: Option<String>,
    /// 发布时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "published")]
    pub publish_at: String,
    /// 更新时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "updated")]
    pub updated_at: String,
    /// 审核时间（源为 DateTime?，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "approved")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    /// 下载总数
    #[serde(rename = "downloads")]
    pub download_count: i32,
    /// 关注数
    #[serde(rename = "followers")]
    pub follow_count: i32,
    /// 分类列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// 附加分类列表
    #[serde(rename = "additional_categories")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_categories: Option<Vec<String>>,
    /// 支持的加载器列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaders: Option<Vec<String>>,
    /// 支持的游戏版本 ID 列表
    #[serde(rename = "game_versions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_version_ids: Option<Vec<String>>,
    /// 版本 ID 列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<String>>,
    /// 图标 URL
    #[serde(rename = "icon_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// 问题跟踪 URL
    #[serde(rename = "issues_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues_url: Option<String>,
    /// 源码 URL
    #[serde(rename = "source_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Wiki URL
    #[serde(rename = "wiki_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wiki_url: Option<String>,
    /// Discord 邀请 URL
    #[serde(rename = "discord_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_url: Option<String>,
    /// 客户端支持（required / optional / unsupported）
    #[serde(rename = "client_side")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_side: Option<String>,
    /// 服务端支持（required / optional / unsupported）
    #[serde(rename = "server_side")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_side: Option<String>,
    /// 画廊条目列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gallery: Option<Vec<GalleryItem>>,
}

/// 项目画廊条目（源：GalleryItem）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GalleryItem {
    /// 图片 URL
    pub url: String,
    /// 是否为特色图片
    pub featured: bool,
    /// 图片标题
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 图片描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 创建时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    pub created: String,
    /// 排序序号
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordering: Option<i32>,
}

/// Modrinth 项目版本信息（源：ProjectVersionInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectVersionInfo {
    /// 版本 ID
    pub id: String,
    /// 所属项目 ID
    #[serde(rename = "project_id")]
    pub project_id: String,
    /// 版本名称
    pub name: String,
    /// 版本号
    #[serde(rename = "version_number")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_number: Option<String>,
    /// 支持的游戏版本 ID 列表
    #[serde(rename = "game_versions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_version_ids: Option<Vec<String>>,
    /// 支持的加载器列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaders: Option<Vec<String>>,
    /// 更新日志
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    /// 发布时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "date_published")]
    pub published_at: String,
    /// 下载总数
    #[serde(rename = "downloads")]
    pub download_count: i32,
    /// 版本类型（release / beta / alpha）
    #[serde(rename = "version_type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_type: Option<String>,
    /// 文件列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<VersionFileInfo>>,
    /// 依赖列表
    #[serde(rename = "dependencies")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies_infos: Option<Vec<DependenciesInfo>>,
}

/// Modrinth 搜索响应（源：SearchResult）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// 命中结果列表
    #[serde(rename = "hits")]
    pub results: Vec<SearchResultInfo>,
    /// 命中总数
    #[serde(rename = "total_hits")]
    pub total_results: i32,
}

/// Modrinth 搜索结果条目（源：SearchResultInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultInfo {
    /// 项目 ID
    #[serde(rename = "project_id")]
    pub id: String,
    /// 项目 slug
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// 项目标题
    #[serde(rename = "title")]
    pub name: String,
    /// 项目描述
    pub description: String,
    /// 项目正文（长描述）
    #[serde(rename = "body")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_description: Option<String>,
    /// 项目类型（mod / modpack / resourcepack / shader / datapack）
    #[serde(rename = "project_type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// 客户端支持（required / optional / unsupported）
    #[serde(rename = "client_side")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_side: Option<String>,
    /// 服务端支持（required / optional / unsupported）
    #[serde(rename = "server_side")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_side: Option<String>,
    /// 下载总数
    #[serde(rename = "downloads")]
    pub download_count: i32,
    /// 关注数
    #[serde(rename = "follows")]
    pub follow_count: i32,
    /// 图标 URL
    #[serde(rename = "icon_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// 创建时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "date_created")]
    pub created_at: String,
    /// 修改时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "date_modified")]
    pub updated_at: String,
    /// 许可证
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// 作者
    pub author: String,
    /// 分类列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// 版本 ID 列表
    #[serde(rename = "versions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_ids: Option<Vec<String>>,
    /// 画廊图片 URL 列表
    #[serde(rename = "gallery")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gallery_urls: Option<Vec<String>>,
}

/// 排序方式常量（源：StaticIndex.cs 内 Index 静态类）
pub mod index {
    /// 按相关度排序
    pub const RELEVANCE: &str = "relevance";
    /// 按下载数排序
    pub const DOWNLOADS: &str = "downloads";
    /// 按关注数排序
    pub const FOLLOWS: &str = "follows";
    /// 按最新排序
    pub const NEWEST: &str = "newest";
    /// 按更新时间排序
    pub const UPDATED: &str = "updated";
}

/// 项目类型常量（源：StaticIndex.cs 内 ProjectType 静态类）
pub mod project_type {
    /// Mod
    pub const MOD: &str = "mod";
    /// 整合包
    pub const MODPACK: &str = "modpack";
    /// 资源包
    pub const RESOURCE_PACK: &str = "resourcepack";
    /// 光影包
    pub const SHADER: &str = "shader";
    /// 数据包
    pub const DATAPACK: &str = "datapack";
}

/// 支持类型常量（源：StaticIndex.cs 内 SupportType 静态类）
pub mod support_type {
    /// 必需
    pub const REQUIRED: &str = "required";
    /// 可选
    pub const OPTIONAL: &str = "optional";
    /// 不支持
    pub const UNSUPPORTED: &str = "unsupported";
}

/// 版本文件哈希反查请求（源：VersionFilesRequest）
/// 无 JsonPropertyName 特性，走 CamelCase 策略 → JSON 键 "hashes" / "algorithm"。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionFilesRequest {
    /// 哈希值列表
    pub hashes: Vec<String>,
    /// 哈希算法，默认 "sha1"（源为 C# 默认参数值，映射为 serde 默认值）
    #[serde(default = "default_algorithm")]
    pub algorithm: String,
}

/// VersionFilesRequest 默认哈希算法（对应 C# 默认参数 `string Algorithm = "sha1"`）
fn default_algorithm() -> String {
    "sha1".to_string()
}

/// 版本文件哈希反查最新版本请求（Modrinth `POST v2/version_files/update`）。
/// 在 VersionFilesRequest 基础上增加 loader / 游戏版本筛选；源 C# 无此端点，
/// 为模组更新检查（批次哈希匹配）新增能力。JSON 键走 CamelCase 策略。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionFilesUpdateRequest {
    /// 哈希值列表
    pub hashes: Vec<String>,
    /// 哈希算法，默认 "sha1"
    #[serde(default = "default_algorithm")]
    pub algorithm: String,
    /// 加载器筛选（如 "forge" / "fabric"），空数组表示不限
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub loaders: Vec<String>,
    /// 游戏版本筛选（如 "1.20.1"），空数组表示不限
    /// 注：Modrinth API 用 snake_case `game_versions`（rename_all=camelCase 会序列化成
    /// `gameVersions` 导致 API 忽略该过滤条件），此处显式指定 JSON 键名。
    #[serde(rename = "game_versions")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub game_versions: Vec<String>,
}

/// Modrinth 版本信息（源：VersionInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// 版本 ID
    pub id: String,
    /// 版本 slug
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// 所属项目 ID
    #[serde(rename = "project_id")]
    pub project_id: String,
    /// 版本标题（API 实际字段为 name → 反序列化 alias 兼容）
    #[serde(rename = "title", alias = "name")]
    pub name: String,
    /// 版本号
    #[serde(rename = "version_number")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_number: Option<String>,
    /// 支持的游戏版本 ID 列表
    #[serde(rename = "game_versions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_version_ids: Option<Vec<String>>,
    /// 支持的加载器列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaders: Option<Vec<String>>,
    /// 更新日志
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    /// 发布时间（源为 DateTime，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "date_published")]
    pub published_at: String,
    /// 更新时间（v2/version 响应无 date_modified/updated 字段 → 可空）
    #[serde(rename = "date_modified")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// 审核时间（源为 DateTime?，暂用原始字符串保真 ⚠️ UNMAPPED）
    #[serde(rename = "approved")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    /// 下载总数
    #[serde(rename = "downloads")]
    pub download_count: i32,
    /// 图标 URL
    #[serde(rename = "icon_url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// 文件列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<VersionFileInfo>>,
    /// 依赖列表
    #[serde(rename = "dependencies")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies_infos: Option<Vec<DependenciesInfo>>,
}

/// 版本文件信息（源：VersionFileInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionFileInfo {
    /// 文件名
    pub filename: String,
    /// 下载地址
    #[serde(rename = "url")]
    pub download_url: String,
    /// 文件大小（字节）
    pub size: i64,
    /// 是否为主文件
    #[serde(rename = "primary")]
    pub is_primary: bool,
    /// 文件类型（source / artifact / binary）
    #[serde(rename = "file_type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    /// 文件哈希
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hashes: Option<FileHashes>,
}

/// 文件哈希（源：FileHashes）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileHashes {
    /// SHA1 哈希
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    /// SHA512 哈希
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha512: Option<String>,
}

/// 依赖信息（源：DependenciesInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DependenciesInfo {
    /// 依赖的版本 ID
    #[serde(rename = "version_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    /// 依赖的项目 ID
    #[serde(rename = "project_id")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// 依赖的文件名
    #[serde(rename = "file_name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// 依赖类型（required / optional / incompatible / embedded）
    #[serde(rename = "dependency_type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_type: Option<String>,
}
