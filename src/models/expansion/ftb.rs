//! FTB 模型（B1）：对应源文件
//! - Models/Expansion/FeedTheBeast/FTBSearchResponse.cs（FTBSearchResponse）
//! - Models/Expansion/FeedTheBeast/ModpackInfo.cs（ModpackInfo、TagInfo、VersionInfo、SpecsInfo、TargetInfo、AuthorInfo、LinkInfo、ArtInfo、MetaInfo、RatingInfo）
//! - Models/Expansion/FeedTheBeast/VersionDetail.cs（VersionDetail、ModsDetail、FtbModInfo、FtbFileInfo、ChangelogResult、CacheData）
//! - JsonContext/FTBJsonContext.cs（CamelCase + WhenWritingNull + UseStringEnumConverter=true；本模块无枚举类型）

use serde::{Deserialize, Serialize};

/// FTB 搜索响应（源：FTBSearchResponse）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FTBSearchResponse {
    pub results: Vec<ModpackInfo>,
    pub total_count: i32,
}

/// FTB 整合包信息（源：ModpackInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModpackInfo {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synopsis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub featured: Option<bool>,
    pub plays: i64,
    pub installs: i64,
    #[serde(rename = "plays_14d")]
    pub plays_14d: i64,
    pub updated: i64,
    pub released: i64,
    pub private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<VersionInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<AuthorInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<LinkInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art: Option<Vec<ArtInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<MetaInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<RatingInfo>,
}

/// FTB 标签信息（源：TagInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub id: i32,
    pub name: String,
}

/// FTB 版本摘要信息（源：VersionInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: i32,
    pub name: String,
    pub r#type: String,
    pub updated: i64,
    pub released: i64,
    pub private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specs: Option<SpecsInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<TargetInfo>>,
}

/// FTB 版本规格信息（源：SpecsInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpecsInfo {
    pub id: i32,
    pub minimum: i32,
    pub recommended: i32,
}

/// FTB 版本目标信息（源：TargetInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub updated: i64,
}

/// FTB 作者信息（源：AuthorInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthorInfo {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    pub updated: i64,
}

/// FTB 链接信息（源：LinkInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    pub id: i32,
    pub name: String,
    pub link: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

/// FTB 美术资源信息（源：ArtInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtInfo {
    pub id: i32,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub width: i32,
    pub height: i32,
    pub compressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    pub size: i64,
    pub updated: i64,
}

/// FTB 元信息（源：MetaInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetaInfo {
    pub supports_worlds: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curseforge_project_id: Option<i32>,
    pub is_legacy: bool,
}

/// FTB 内容分级信息（源：RatingInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RatingInfo {
    pub id: i32,
    pub configured: bool,
    pub verified: bool,
    pub age: i32,
    pub gambling: bool,
    pub frightening: bool,
    #[serde(rename = "alcoholdrugs")]
    pub alcohol_drugs: bool,
    #[serde(rename = "nuditysexual")]
    pub nudity_sexual: bool,
    #[serde(rename = "sterotypeshate")]
    pub stereotypes_hate: bool,
    pub language: bool,
    pub violence: bool,
}

/// FTB 版本详情（源：VersionDetail）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionDetail {
    pub id: i32,
    pub parent: i32,
    pub name: String,
    pub r#type: String,
    pub plays: i64,
    pub installs: i64,
    pub updated: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specs: Option<SpecsInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<TargetInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FtbFileInfo>>,
}

/// FTB 版本 Mods 详情（源：ModsDetail）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModsDetail {
    pub id: i32,
    pub parent: i32,
    pub name: String,
    pub r#type: String,
    pub plays: i64,
    pub installs: i64,
    pub updated: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specs: Option<SpecsInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<TargetInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mods: Option<Vec<FtbModInfo>>,
}

/// FTB Mod 信息（源：FtbModInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FtbModInfo {
    pub curse_project: i64,
    pub curse_file: i64,
    pub name: String,
    pub filename: String,
    pub curse_slug: String,
    pub size: i64,
}

/// FTB 文件信息（源：FtbFileInfo）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FtbFileInfo {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    pub size: i64,
    #[serde(rename = "clientonly")]
    pub client_only: bool,
    #[serde(rename = "serveronly")]
    pub server_only: bool,
    pub optional: bool,
    pub updated: i64,
}

/// FTB 更新日志结果（源：ChangelogResult）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    pub updated: i64,
}

/// FTB 缓存数据（源：CacheData）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CacheData {
    pub saved_at: i64,
    pub modpacks: Vec<ModpackInfo>,
}
