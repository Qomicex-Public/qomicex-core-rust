//! Modrinth API：搜索 / 项目详情 / 版本 / 哈希反查 / 标签（B13）
//!
//! 对应源文件：`Services/Expansion/Modrinth/ModrinthBase.cs`（160 行，internal class ModrinthBase : IModrinthSource）。
//! 契约：`src/api/expansion.rs` 的 `ModrinthSource` trait（9 方法，B3 定案）。
//! 模型：`src/models/expansion/modrinth.rs`（B1 全量移植）。
//!
//! 端点清单（URL 模板/查询参数/JSON 字段逐字保留，改一个字符请求即失败）：
//! - GET  `{base}v2/search?query={q}&facets={...}&limit={n}&offset={n}&index={index}`
//! - GET  `{base}v2/project/{id}`
//! - GET  `{base}v2/project/{id}/version`
//! - GET  `{base}v2/version/{id}`
//! - POST `{base}v2/version_files`（body：`{"hashes":[...],"algorithm":"sha1"}`，源 VersionFilesRequest）
//! - GET  `{base}v2/tag/category` / `{base}v2/tag/loader` / `{base}v2/tag/project_type`
//!
//! 依赖类说明（源同目录 6 个派生类 Mods/DataPacks/Modpacks/ResourcePacks/Shaders/Worlds.cs）：
//! **不移植为独立类型** —— 它们仅把 `SearchAsync` 的 `projectType` 固化为常量后转调基类，
//! Rust 侧该语义由 `search()` 的 `project_type` 参数 + `models::expansion::modrinth::project_type`
//! 常量承载（调用方显式传入）。各派生类固化值：Mods="mod"、DataPacks="datapack"、
//! Modpacks="modpack"、ResourcePacks="resourcepack"、Shaders="shader"、Worlds="mod"
//! （⚠️ 源 Worlds.cs 固化值同为 "mod"，与 Mods 相同，按源保留）。
//!
//! 差异说明：
//! - User-Agent：源 ModrinthBase 自身不设置——共享 HttpClient 的 DefaultRequestHeaders.UserAgent
//!   由 GameCoreBuilder.Build() 从 CoreOptions.UserAgent 注入（Modrinth API 要求）；
//!   Rust 侧共享 reqwest::Client 由 builder 统一构造（builder 设 .user_agent() 则全部请求自动携带）。
//!   本类仅添加 Accept: application/json（源 DefaultRequestHeaders.Accept）——reqwest 无法
//!   事后修改已注入 client 的默认头 → 逐请求设置，语义等价。
//! - 错误映射：网络/JSON/非 2xx → Error::Http（源 HttpRequestException/JsonException，
//!   B6 先例 services/version/manifest.rs）；参数校验（Argument*/OutOfRange）→ Error::Params；
//!   源反序列化 null 抛 InvalidOperationException 与 JsonException 语义合并（均 → Error::Http）。
//! - ⚠️ 源 GetProjectVersionInfoAsync 内的转换引用 ModrinthVersionInfo 不存在的成员且
//!   Files 类型不匹配（List<ModrinthFile> vs List<VersionFileInfo>）——属源编译错误，
//!   本文件按"可用字段保真 + 缺失字段取默认"移植，见 to_project_version_info。
//! - `get_project_versions_from_hashes` 的返回顺序不保证（源 Dictionary.Values 插入序，
//!   非 API 契约）；Rust HashMap 无序 → into_values()。

use std::collections::HashMap;

use async_trait::async_trait;

use crate::api::expansion::ModrinthSource;
use crate::error::Error;
use crate::models::expansion::modrinth::{
    FileHashes, ModrinthFile, ModrinthTag, ModrinthVersionInfo, ProjectInfo, ProjectVersionInfo,
    SearchResult, VersionFileInfo, VersionFilesRequest, VersionFilesUpdateRequest, VersionInfo,
};

/// 默认基础 URL（源：`private const string DefaultBaseUrl`，逐字保留）
const DEFAULT_BASE_URL: &str = "https://api.modrinth.com/";

/// Modrinth 数据源基类（源：`internal class ModrinthBase : IModrinthSource`）。
///
/// 提供 Modrinth 平台的项目搜索、项目/版本详情、哈希反查与标签列表查询。
pub(crate) struct ModrinthBase {
    /// 共享 HTTP 客户端（源：`_http` HttpClient）
    http: reqwest::Client,
    /// 基础 URL（源：`_baseUrl`，构造时 `(baseUrl ?? Default).TrimEnd('/') + "/"`）
    base_url: String,
}

impl ModrinthBase {
    /// 创建数据源（源：构造函数 `ModrinthBase(HttpClient http, string? baseUrl = null)`）。
    ///
    /// `base_url` 为 `None` 时使用默认地址（Rust 无默认参数，C# 默认参数 → Option）；
    /// 尾斜杠规范化 `TrimEnd('/') + "/"` 按源保留。
    ///
    /// User-Agent 说明：源此处仅添加 `Accept: application/json`（对应本实现逐请求
    /// 添加 Accept 头）；User-Agent 由共享 client 承载（源 builder 注入 CoreOptions.UserAgent）。
    pub(crate) fn new(http: reqwest::Client, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string() + "/",
        }
    }

    /// GET 请求并返回响应体文本（源：`GetDataAsync`）。
    ///
    /// URL 前缀逻辑按源逐字保留：不以 `http` 开头时拼上 `_baseUrl`（本类调用点均
    /// 已带 baseUrl 前缀，该分支实际不触发，但语义保留）；非 2xx 按 .NET
    /// `EnsureSuccessStatusCode` 的 HttpRequestException 消息报错。
    async fn get_data(&self, url: &str) -> Result<String, Error> {
        let url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{}", self.base_url, url)
        };

        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(http_err)?;
        let status = resp.status();
        let body = resp.text().await.map_err(http_err)?;

        if !status.is_success() {
            return Err(Error::Http {
                message: format!(
                    "Response status code does not indicate success: {} ({}).",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
                status: None,
                source: None,
            });
        }
        Ok(body)
    }

    /// POST JSON 请求并返回响应体文本（源：`PostDataAsync`）。
    ///
    /// Content-Type 固定 `application/json` + UTF-8（源 StringContent(jsonData, Encoding.UTF8,
    /// "application/json")）；URL 前缀与错误处理同 `get_data`。
    async fn post_data(&self, url: &str, json_data: &str) -> Result<String, Error> {
        let url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{}", self.base_url, url)
        };

        let resp = self
            .http
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(json_data.to_string())
            .send()
            .await
            .map_err(http_err)?;
        let status = resp.status();
        let body = resp.text().await.map_err(http_err)?;

        if !status.is_success() {
            return Err(Error::Http {
                message: format!(
                    "Response status code does not indicate success: {} ({}).",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
                status: None,
                source: None,
            });
        }
        Ok(body)
    }

    /// 获取标签列表（源：`GetTagsAsync(string tagType)`）。
    ///
    /// 空/空白 tagType → Error::Params（源 ArgumentException.ThrowIfNullOrEmpty）；
    /// 响应为标签对象列表，null → 空列表（源 `?? []`）。
    async fn get_tags(&self, tag_type: &str) -> Result<Vec<ModrinthTag>, Error> {
        if tag_type.is_empty() {
            return Err(Error::Params {
                message: format!(
                    "tagType 不能为空（源 ArgumentException.ThrowIfNullOrEmpty，参数 {tag_type}）"
                ),
                source: None,
            });
        }

        let body = self
            .get_data(&format!("{}v2/tag/{}", self.base_url, tag_type))
            .await?;
        let list: Option<Vec<ModrinthTag>> = serde_json::from_str(&body).map_err(json_err)?;
        Ok(list.unwrap_or_default())
    }
}

#[async_trait]
impl ModrinthSource for ModrinthBase {
    /// 搜索项目（源：`SearchAsync`）。
    ///
    /// URL 构造逐字保留：`v2/search?query={q}` + 可选 facets（project_type /
    /// categories / loaders / game_version，其中 loaders 源同样使用 `categories:` 前缀——
    /// Modrinth API 加载器筛选本就归入 categories facet，按源保留）+
    /// `&limit={pageSize}&offset={page * pageSize}&index={index}`；
    /// 参数编码对应 .NET Uri.EscapeDataString（RFC 3986 unreserved，见 escape_data_string）。
    ///
    /// 校验按源：pageSize 为负 → Error::Params（源 ArgumentOutOfRangeException.ThrowIfNegative）；
    /// pageSize > 100 → Error::Params（源 ArgumentOutOfRangeException，"每页数量最大 100"）。
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
    ) -> Result<SearchResult, Error> {
        if page_size < 0 {
            return Err(Error::Params {
                message: format!(
                    "pageSize 不能为负数（源 ArgumentOutOfRangeException.ThrowIfNegative，参数 pageSize={page_size}）"
                ),
                source: None,
            });
        }
        if page_size > 100 {
            return Err(Error::Params {
                message: "每页数量最大 100".to_string(),
                source: None,
            });
        }

        let mut url = format!(
            "{}v2/search?query={}",
            self.base_url,
            escape_data_string(query)
        );

        let mut facets: Vec<String> = Vec::new();
        if let Some(pt) = project_type {
            if !pt.is_empty() {
                facets.push(format!("[\"project_type:{}\"]", escape_data_string(pt)));
            }
        }
        if let Some(cats) = categories {
            for c in cats {
                facets.push(format!("[\"categories:{}\"]", escape_data_string(c)));
            }
        }
        if let Some(loads) = loaders {
            for l in loads {
                facets.push(format!("[\"categories:{}\"]", escape_data_string(l)));
            }
        }
        if let Some(gv) = game_version {
            if !gv.is_empty() {
                facets.push(format!("[\"versions:{}\"]", escape_data_string(gv)));
            }
        }
        if !facets.is_empty() {
            let joined = format!("[{}]", facets.join(","));
            url.push_str(&format!("&facets={}", escape_data_string(&joined)));
        }

        url.push_str(&format!(
            "&limit={page_size}&offset={}&index={}",
            page * page_size,
            escape_data_string(index)
        ));

        let body = self.get_data(&url).await?;
        serde_json::from_str(&body).map_err(|e| Error::Http {
            message: "搜索结果反序列化失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 获取项目详情（源：`GetProjectInfoAsync`）。
    ///
    /// 空/空白 projectId → Error::Params（源 ArgumentException.ThrowIfNullOrEmpty）；
    /// 反序列化失败 → Error::Http（源 null 时抛 InvalidOperationException"项目信息反序列化失败"）。
    async fn get_project_info(&self, project_id: &str) -> Result<ProjectInfo, Error> {
        if project_id.is_empty() {
            return Err(Error::Params {
                message: "projectId 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）"
                    .to_string(),
                source: None,
            });
        }

        let body = self
            .get_data(&format!(
                "{}v2/project/{}",
                self.base_url,
                escape_data_string(project_id)
            ))
            .await?;
        serde_json::from_str(&body).map_err(|e| Error::Http {
            message: "项目信息反序列化失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 获取项目全部版本信息（源：`GetProjectVersionInfoAsync`）。
    ///
    /// 响应为 ModrinthVersionInfo 列表（源 ListVersionInfo 上下文），逐项转换为
    /// ProjectVersionInfo（源 `v => new ProjectVersionInfo(...)`）。
    ///
    /// ⚠️ 源 C# 转换引用了 ModrinthVersionInfo 不存在的成员（v.GameVersionIds /
    /// v.Changelog / v.PublishedAt / v.DownloadCount / v.DependenciesInfos）且 v.Files
    /// 类型（List<ModrinthFile>）与目标 Files（List<VersionFileInfo>）不匹配——
    /// 属源编译错误；本文件按字段语义保真映射，缺失字段取 None/0（见
    /// `to_project_version_info` 各 ⚠️ 标注）。
    async fn get_project_version_info(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectVersionInfo>, Error> {
        if project_id.is_empty() {
            return Err(Error::Params {
                message: "projectId 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）"
                    .to_string(),
                source: None,
            });
        }

        let body = self
            .get_data(&format!(
                "{}v2/project/{}/version",
                self.base_url,
                escape_data_string(project_id)
            ))
            .await?;
        let list: Option<Vec<ModrinthVersionInfo>> =
            serde_json::from_str(&body).map_err(json_err)?;
        Ok(list
            .unwrap_or_default()
            .into_iter()
            .map(to_project_version_info)
            .collect())
    }

    /// 获取单个版本详情（源：`GetVersionInfoAsync`）。
    ///
    /// 空/空白 versionId → Error::Params（源 ArgumentException.ThrowIfNullOrEmpty）；
    /// 返回 `models::expansion::modrinth::VersionInfo`（C# VersionInfo；
    /// 契约 api/expansion.rs 中别名 ModrinthVersionInfo 即该类型）。
    async fn get_version_info(&self, version_id: &str) -> Result<VersionInfo, Error> {
        if version_id.is_empty() {
            return Err(Error::Params {
                message: "versionId 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）"
                    .to_string(),
                source: None,
            });
        }

        let body = self
            .get_data(&format!(
                "{}v2/version/{}",
                self.base_url,
                escape_data_string(version_id)
            ))
            .await?;
        serde_json::from_str(&body).map_err(|e| Error::Http {
            message: "版本信息反序列化失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 通过哈希值反查项目版本信息（源：`GetProjectVersionsFromHashesAsync`）。
    ///
    /// 委托 `get_project_versions_from_hashes_dict` 后取全部值
    /// （源 `dict.Values.ToList()`）。⚠️ 返回顺序不保证——源为 Dictionary 插入序，
    /// Rust HashMap 无序（非 API 契约，不影响语义）。
    async fn get_project_versions_from_hashes(
        &self,
        hashes: &[String],
    ) -> Result<Vec<ProjectVersionInfo>, Error> {
        Ok(self
            .get_project_versions_from_hashes_dict(hashes)
            .await?
            .into_values()
            .collect())
    }

    /// 通过哈希值反查项目版本信息，返回 版本ID → 版本信息 映射
    /// （源：`GetProjectVersionsFromHashesDictAsync`）。
    ///
    /// POST `v2/version_files`，body 为 VersionFilesRequest（源 VersionFilesRequest：
    /// `{"hashes":[...],"algorithm":"sha1"}`，algorithm 为 C# 默认参数值，模型 serde 默认承载）；
    /// 响应为 哈希 → ModrinthVersionInfo 映射（源 DictionaryStringModrinthVersionInfo 上下文），
    /// 逐项转换为 ProjectVersionInfo（Changelog=null / DownloadCount=0 / VersionType=null /
    /// Files=null / DependenciesInfos=null 为源显式传值，非缺失）；
    /// 响应为 null → 空映射（源 `if (dict == null) return [];`）。
    async fn get_project_versions_from_hashes_dict(
        &self,
        hashes: &[String],
    ) -> Result<HashMap<String, ProjectVersionInfo>, Error> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let request = VersionFilesRequest {
            hashes: hashes.to_vec(),
            algorithm: "sha1".to_string(),
        };
        let json_data = serde_json::to_string(&request).map_err(|e| Error::Http {
            message: "版本文件反查请求序列化失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;

        let body = self
            .post_data(&format!("{}v2/version_files", self.base_url), &json_data)
            .await?;
        // 源先反序列化为可空字典，null → 空映射 → 以 Option<HashMap> 承载
        let dict: Option<HashMap<String, ModrinthVersionInfo>> =
            serde_json::from_str(&body).map_err(json_err)?;

        Ok(dict
            .unwrap_or_default()
            .into_iter()
            .map(|(hash, v)| (hash, to_project_version_info_from_hash(v)))
            .collect())
    }

    /// 通过哈希值反查匹配指定加载器/游戏版本的最新版本，返回 哈希 → 最新版本 映射
    /// （源 C# 无对应方法，为批次更新检查新增；POST `v2/version_files/update`）。
    ///
    /// body 为 VersionFilesUpdateRequest：`{"hashes":[...],"algorithm":"sha1",
    /// "loaders":[...],"game_versions":[...]}`（空数组表示不限）；
    /// 响应为 哈希 → VersionInfo 映射；响应为 null → 空映射。
    async fn get_latest_versions_from_hashes(
        &self,
        hashes: &[String],
        loaders: &[String],
        game_versions: &[String],
    ) -> Result<HashMap<String, VersionInfo>, Error> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let request = VersionFilesUpdateRequest {
            hashes: hashes.to_vec(),
            algorithm: "sha1".to_string(),
            loaders: loaders.to_vec(),
            game_versions: game_versions.to_vec(),
        };
        let json_data = serde_json::to_string(&request).map_err(|e| Error::Http {
            message: "最新版本反查请求序列化失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;

        let body = self
            .post_data(
                &format!("{}v2/version_files/update", self.base_url),
                &json_data,
            )
            .await?;
        // 源先反序列化为可空字典，null → 空映射 → 以 Option<HashMap> 承载
        let dict: Option<HashMap<String, VersionInfo>> =
            serde_json::from_str(&body).map_err(json_err)?;
        Ok(dict.unwrap_or_default())
    }

    /// 获取分类标签列表（源：`GetCategoriesAsync` → `GetTagsAsync("category")`）。
    async fn get_categories(&self) -> Result<Vec<ModrinthTag>, Error> {
        self.get_tags("category").await
    }

    /// 获取加载器标签列表（源：`GetLoadersAsync` → `GetTagsAsync("loader")`）。
    async fn get_loaders(&self) -> Result<Vec<ModrinthTag>, Error> {
        self.get_tags("loader").await
    }

    /// 获取项目类型标签列表（源：`GetProjectTypesAsync`）。
    ///
    /// 响应为字符串列表（源 ListString 上下文），逐项包装为
    /// `ModrinthTag(name, icon: null, description: null)`（源
    /// `new ModrinthTag(t, null, null)`）；null → 空列表（源 `?? []`）。
    async fn get_project_types(&self) -> Result<Vec<ModrinthTag>, Error> {
        let body = self
            .get_data(&format!("{}v2/tag/project_type", self.base_url))
            .await?;
        let string_tags: Option<Vec<String>> = serde_json::from_str(&body).map_err(json_err)?;
        Ok(string_tags
            .unwrap_or_default()
            .into_iter()
            .map(|t| ModrinthTag {
                name: t,
                icon: None,
                description: None,
            })
            .collect())
    }
}

/// ModrinthVersionInfo → ProjectVersionInfo 转换
/// （源：`GetProjectVersionInfoAsync` 内 `v => new ProjectVersionInfo(...)`）。
///
/// ⚠️ 源 C# 引用 ModrinthVersionInfo 不存在的成员（见 trait 方法注释），按可用字段保真：
/// - `changelog` → None（源 v.Changelog 不存在）
/// - `download_count` → 0（源 v.DownloadCount 不存在）
/// - `version_type` → None（源显式传 null）
/// - `dependencies_infos` → `v.dependencies` 直映（Rust 模型已补 dependencies 字段，
///   供前置模组/依赖解析使用；源虽不映射，但 API 恒携带依赖）
/// - `files` → 跨类型转换（源 List<ModrinthFile> vs List<VersionFileInfo> 不匹配，
///   属源编译错误；转换规则见 modrinth_file_to_version_file_info）
fn to_project_version_info(v: ModrinthVersionInfo) -> ProjectVersionInfo {
    ProjectVersionInfo {
        id: v.id,
        project_id: v.project_id,
        name: v.name,
        version_number: v.version_number,
        game_version_ids: v.game_versions,
        loaders: v.loaders,
        changelog: None, // ⚠️ UNMAPPED：源 ModrinthVersionInfo 无 changelog 字段
        published_at: v.date_published,
        download_count: 0, // ⚠️ UNMAPPED：源 ModrinthVersionInfo 无 downloads 字段
        version_type: None,
        files: v.files.map(|files| {
            files
                .into_iter()
                .map(modrinth_file_to_version_file_info)
                .collect()
        }),
        dependencies_infos: v.dependencies,
    }
}

/// ModrinthVersionInfo → ProjectVersionInfo 转换（哈希反查路径）
/// （源：`GetProjectVersionsFromHashesDictAsync` 内 `kv => new ProjectVersionInfo(...)`，
/// 源显式传 null/0 的字段原样保留，无缺失项）。
fn to_project_version_info_from_hash(v: ModrinthVersionInfo) -> ProjectVersionInfo {
    ProjectVersionInfo {
        id: v.id,
        project_id: v.project_id,
        name: v.name,
        version_number: v.version_number,
        game_version_ids: v.game_versions,
        loaders: v.loaders,
        changelog: None,
        published_at: v.date_published,
        download_count: 0,
        version_type: None,
        // ⚠️ 源 C# 在此路径显式 Files=null；Rust 保留 files，供 mrpack 导出等
        //    哈希反查场景取下载 URL/大小/哈希（mods.rs 更新检查只取 id/project_id，
        //    不受影响）。
        files: v.files.map(|files| {
            files
                .into_iter()
                .map(modrinth_file_to_version_file_info)
                .collect()
        }),
        dependencies_infos: None,
    }
}

/// ModrinthFile → VersionFileInfo 转换。
///
/// ⚠️ 仅源 `GetProjectVersionInfoAsync` 路径需要（v.Files 类型不匹配属源编译错误）：
/// 按字段名/语义取可提取部分——filename/url 直映；hashes 从 哈希算法→值 键值表
/// 提取 sha1/sha512（源 FileHashes 结构）；size/is_primary 源 C# ModrinthFile 缺失
/// （有损），Rust 模型已补字段（缺失按 0/false）。
fn modrinth_file_to_version_file_info(f: ModrinthFile) -> VersionFileInfo {
    VersionFileInfo {
        filename: f.filename,
        download_url: f.url,
        size: f.size.unwrap_or(0),
        is_primary: f.primary.unwrap_or(false),
        file_type: None,
        hashes: f.hashes.map(|h| FileHashes {
            sha1: h.get("sha1").cloned(),
            sha512: h.get("sha512").cloned(),
        }),
    }
}

/// 对应 .NET `Uri.EscapeDataString`：仅保留 RFC 3986 unreserved 字符
/// （ALPHA / DIGIT / `-` / `.` / `_` / `~`），其余按 UTF-8 字节 %XX 转义。
/// 与 Modrinth API 兼容性关键：facets 的 `[` `]` `"` `:` `,` 均被转义
/// （.NET 侧同样转义），若改用手写拼接或 form_urlencoded（`+` 编码空格）请求即失败。
fn escape_data_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 网络错误映射（源：HttpRequestException → Error::Http，消息格式沿用既有惯例）
fn http_err(e: reqwest::Error) -> Error {
    Error::Http {
        message: format!("HTTP 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    }
}

/// JSON 解析错误映射（源：JsonException → Error::Http，B6 先例 services/version/manifest.rs）
fn json_err(e: serde_json::Error) -> Error {
    Error::Http {
        message: format!("JSON 解析失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    }
}
