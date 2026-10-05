//! 扩展平台数据源 traits：Modrinth / CurseForge / FTB（B3）
//!
//! 对应源文件：
//! - `Public/Expansion/IModrinthSource.cs`（IModrinthSource）
//! - `Public/Expansion/ICurseForgeSource.cs`（ICurseForgeSource）
//! - `Public/Expansion/IFTBSource.cs`（IFTBSource）
//!
//! 方法映射表（C# → Rust）：
//!
//! ModrinthSource（源 IModrinthSource）：
//! - `SearchAsync(...) -> Task<SearchResult>` → `search(...) -> Result<SearchResult, Error>`
//! - `GetProjectInfoAsync(string) -> Task<ProjectInfo>` → `get_project_info(...) -> Result<ProjectInfo, Error>`
//! - `GetProjectVersionInfoAsync(string) -> Task<List<ProjectVersionInfo>>` → `get_project_version_info(...) -> Result<Vec<ProjectVersionInfo>, Error>`
//! - `GetVersionInfoAsync(string) -> Task<VersionInfo>` → `get_version_info(...) -> Result<ModrinthVersionInfo, Error>`
//! - `GetProjectVersionsFromHashesAsync(List<string>) -> Task<List<ProjectVersionInfo>>` → `get_project_versions_from_hashes(...)`
//! - `GetProjectVersionsFromHashesDictAsync(List<string>) -> Task<Dictionary<string, ProjectVersionInfo>>` → `get_project_versions_from_hashes_dict(...) -> Result<HashMap<String, ProjectVersionInfo>, Error>`
//! - `GetCategoriesAsync() / GetLoadersAsync() / GetProjectTypesAsync() -> Task<List<ModrinthTag>>` → `get_categories() / get_loaders() / get_project_types() -> Result<Vec<ModrinthTag>, Error>`
//!
//! CurseForgeSource（源 ICurseForgeSource）：
//! - `SearchAsync(...) -> Task<CurseForgeSearchResponse>` → `search(...) -> Result<CurseForgeSearchResponse, Error>`
//! - `GetModInfoAsync(string) -> Task<CurseForgeInfo>` → `get_mod_info(...)`
//! - `GetFileInfoAsync(string, string) -> Task<CurseForgeFileInfo>` → `get_file_info(...)`
//! - `GetDownloadUrlAsync(string, string) -> Task<string>` → `get_download_url(...) -> Result<String, Error>`
//! - `GetInfoFromHashesAsync(List<long>) -> Task<List<FingerprintsFilesMeta>>` → `get_info_from_hashes(...)`
//! - `GetInfoFromHashesDictAsync(List<long>) -> Task<Dictionary<long, FingerprintsFilesMeta>>` → `get_info_from_hashes_dict(...) -> Result<HashMap<i64, FingerprintsFilesMeta>, Error>`
//!
//! FtbSource（源 IFTBSource）：
//! - `SearchAsync(...) -> Task<List<ModpackInfo>>` → `search(...) -> Result<Vec<ModpackInfo>, Error>`
//! - `GetPackDetailAsync(int) -> Task<ModpackInfo?>` → `get_pack_detail(...) -> Result<Option<ModpackInfo>, Error>`
//! - `GetVersionDetailAsync(int, int) -> Task<VersionDetail?>` → `get_version_detail(...) -> Result<Option<VersionDetail>, Error>`
//! - `GetChangelogAsync(int, int) -> Task<ChangelogResult?>` → `get_changelog(...) -> Result<Option<ChangelogResult>, Error>`
//! - `static VersionInfo? GetLatestVersion(ModpackInfo)`（默认实现）→ 模块级自由函数 `get_latest_version(pack) -> Option<FtbVersionInfo>`
//!
//! 注：`modrinth::VersionInfo` 与 `ftb::VersionInfo` 同名冲突，分别别名导入
//! 为 `ModrinthVersionInfo` / `FtbVersionInfo`（对应源 C# 类型名均仍为 VersionInfo）。

use crate::error::Error;
use crate::models::expansion::curseforge::{
    CurseForgeBatchFileInfo, CurseForgeFileInfo, CurseForgeFingerprintMatch, CurseForgeInfo,
    CurseForgeSearchResponse, FingerprintsFilesMeta,
};
use crate::models::expansion::ftb::{
    ChangelogResult, ModpackInfo, VersionDetail, VersionInfo as FtbVersionInfo,
};
use crate::models::expansion::modrinth::{
    ModrinthTag, ProjectInfo, ProjectVersionInfo, SearchResult, VersionInfo as ModrinthVersionInfo,
};
use crate::models::expansion::technic::{
    TechnicPackDetail, TechnicPackSummary, TechnicSolderBuild, TechnicSolderPack,
};
use async_trait::async_trait;
use std::collections::HashMap;

/// Modrinth 数据源（源：IModrinthSource 接口）。
///
/// 提供 Modrinth 平台的项目搜索、项目/版本详情、哈希反查与标签列表查询。
#[async_trait]
pub trait ModrinthSource: Send + Sync {
    /// 搜索项目（源：`SearchAsync`）。
    ///
    /// `project_type` / `game_version` / `categories` / `loaders` 为可选筛选条件
    /// （C# 可空参数，`None` 表示不指定）；
    /// `index` 为排序方式（C# 默认参数 `"relevance"`，Rust 无默认参数，调用方需显式传入）；
    /// `page` 为页码（C# 默认参数 `0`）；`page_size` 为每页条数（C# 默认参数 `20`）。
    async fn search(
        &self,
        query: &str,
        project_type: Option<&str>,
        game_version: Option<&str>,
        categories: Option<&[String]>,
        loaders: Option<&[String]>,
        index: &str,
        page: i32,
        page_size: i32,
    ) -> Result<SearchResult, Error>;

    /// 获取项目详情（源：`GetProjectInfoAsync`）。
    async fn get_project_info(&self, project_id: &str) -> Result<ProjectInfo, Error>;

    /// 获取项目全部版本信息（源：`GetProjectVersionInfoAsync`）。
    async fn get_project_version_info(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectVersionInfo>, Error>;

    /// 获取单个版本详情（源：`GetVersionInfoAsync`）。
    async fn get_version_info(&self, version_id: &str) -> Result<ModrinthVersionInfo, Error>;

    /// 通过哈希值反查项目版本信息（源：`GetProjectVersionsFromHashesAsync`）。
    async fn get_project_versions_from_hashes(
        &self,
        hashes: &[String],
    ) -> Result<Vec<ProjectVersionInfo>, Error>;

    /// 通过哈希值反查项目版本信息，返回 版本ID → 版本信息 映射（源：`GetProjectVersionsFromHashesDictAsync`）。
    async fn get_project_versions_from_hashes_dict(
        &self,
        hashes: &[String],
    ) -> Result<HashMap<String, ProjectVersionInfo>, Error>;

    /// 通过哈希值反查匹配指定加载器/游戏版本的最新版本，返回 哈希 → 最新版本 映射
    /// （Modrinth `POST v2/version_files/update`；源 C# 无对应方法，为更新检查新增）。
    async fn get_latest_versions_from_hashes(
        &self,
        hashes: &[String],
        loaders: &[String],
        game_versions: &[String],
    ) -> Result<HashMap<String, ModrinthVersionInfo>, Error>;

    /// 获取分类标签列表（源：`GetCategoriesAsync`）。
    async fn get_categories(&self) -> Result<Vec<ModrinthTag>, Error>;

    /// 获取加载器标签列表（源：`GetLoadersAsync`）。
    async fn get_loaders(&self) -> Result<Vec<ModrinthTag>, Error>;

    /// 获取项目类型标签列表（源：`GetProjectTypesAsync`）。
    async fn get_project_types(&self) -> Result<Vec<ModrinthTag>, Error>;
}

/// CurseForge 数据源（源：ICurseForgeSource 接口）。
///
/// 提供 CurseForge 平台的模组搜索、模组/文件详情、下载地址与指纹反查。
#[async_trait]
pub trait CurseForgeSource: Send + Sync {
    /// 搜索模组（源：`SearchAsync`）。
    ///
    /// `search_filter` 为搜索关键字；`game_versions` / `mod_loader_types` 为可选筛选
    /// （C# 可空数组，`None` 表示不指定）；`categories` 为可空分类 ID 数组，
    /// 数组元素本身可空（C# `int?[]?` → `Option<&[Option<i32>]>`）；
    /// `sort_field`（C# 默认 `1`）、`page`（默认 `1`）、`page_size`（默认 `25`）、
    /// `class_id`（默认 `null`）均可空，`None` 表示不指定/使用默认。
    async fn search(
        &self,
        search_filter: &str,
        game_versions: Option<&[String]>,
        categories: Option<&[Option<i32>]>,
        mod_loader_types: Option<&[String]>,
        sort_field: Option<i32>,
        page: Option<i32>,
        page_size: Option<i32>,
        class_id: Option<i32>,
    ) -> Result<CurseForgeSearchResponse, Error>;

    /// 获取模组详情（源：`GetModInfoAsync`）。
    async fn get_mod_info(&self, id: &str) -> Result<CurseForgeInfo, Error>;

    /// 获取模组文件信息（源：`GetFileInfoAsync`）。
    async fn get_file_info(&self, mod_id: &str, file_id: &str)
    -> Result<CurseForgeFileInfo, Error>;

    /// 获取文件下载地址（源：`GetDownloadUrlAsync`）。
    async fn get_download_url(&self, id: &str, file_id: &str) -> Result<String, Error>;

    /// 批量获取文件信息（源 `GetFilesAsync`；每批 MaxBatchFileIds=100 自动分批）。
    /// 返回 fileId → 文件信息（含 fileName/downloadUrl）；整合包安装用它把
    /// projectID:fileID 清单一次解析为下载链接，替代逐文件两次串行查询。
    async fn get_files_batch(
        &self,
        file_ids: &[i64],
    ) -> Result<HashMap<i64, CurseForgeBatchFileInfo>, Error>;

    /// 通过指纹反查文件信息（源：`GetInfoFromHashesAsync`）。
    async fn get_info_from_hashes(
        &self,
        hashes: &[i64],
    ) -> Result<Vec<FingerprintsFilesMeta>, Error>;

    /// 通过指纹反查文件信息，返回 指纹 → 文件信息 映射（源：`GetInfoFromHashesDictAsync`）。
    async fn get_info_from_hashes_dict(
        &self,
        hashes: &[i64],
    ) -> Result<HashMap<i64, FingerprintsFilesMeta>, Error>;

    /// 通过指纹反查完整命中（file = 当前已安装文件，latestFiles = 候选更新文件）。
    /// 源 C# 无对应方法，为批次更新检查新增。
    async fn get_fingerprint_matches(
        &self,
        hashes: &[i64],
    ) -> Result<Vec<CurseForgeFingerprintMatch>, Error>;
}

/// FTB 数据源（源：IFTBSource 接口）。
///
/// 提供 FTB App 的整合包搜索、整合包/版本/更新日志查询。
#[async_trait]
pub trait FtbSource: Send + Sync {
    /// 搜索整合包（源：`SearchAsync`）。
    ///
    /// `query` / `tags` / `mc_version` / `loader` 为可选筛选（C# 可空参数，`None` 表示不指定）；
    /// `sort` 为排序方式（C# 默认参数 `"featured"`，Rust 无默认参数，调用方需显式传入）；
    /// `limit` 为返回条数上限（C# 默认参数 `20`）。
    async fn search(
        &self,
        query: Option<&str>,
        tags: Option<&[String]>,
        mc_version: Option<&str>,
        loader: Option<&str>,
        sort: &str,
        limit: i32,
    ) -> Result<Vec<ModpackInfo>, Error>;

    /// 获取整合包详情（源：`GetPackDetailAsync`，可空返回 → `Option`）。
    async fn get_pack_detail(&self, id: i32) -> Result<Option<ModpackInfo>, Error>;

    /// 获取整合包指定版本详情（源：`GetVersionDetailAsync`，可空返回 → `Option`）。
    async fn get_version_detail(
        &self,
        pack_id: i32,
        version_id: i32,
    ) -> Result<Option<VersionDetail>, Error>;

    /// 获取整合包指定版本的更新日志（源：`GetChangelogAsync`，可空返回 → `Option`）。
    async fn get_changelog(
        &self,
        pack_id: i32,
        version_id: i32,
    ) -> Result<Option<ChangelogResult>, Error>;
}

/// 获取整合包最新正式版本（源：IFTBSource 的 `GetLatestVersion` 静态方法默认实现）。
///
/// C# 为 static 方法（无实例语义，且接口方法为对象安全需要，无 self 的关联函数
/// 会破坏 dyn 兼容性）→ 映射为模块级自由函数。
/// 逻辑：从 `pack.Versions` 中筛选 `Type == "release"` 且 `Updated` 最大的版本；
/// `Versions` 为空或没有 release 版本时返回 `None`。
pub fn get_latest_version(pack: &ModpackInfo) -> Option<FtbVersionInfo> {
    pack.versions
        .as_ref()
        .and_then(|versions| {
            versions
                .iter()
                .filter(|v| v.r#type == "release")
                .max_by_key(|v| v.updated)
        })
        .cloned()
}

/// Technic 数据源（issue #151，**无 C# 源对应**）。
///
/// 提供 Technic 平台的整合包搜索/浏览与详情查询。
///
/// # 与其它三个源的语义差异（消费方必读）
///
/// - **无分页**：Technic API 的 `/search` 固定返回 15 条且忽略 `page`/`sort`；
///   `/trending` 返回 20 条。因此本 trait 的 `search` **不接收** `page`/`page_size`，
///   返回 `(Vec<TechnicPackSummary>, i32)` 中的总数即实际返回条数（如实回报，
///   不伪造分页语义）。
/// - **无版本/fileId 概念**：一个包只有一个直链（`url`），没有版本列表。
///   故本 trait 不提供 `versions` / `downloads` 类方法。
/// - **slug 是唯一键**：数字 id 在详情接口上 404（实测），详情按 slug 查询。
#[async_trait]
pub trait TechnicSource: Send + Sync {
    /// 搜索整合包（`GET /search?q={kw}`）。
    ///
    /// `query` 为空时服务端返回 400，调用方应改走 [`TechnicSource::trending`]。
    /// 返回 `(列表, 总数)`；总数即本次实际返回条数（服务端无独立 total 字段）。
    async fn search(&self, query: &str) -> Result<(Vec<TechnicPackSummary>, i32), Error>;

    /// 浏览热门整合包（`GET /trending`，搜索无关键词时的默认列表）。
    async fn trending(&self) -> Result<(Vec<TechnicPackSummary>, i32), Error>;

    /// 整合包详情（`GET /modpack/{slug}`）。
    ///
    /// 不存在的 slug → `Ok(None)`（服务端 404 且响应体为 `{"error":"Modpack does not exist"}`）。
    async fn get_pack_detail(&self, slug: &str) -> Result<Option<TechnicPackDetail>, Error>;

    /// Solder build 列表（`GET {solder}/modpack/{slug}`，issue #181 期3）。
    ///
    /// `solder` 基地址来自 [`TechnicPackDetail::solder`]（仅 Solder 分发包有值）。
    /// Solder 无 `build` 参数要求（与 api.technicpack.net 不同，期1 实测）。
    /// build 列表为空 / 响应畸形 → `Ok(None)`（「无可安装内容」语义，与 404 对齐）。
    async fn get_solder_pack(
        &self,
        solder_base: &str,
        slug: &str,
    ) -> Result<Option<TechnicSolderPack>, Error>;

    /// Solder build 详情（`GET {solder}/modpack/{slug}/{build}`，issue #181 期3）。
    ///
    /// 返回该 build 的完整 mod 清单（含逐文件 MD5 与 CDN 直链）。
    /// build 不存在 → `Ok(None)`。
    async fn get_solder_build(
        &self,
        solder_base: &str,
        slug: &str,
        build: &str,
    ) -> Result<Option<TechnicSolderBuild>, Error>;
}
