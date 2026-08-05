//! CurseForge 模型（B1）：对应源文件 Models/Expansion/CurseForge/*.cs
//! - AuthorMeta.cs, CategoryMeta.cs, CurseForgeBatchFileInfo.cs
//! - CurseForgeDependenciesMeta.cs, CurseForgeDependenciesType.cs, CurseForgeFileInfo.cs
//! - CurseForgeFilePageResponse.cs, CurseForgeFilesMeta.cs, CurseForgeInfo.cs
//! - CurseForgeLogo.cs, CurseForgeSearchResponse.cs, CurseForgeSearchResult.cs
//! - FingerprintsFilesMeta.cs, FingerprintsRequest.cs, ModLoaderType.cs
//! - ScreenshotsMeta.cs, SortField.cs
//! 序列化规则：CurseForgeJsonContext 使用 UseStringEnumConverter=true（字符串枚举）
//! + CamelCase + WhenWritingNull → 本模块枚举均为字符串序列化，JSON 值即变体名（无 EnumMember）；
//! Option 字段 skip_serializing_if；DateTime 字段按 B1 决策用 String 保存原始文本（未引入 chrono）。

use serde::{Deserialize, Serialize};

/// 作者元信息（源：AuthorMeta.cs 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthorMeta {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// 分类元信息（源：CategoryMeta.cs 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CategoryMeta {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// 批量 GetFiles API 返回的单条文件信息（精简版，含 downloadUrl 和 Sha1）。
/// （源：CurseForgeBatchFileInfo.cs）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeBatchFileInfo {
    pub id: i64,
    pub mod_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
}

/// 依赖元信息（源：CurseForgeDependenciesMeta.cs 记录；JSON 键为 modId / relationType）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeDependenciesMeta {
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(rename = "relationType")]
    pub r#type: i32,
}

/// 依赖类型（源：CurseForgeDependenciesType.cs 枚举，字符串序列化，JSON 值即变体名）。
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Copy)]
pub enum CurseForgeDependenciesType {
    /// JSON 值 "EmbeddedLibrary"（=1）
    EmbeddedLibrary,
    /// JSON 值 "OptionalDependency"（=2）
    OptionalDependency,
    /// JSON 值 "RequiredDependency"（=3）
    RequiredDependency,
    /// JSON 值 "Tool"（=4）
    Tool,
    /// JSON 值 "Incompatible"（=5）
    Incompatible,
    /// JSON 值 "Include"（=6）
    Include,
}

/// 文件信息（源：CurseForgeFileInfo.cs 记录；FileId/ModId 为字符串，JSON 键为 id / modId）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFileInfo {
    #[serde(rename = "id")]
    pub file_id: String,
    #[serde(rename = "modId")]
    pub mod_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    pub release_type: i32,
    pub file_status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<CurseForgeDependenciesMeta>>,
}

/// 文件分页响应（源：CurseForgeFilePageResponse.cs 的 CurseForgeFilePageResponse 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFilePageResponse {
    pub files: Vec<CurseForgeFilePageItem>,
    pub total_count: i32,
}

/// 文件分页条目（源：CurseForgeFilePageResponse.cs 的 CurseForgeFilePageItem 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFilePageItem {
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    /// 源字段为 `DateTime`，按 B1 决策用 String 保存原始文本（未引入 chrono）。
    pub file_date: String,
    pub file_length: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_versions: Option<Vec<String>>,
    pub mod_loader: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sortable_game_versions: Option<Vec<CurseForgeSortableGameVersion>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<CurseForgeDependenciesMeta>>,
}

/// 可排序游戏版本（源：CurseForgeFilePageResponse.cs 的 CurseForgeSortableGameVersion 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeSortableGameVersion {
    pub game_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_version_padded: Option<String>,
}

/// 最新文件索引（源：CurseForgeFilesMeta.cs 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFilesMeta {
    pub file_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    pub release_type: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_version: Option<String>,
    pub mod_loader: i32,
}

/// 模组详情（源：CurseForgeInfo.cs 记录；Files 字段 JSON 键为 latestFilesIndexes）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeInfo {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: i32,
    pub download_count: i32,
    pub is_featured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<CategoryMeta>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<AuthorMeta>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshots: Option<Vec<ScreenshotsMeta>>,
    #[serde(rename = "latestFilesIndexes")]
    pub files: Option<Vec<CurseForgeFilesMeta>>,
}

/// 模组 Logo（源：CurseForgeLogo.cs 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeLogo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}

/// 搜索响应（源：CurseForgeSearchResponse.cs 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeSearchResponse {
    pub results: Vec<CurseForgeSearchResult>,
    pub total_count: i32,
}

/// 搜索结果（源：CurseForgeSearchResult.cs 类；字符串字段默认空串，列表默认空）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeSearchResult {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub summary: String,
    pub status: String,
    pub game_version: String,
    pub download_count: String,
    pub is_featured: bool,
    pub icon_url: String,
    pub categories: Vec<CategoryMeta>,
    pub authors: Vec<AuthorMeta>,
    pub screenshots: Vec<ScreenshotsMeta>,
}

/// 指纹反查文件（源：FingerprintsFilesMeta.cs 记录；JSON 键为 modId / id）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintsFilesMeta {
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(rename = "id")]
    pub file_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<CurseForgeDependenciesMeta>>,
}

/// 指纹反查请求（源：FingerprintsRequest.cs 密封记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintsRequest {
    pub fingerprints: Vec<i64>,
}

/// CurseForge ModLoaderType 字符串常量类（源：ModLoaderType.cs 静态类，非枚举）。
/// JSON 值即字符串本身；All 为除 Any 外的全部加载器。
pub mod mod_loader_type {
    pub const ANY: &str = "Any";
    pub const FORGE: &str = "Forge";
    pub const LITE_LOADER: &str = "LiteLoader";
    pub const FABRIC: &str = "Fabric";
    pub const QUILT: &str = "Quilt";
    pub const NEO_FORGE: &str = "NeoForge";

    pub const ALL: [&str; 5] = [FORGE, LITE_LOADER, FABRIC, QUILT, NEO_FORGE];
}

/// 截图元信息（源：ScreenshotsMeta.cs 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotsMeta {
    pub id: i32,
    pub mod_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// 排序字段（源：SortField.cs 枚举，字符串序列化，JSON 值即变体名）。
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Copy)]
pub enum SortField {
    /// JSON 值 "Featured"（=1）
    Featured,
    /// JSON 值 "Popularity"（=2）
    Popularity,
    /// JSON 值 "LastUpdated"（=3）
    LastUpdated,
    /// JSON 值 "Name"（=4）
    Name,
    /// JSON 值 "Author"（=5）
    Author,
    /// JSON 值 "TotalDownloads"（=6）
    TotalDownloads,
    /// JSON 值 "Category"（=7）
    Category,
    /// JSON 值 "GameVersion"（=8）
    GameVersion,
    /// JSON 值 "EarlyAccess"（=9）
    EarlyAccess,
    /// JSON 值 "FeaturedReleased"（=10）
    FeaturedReleased,
    /// JSON 值 "ReleasedDate"（=11）
    ReleasedDate,
    /// JSON 值 "Rating"（=12）
    Rating,
}
