//! FTB API：整合包 / 版本 / 更新日志（B13）
//!
//! 对应源文件：
//! - `Services/Expansion/FeedTheBeast/FTBBase.cs`（FTBBase）
//! - `Services/Expansion/FeedTheBeast/Modpacks.cs`（`Modpacks : FTBBase`，无额外逻辑，直接复用本类型）
//! - `Public/Expansion/IFTBSource.cs`（接口 → `crate::api::expansion::FtbSource`，B3）
//!
//! 方法映射表（C# → Rust）：
//! - `FetchAllPacksAsync` → `fetch_all_packs`（内部方法）
//! - `SearchAsync` → `FtbSource::search`
//! - `GetPackDetailAsync` → `FtbSource::get_pack_detail`
//! - `GetVersionDetailAsync` → `FtbSource::get_version_detail`
//! - `GetChangelogAsync` → `FtbSource::get_changelog`
//! - `GetPackGameDetailAsync` → `get_pack_game_detail`（内部方法，不在接口中）
//! - `GetModDetailAsync` → `get_mod_detail`（内部方法，不在接口中）
//! - `static GetLatestVersion`（IFTBSource 默认实现）→ `crate::api::expansion::get_latest_version`（模块级函数，B3 已实现）
//! - `static FormatNumber` / `static FormatSize` → `FtbBase::format_number` / `FtbBase::format_size`（关联函数）
//! - `static GetDataDir` → 模块级 `get_data_dir`
//!
//! API 端点（基地址 `https://api.feed-the-beast.com/v1/modpacks/public`，URL/参数逐字保留自源）：
//! - GET `/modpack/all`（全部整合包 ID 列表，响应 `{"packs": [...]}`）
//! - GET `/modpack/{id}`（整合包详情）
//! - GET `/modpack/{packId}/{versionId}`（版本详情）
//! - GET `/modpack/{packId}/{versionId}/mods`（版本 Mods 详情）
//! - GET `/modpack/{packId}/{versionId}/changelog`（更新日志）
//!
//! 行为要点（与源一致）：
//! - 全量拉取带内存缓存 + 1 小时 TTL 的 JSON 文件缓存（`{dataDir}/QML/cache/ftb/ftb_cache.json`，CacheData 模型）
//! - 并发拉取各整合包详情，并发上限 8（源 `SemaphoreSlim(8)`），失败项丢弃
//! - 非 2xx 响应抛错（对应 `EnsureSuccessStatusCode`）；详情类查询错误全部吞掉返回 `None`（源 try/catch → null）
//! - 缓存文件读写错误全部吞掉（源 catch { }）

use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::expansion::{get_latest_version, FtbSource};
use crate::error::Error;
use crate::models::expansion::ftb::{
    CacheData, ChangelogResult, ModpackInfo, ModsDetail, VersionDetail,
};

/// 默认 API 基地址（源：`DefaultBaseUrl`）
const DEFAULT_BASE_URL: &str = "https://api.feed-the-beast.com/v1/modpacks/public";
/// 缓存文件名（源：`CacheFileName`）
const CACHE_FILE_NAME: &str = "ftb_cache.json";
/// 内存/文件缓存有效期（秒）（源：`DateTimeOffset.UtcNow.ToUnixTimeSeconds() - _cacheSavedAt < 3600`）
const CACHE_TTL_SECS: i64 = 3600;
/// 并发拉取整合包详情上限（源：`new SemaphoreSlim(8)`）
const MAX_CONCURRENT_FETCH: usize = 8;

/// 缓存锁（源：`static readonly SemaphoreSlim CacheLock = new(1, 1)`——所有实例共享）
static CACHE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 内存缓存状态（源：`_cache` 与 `_cacheSavedAt` 实例字段）
struct CacheState {
    modpacks: Vec<ModpackInfo>,
    saved_at: i64,
}

impl Default for CacheState {
    fn default() -> Self {
        Self {
            modpacks: Vec::new(),
            saved_at: 0,
        }
    }
}

/// FTB 数据源实现（源：`internal class FTBBase : IFTBSource`）。
///
/// 提供 FTB App 的整合包全量拉取（带缓存）、搜索、整合包/版本/更新日志查询。
/// 源派生类 `Modpacks : FTBBase`（Modpacks.cs）无任何额外逻辑，直接复用本类型。
pub(crate) struct FtbBase {
    /// 共享 HTTP 客户端（源：`_http` HttpClient；B4 定案：不持有、外部注入共享 client）
    http: reqwest::Client,
    /// API 基地址（源：`_baseUrl`，默认值去除尾部 '/'）
    base_url: String,
    /// 缓存目录（源：`_cacheDir`，默认 `{dataDir}/QML/cache/ftb`）
    cache_dir: String,
    /// 内存缓存（源：`_cache` / `_cacheSavedAt`；接口方法为 `&self`，需内部可变性）
    cache: Mutex<CacheState>,
}

impl FtbBase {
    /// 创建 FTB 数据源（源：构造函数 `FTBBase(HttpClient http, string? baseUrl, string? cacheDir)`）。
    ///
    /// `base_url` 为空时使用默认基地址并去除尾部 `/`；`cache_dir` 为空时使用
    /// `{dataDir}/QML/cache/ftb`。源在构造时给 HttpClient 设置 `Accept: application/json`，
    /// 本实现为共享客户端，改为每次请求带 Accept 头（B4 定案）。
    pub(crate) fn new(
        http: reqwest::Client,
        base_url: Option<String>,
        cache_dir: Option<String>,
    ) -> Self {
        let base_url = base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let cache_dir = cache_dir.unwrap_or_else(|| {
            Path::new(&get_data_dir())
                .join("QML")
                .join("cache")
                .join("ftb")
                .to_string_lossy()
                .into_owned()
        });
        Self {
            http,
            base_url,
            cache_dir,
            cache: Mutex::new(CacheState::default()),
        }
    }

    /// 缓存文件路径（源：`CacheFile` 属性）。
    /// 注意：`cache_dir` 为空时退化为相对路径 `ftb_cache.json`（同源 `Path.Combine("", ...)`）。
    fn cache_file(&self) -> std::path::PathBuf {
        Path::new(&self.cache_dir).join(CACHE_FILE_NAME)
    }

    /// 发起 GET 请求并返回响应体文本（源：`GetDataAsync`）。
    ///
    /// 相对路径（不以 `http` 开头）拼接基地址；非 2xx 抛错（对应 `EnsureSuccessStatusCode`）。
    async fn get_data_async(&self, url: &str) -> Result<String, Error> {
        let url = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{}", self.base_url, url)
        };
        let response = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Http {
                message: format!("GET {url} 失败"),
                status: None,
                source: Some(Box::new(e)),
            })?;
        if !response.status().is_success() {
            return Err(Error::Http {
                message: format!("请求失败，状态码: {}: {url}", response.status()),
                status: None,
                source: None,
            });
        }
        response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应体失败: {url}"),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 拉取单个整合包详情（源：`GET /modpack/{id}` + Deserialize，方法内 try/catch → null）。
    async fn fetch_pack_inner(&self, id: i32) -> Option<ModpackInfo> {
        let json = self.get_data_async(&format!("/modpack/{id}")).await.ok()?;
        serde_json::from_str(&json).ok()
    }

    /// 带并发上限的整合包详情拉取任务（源：`SemaphoreSlim(8)` + try/catch → null，finally Release）。
    async fn fetch_pack_with_semaphore(
        &self,
        id: i32,
        semaphore: Arc<tokio::sync::Semaphore>,
    ) -> Option<ModpackInfo> {
        let _permit = semaphore.acquire().await.ok()?;
        self.fetch_pack_inner(id).await
    }

    /// 拉取版本详情（源：`GetVersionDetailAsync` 内部，错误吞掉 → null）。
    async fn get_version_detail_inner(&self, pack_id: i32, version_id: i32) -> Option<VersionDetail> {
        let json = self
            .get_data_async(&format!("/modpack/{pack_id}/{version_id}"))
            .await
            .ok()?;
        serde_json::from_str(&json).ok()
    }

    /// 全量拉取整合包列表（源：`FetchAllPacksAsync`）。
    ///
    /// 流程与源一致：全局缓存锁 → 检查内存缓存（空则尝试读文件缓存）→ 未过期直接返回 →
    /// 否则 GET `/modpack/all` 取 ID 列表 → 并发 8 拉取每个详情（失败项丢弃）→ 更新内存缓存 →
    /// 尝试写回文件缓存（错误吞掉）。`/modpack/all` 请求/解析失败时向上抛错（源此处无 catch）。
    async fn fetch_all_packs(&self) -> Result<Vec<ModpackInfo>, Error> {
        let _lock = CACHE_LOCK.lock().await;
        let now = unix_timestamp_secs();

        {
            let mut cache = self.cache.lock().unwrap();
            if cache.modpacks.is_empty() {
                let cache_file = self.cache_file();
                if let Ok(json) = std::fs::read_to_string(&cache_file) {
                    if let Ok(cached) = serde_json::from_str::<CacheData>(&json) {
                        if !cached.modpacks.is_empty() {
                            cache.modpacks = cached.modpacks;
                            cache.saved_at = cached.saved_at;
                        }
                    }
                }
            }
            if !cache.modpacks.is_empty() && now - cache.saved_at < CACHE_TTL_SECS {
                return Ok(cache.modpacks.clone());
            }
        }

        let ids_json = self.get_data_async("/modpack/all").await?;
        let ids_doc: Value = serde_json::from_str(&ids_json).map_err(|e| Error::Http {
            message: "解析 /modpack/all 响应 JSON 失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;
        // 源：idsDoc["packs"] is JsonArray arr ? arr.Select(n => (int)(n ?? 0)).ToList() : []
        // （空/缺省 → 空列表；null 元素 → 0；非数字元素源会抛异常，此处按 0 处理 ⚠️）
        let ids: Vec<i32> = match ids_doc.get("packs") {
            Some(Value::Array(arr)) => arr
                .iter()
                .map(|n| n.as_i64().unwrap_or(0) as i32)
                .collect(),
            _ => Vec::new(),
        };

        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_FETCH));
        let tasks: Vec<_> = ids
            .iter()
            .map(|&id| self.fetch_pack_with_semaphore(id, semaphore.clone()))
            .collect();
        let results = futures::future::join_all(tasks).await;
        let modpacks: Vec<ModpackInfo> = results.into_iter().flatten().collect();

        {
            let mut cache = self.cache.lock().unwrap();
            cache.modpacks = modpacks.clone();
            cache.saved_at = now;
        }

        // 源：try { Directory.CreateDirectory(_cacheDir); WriteAllText } catch { }——错误全部吞掉
        if !self.cache_dir.is_empty() {
            let _ = std::fs::create_dir_all(&self.cache_dir);
        }
        let cache_data = CacheData {
            saved_at: now,
            modpacks: modpacks.clone(),
        };
        if let Ok(json) = serde_json::to_string(&cache_data) {
            let _ = std::fs::write(self.cache_file(), json);
        }

        Ok(modpacks)
    }

    /// 获取整合包版本 Mods 详情（源：`GetModDetailAsync`，错误吞掉 → null）。
    pub(crate) async fn get_mod_detail(&self, pack_id: i32, version_id: i32) -> Option<ModsDetail> {
        let json = self
            .get_data_async(&format!("/modpack/{pack_id}/{version_id}/mods"))
            .await
            .ok()?;
        serde_json::from_str(&json).ok()
    }
}

#[async_trait]
impl FtbSource for FtbBase {
    /// 搜索整合包（源：`SearchAsync`）。
    ///
    /// 基于全量拉取结果在内存中过滤，参数语义与源一致：
    /// - `query`：Name / Synopsis 子串匹配（源 `ToLower()` 忽略大小写）
    /// - `tags`：标签名忽略大小写匹配（源 `StringComparison.OrdinalIgnoreCase`）
    /// - `mc_version`：最新 release 版本（`get_latest_version`）的 `game` target 版本精确匹配
    /// - `loader`：最新 release 版本的 `modloader` target 名称忽略大小写匹配
    /// - `sort`：`featured`（默认：featured 优先 + plays 降序）/ `trending` / `name` / `plays` /
    ///   `downloads` / `released` / `updated`
    async fn search(
        &self,
        query: Option<&str>,
        tags: Option<&[String]>,
        mc_version: Option<&str>,
        loader: Option<&str>,
        sort: &str,
        limit: i32,
    ) -> Result<Vec<ModpackInfo>, Error> {
        let mut result = self.fetch_all_packs().await?;

        if let Some(q) = query.filter(|q| !q.is_empty()) {
            let q = q.to_lowercase();
            result.retain(|p| {
                p.name.to_lowercase().contains(&q)
                    || p.synopsis
                        .as_deref()
                        .map_or(false, |s| s.to_lowercase().contains(&q))
            });
        }

        if let Some(tags) = tags.filter(|t| !t.is_empty()) {
            result.retain(|p| {
                p.tags.as_ref().is_some_and(|pt| {
                    pt.iter().any(|t| tags.iter().any(|f| t.name.eq_ignore_ascii_case(f)))
                })
            });
        }

        if let Some(mc_version) = mc_version.filter(|v| !v.is_empty()) {
            result.retain(|p| {
                get_latest_version(p).is_some_and(|latest| {
                    latest.targets.is_some_and(|targets| {
                        targets.iter().any(|t| {
                            t.r#type.as_deref() == Some("game")
                                && t.version.as_deref() == Some(mc_version)
                        })
                    })
                })
            });
        }

        if let Some(loader) = loader.filter(|l| !l.is_empty()) {
            result.retain(|p| {
                get_latest_version(p).is_some_and(|latest| {
                    latest.targets.is_some_and(|targets| {
                        targets.iter().any(|t| {
                            t.r#type.as_deref() == Some("modloader")
                                && t.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(loader))
                        })
                    })
                })
            });
        }

        match sort {
            "trending" => result.sort_by_key(|p| std::cmp::Reverse(p.plays_14d)),
            "name" => result.sort_by(|a, b| a.name.cmp(&b.name)),
            "plays" => result.sort_by_key(|p| std::cmp::Reverse(p.plays)),
            "downloads" => result.sort_by_key(|p| std::cmp::Reverse(p.installs)),
            "released" => result.sort_by_key(|p| std::cmp::Reverse(p.released)),
            "updated" => result.sort_by_key(|p| std::cmp::Reverse(p.updated)),
            // 默认（featured）：OrderByDescending(Featured == true).ThenByDescending(Plays)
            _ => result.sort_by(|a, b| {
                (b.featured.is_some_and(|f| f), b.plays).cmp(&(a.featured.is_some_and(|f| f), a.plays))
            }),
        }

        result.truncate(limit.max(0) as usize);
        Ok(result)
    }

    /// 获取整合包详情（源：`GetPackDetailAsync`，错误吞掉 → null）。
    async fn get_pack_detail(&self, id: i32) -> Result<Option<ModpackInfo>, Error> {
        Ok(self.fetch_pack_inner(id).await)
    }

    /// 获取整合包指定版本详情（源：`GetVersionDetailAsync`，错误吞掉 → null）。
    async fn get_version_detail(
        &self,
        pack_id: i32,
        version_id: i32,
    ) -> Result<Option<VersionDetail>, Error> {
        Ok(self.get_version_detail_inner(pack_id, version_id).await)
    }

    /// 获取整合包指定版本的更新日志（源：`GetChangelogAsync`，错误吞掉 → null）。
    async fn get_changelog(
        &self,
        pack_id: i32,
        version_id: i32,
    ) -> Result<Option<ChangelogResult>, Error> {
        let json = match self
            .get_data_async(&format!("/modpack/{pack_id}/{version_id}/changelog"))
            .await
        {
            Ok(json) => json,
            Err(_) => return Ok(None),
        };
        Ok(serde_json::from_str(&json).ok())
    }
}

/// 数据目录解析（源：`GetDataDir` 静态方法）。
///
/// 优先级：`QOMICEX_HOME` 环境变量（非空）→ 默认目录（本地应用数据目录 + `qomicex-launcher`）
/// → `.qomicex-bootstrap` 文件内容（trim 后非空则使用）；均不满足时用默认目录。
fn get_data_dir() -> String {
    if let Ok(env) = std::env::var("QOMICEX_HOME") {
        if !env.is_empty() {
            return env;
        }
    }

    let default_dir = local_app_data_dir().join("qomicex-launcher");
    let bootstrap_file = default_dir.join(".qomicex-bootstrap");
    if let Ok(content) = std::fs::read_to_string(&bootstrap_file) {
        let custom = content.trim();
        if !custom.is_empty() {
            return custom.to_string();
        }
    }
    default_dir.to_string_lossy().into_owned()
}

/// 本地应用数据目录（源：`Environment.GetFolderPath(SpecialFolder.LocalApplicationData)`）。
///
/// Windows：`%LOCALAPPDATA%`；其他平台：`$XDG_DATA_HOME` 或 `$HOME/.local/share`；
/// 均不可用时回退当前目录。
/// ⚠️ UNMAPPED：macOS 应为 `~/Library/Application Support`，此处按 XDG 处理；
/// Android 无上述环境变量时回退当前目录，与 .NET 语义不完全一致。
fn local_app_data_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    if let Ok(dir) = std::env::var("LOCALAPPDATA") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }

    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return std::path::PathBuf::from(home).join(".local/share");
        }
    }
    std::path::PathBuf::from(".")
}

/// 当前 Unix 时间戳（秒）（源：`DateTimeOffset.UtcNow.ToUnixTimeSeconds()`）。
fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}





