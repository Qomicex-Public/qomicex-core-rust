//! 版本管理编排服务（B6，对应源：Services/VersionManagementService.cs）
//!
//! 版本域总入口实现：清单获取（含磁盘缓存）、可用/最新版本查询、
//! 元数据获取（本地优先）、安装/卸载编排。同时内嵌移植源
//! Services/VersionManifestCache.cs（磁盘缓存，见 manifest.rs 头注释约定：
//! 缓存由本服务持有，manifest 服务不实现缓存）。
//!
//! 依赖注入决策（详见翻译日志 p29）：
//! - `manifest_service`：具体类型 `VersionManifestService`（源字段类型为具体类）
//! - `version_locator` / `resource_completer`：trait 对象（源字段类型为接口
//!   IVersionLocator / IResourceCompleter），构造时内部 new 默认实现
//! - `download_source_manager`：`Arc<dyn DownloadSourceManager>`（源可选参数注入，
//!   null → DefaultDownloadSourceManager()；Arc 与 completer 共享）
//! - 构造仅注入 httpClient / downloadSourceManager（源构造签名），其余内部 new
//!
//! 错误映射（本文件定案）：
//! - JSON 序列化失败（源 JsonException）→ Error::Http（沿 manifest.rs 先例）
//! - 文件 IO / 目录创建（源 IOException）→ Error::DownloadFailed（沿 completer.rs 先例）
//! - 版本不存在（源 VersionNotFoundException）→ Error::VersionNotFound
//! - 元数据 URL 无效（源 VersionMetadataException）→ Error::VersionMetadata

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use crate::api::download::DownloadSourceManager;
use crate::api::version::{ResourceCompleter, VersionLocator, VersionManagement, VersionManifest};
use crate::error::Error;
use crate::event::ProgressReporter;
use crate::models::download::DownloadMirror;
use crate::models::local::LocalVersionInfo;
use crate::models::version_manifest::{LatestVersionInfo, ManifestVersionInfo, VersionManifestRoot};
use crate::models::version_metadata::CompleteVersionMetadata;
use crate::net::NetworkConfig;
use crate::services::download::mirror::DefaultDownloadSourceManager;
use crate::services::version::completer::DefaultResourceCompleter;
use crate::services::version::locator::DefaultVersionLocator;
use crate::services::version::manifest::VersionManifestService;
use crate::util::json_helper::{
    deserialize_version_manifest, serialize_version_manifest, serialize_version_metadata,
};

/// 版本清单磁盘缓存（源：`internal class VersionManifestCache`，内嵌移植）。
/// 缓存路径 `{gameRoot}/cache/version_manifest.json`，有效期 5 分钟。
struct VersionManifestCache {
    /// 缓存文件路径（源：`_cacheFilePath`）
    cache_file_path: String,
    /// 缓存有效期（TD-8：构造参数传入，源默认 TimeSpan.FromMinutes(5)）
    cache_duration: Duration,
}

impl VersionManifestCache {
    /// 缓存有效期由构造参数传入（TD-8：源 TimeSpan.FromMinutes(5) 默认）

    /// 构造缓存（源：构造函数）。
    /// ⚠️ UNMAPPED：源同步 `Directory.CreateDirectory` 失败抛 IOException；
    /// Rust 侧 new() 返回 Self（无错误通道），创建失败静默忽略，
    /// 错误推迟到 save_to_cache 上报（见翻译日志 p29）。
    fn new(cache_file_path: String, cache_duration: Duration) -> Self {
        if let Some(directory) = Path::new(&cache_file_path).parent() {
            if !directory.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(directory);
            }
        }
        Self { cache_file_path, cache_duration }
    }

    /// 缓存是否有效（源：HasValidCache）。
    /// 文件存在 且 `DateTime.Now - LastWriteTime < 5分钟`。
    /// 时钟倒退边界逐条保留：源差值为负 → 恒 < 5 分钟 → 缓存有效；
    /// Rust `duration_since` 返回 Err → 等义返回 true（不修正）。
    fn has_valid_cache(&self) -> bool {
        let Ok(metadata) = std::fs::metadata(&self.cache_file_path) else {
            return false;
        };
        let Ok(last_write_time) = metadata.modified() else {
            return false;
        };
        match SystemTime::now().duration_since(last_write_time) {
            Ok(age) => age < self.cache_duration,
            Err(_) => true,
        }
    }

    /// 从缓存加载清单（源：LoadFromCacheAsync）。
    /// 文件不存在 → None；读/解析任何异常 → None（源 catch 全吞 → null）。
    async fn load_from_cache(&self) -> Option<VersionManifestRoot> {
        if !Path::new(&self.cache_file_path).is_file() {
            return None;
        }
        let json = tokio::fs::read_to_string(&self.cache_file_path).await.ok()?;
        deserialize_version_manifest(&json).ok().flatten()
    }

    /// 保存清单到缓存（源：SaveToCacheAsync）。
    /// 序列化失败（源 JsonException）→ Error::Http；写文件失败（源 IOException）→
    /// Error::DownloadFailed（先例见 manifest.rs / completer.rs）。
    async fn save_to_cache(&self, manifest: &VersionManifestRoot) -> Result<(), Error> {
        let json = serialize_version_manifest(manifest).map_err(|e| Error::Http {
            message: "序列化版本清单失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;
        tokio::fs::write(&self.cache_file_path, json)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("写入版本清单缓存失败: {}", self.cache_file_path),
                source: Some(Box::new(e)),
            })
    }

    /// 使缓存失效（源：InvalidateCache；文件存在则删除）。
    /// `#[allow(dead_code)]`：源 VersionManagementService 从未调用（源类完整 API 保留）。
    #[allow(dead_code)]
    fn invalidate_cache(&self) {
        let _ = std::fs::remove_file(&self.cache_file_path);
    }
}

/// 版本管理服务（源：`internal sealed class VersionManagementService : IVersionManagementService`）。
/// 清单/可用版本/最新版本/元数据/安装/卸载/已装列表的编排总入口。
pub(crate) struct VersionManagementService {
    /// 游戏根目录（源：`_gameRootPath`）
    game_root_path: String,
    /// 版本清单服务（源：`_manifestService`，具体类型，构造内部 new）
    manifest_service: VersionManifestService,
    /// 本地版本定位器（源：`_versionLocator` IVersionLocator，接口 → trait 对象）
    version_locator: Box<dyn VersionLocator + Send + Sync>,
    /// 下载源管理器（源：`_downloadSourceManager` IDownloadSourceManager；
    /// 仅用于构造 completer，源同逻辑 → allow(dead_code)）
    #[allow(dead_code)]
    download_source_manager: Arc<dyn DownloadSourceManager + Send + Sync>,
    /// 资源补全器（源：`_resourceCompleter` IResourceCompleter，接口 → trait 对象）
    resource_completer: Box<dyn ResourceCompleter + Send + Sync>,
    /// 版本清单磁盘缓存（源：`_cache`）
    cache: VersionManifestCache,
}

impl VersionManagementService {
    /// 构造版本管理服务（源：`VersionManagementService(string gameRootPath,
    /// HttpClient? httpClient = null, IDownloadSourceManager? downloadSourceManager = null)`）。
    /// Rust 无默认参数：httpClient / downloadSourceManager 不注入时传 None；
    /// manifest/locator/completer/cache 均内部 new（源同逻辑，仅上述两项可注入）。
    pub(crate) fn new(
        game_root_path: String,
        http_client: Option<reqwest::Client>,
        download_source_manager: Option<Arc<dyn DownloadSourceManager + Send + Sync>>,
        cache_expiry: Duration,
        max_concurrent_downloads: usize,
    ) -> Self {
        // 源：httpClient ?? new HttpClient()
        // （内部自建时应用全局 proxy/TLS 配置）
        let http = http_client.unwrap_or_else(|| {
            NetworkConfig::global()
                .apply(reqwest::Client::builder())
                .build()
                .expect("构建版本管理 HTTP 客户端失败")
        });

        // 源：new VersionManifestService(httpClient)
        let manifest_service = VersionManifestService::new(http);

        // 源：new DefaultVersionLocator(gameRootPath)（mirror 默认 Official、
        // httpClient 默认 null → 内部自建）
        let version_locator: Box<dyn VersionLocator + Send + Sync> =
            Box::new(DefaultVersionLocator::new(game_root_path.clone(), DownloadMirror::Official));

        // 源：downloadSourceManager ?? new DefaultDownloadSourceManager()（Default 构造 → BMCLAPI 首选）
        let download_source_manager =
            download_source_manager.unwrap_or_else(|| Arc::new(DefaultDownloadSourceManager::default()));

        // 源：new DefaultResourceCompleter(gameRootPath, _downloadSourceManager)
        // （maxConcurrentDownloads 默认 8，Rust 显式传入）
        let resource_completer: Box<dyn ResourceCompleter + Send + Sync> =
            Box::new(DefaultResourceCompleter::new(
                game_root_path.clone(),
                download_source_manager.clone(),
                max_concurrent_downloads,
            ));

        // 源：new VersionManifestCache(Path.Combine(gameRootPath, "cache", "version_manifest.json"))
        let cache_path = Path::new(&game_root_path)
            .join("cache")
            .join("version_manifest.json")
            .to_string_lossy()
            .into_owned();
        let cache = VersionManifestCache::new(cache_path, cache_expiry);

        Self {
            game_root_path,
            manifest_service,
            version_locator,
            download_source_manager,
            resource_completer,
            cache,
        }
    }

    /// 将版本元数据保存到本地（源：SaveVersionMetadataToLocal，私有）。
    /// 建 `versions/{versionId}` 目录并写入 `{versionId}.json`。
    async fn save_version_metadata_to_local(
        &self,
        version_id: &str,
        metadata: &CompleteVersionMetadata,
    ) -> Result<(), Error> {
        let version_path = Path::new(&self.game_root_path)
            .join("versions")
            .join(version_id);
        tokio::fs::create_dir_all(&version_path)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("创建版本目录失败: {}", version_path.display()),
                source: Some(Box::new(e)),
            })?;

        let json_path = version_path.join(format!("{version_id}.json"));
        let json_content = serialize_version_metadata(metadata).map_err(|e| Error::Http {
            message: format!("序列化版本元数据失败: {version_id}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        tokio::fs::write(&json_path, json_content)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("写入版本JSON失败: {}", json_path.display()),
                source: Some(Box::new(e)),
            })
    }
}

#[async_trait]
impl VersionManagement for VersionManagementService {
    /// 获取版本清单（源：GetManifestAsync，优先使用缓存）。
    /// 非强制刷新且缓存有效 → 读缓存；否则网络获取并覆写缓存。
    /// forceRefresh=true 跳过缓存直接拉取（源逻辑逐条保留）。
    async fn get_manifest(&self, force_refresh: bool) -> Result<VersionManifestRoot, Error> {
        // 源：如果不是强制刷新，尝试从缓存加载
        if !force_refresh && self.cache.has_valid_cache() {
            if let Some(cached) = self.cache.load_from_cache().await {
                return Ok(cached);
            }
        }

        // 源：缓存无效或不存在，从网络获取
        let manifest = self.manifest_service.get_version_manifest().await?;
        self.cache.save_to_cache(&manifest).await?;
        Ok(manifest)
    }

    /// 获取所有可用版本列表（源：GetAvailableVersionsAsync）
    async fn get_available_versions(
        &self,
        force_refresh: bool,
    ) -> Result<Vec<ManifestVersionInfo>, Error> {
        Ok(self.get_manifest(force_refresh).await?.versions)
    }

    /// 获取最新版本信息（源：GetLatestVersionsAsync）
    async fn get_latest_versions(&self, force_refresh: bool) -> Result<LatestVersionInfo, Error> {
        Ok(self.get_manifest(force_refresh).await?.latest)
    }

    /// 获取特定版本的完整元数据（源：GetVersionMetadataAsync(string versionId)）。
    /// 1. 本地已安装版本优先；2. 清单中查找（不存在 → VersionNotFound，
    /// URL 无效 → VersionMetadata）；3. 下载并保存到本地。
    async fn get_version_metadata(&self, version_id: &str)
        -> Result<CompleteVersionMetadata, Error> {
        // 源：先从本地已安装的版本中读取
        if let Some(local_metadata) = self.version_locator.get_version_metadata(version_id) {
            return Ok(local_metadata);
        }

        // 源：从网络获取（GetManifestAsync() 默认 forceRefresh=false）
        let manifest = self.get_manifest(false).await?;
        let version_info = manifest.versions.iter().find(|v| v.id == version_id);

        let Some(version_info) = version_info else {
            return Err(Error::VersionNotFound {
                message: format!("版本 {version_id} 在官方清单中不存在"),
                source: None,
            });
        };

        if version_info.url.is_empty() {
            return Err(Error::VersionMetadata {
                message: format!("版本 {version_id} 的元数据URL无效"),
                source: None,
            });
        }

        // 源：下载并保存到本地
        let metadata = self
            .manifest_service
            .get_version_metadata(&version_info.url)
            .await?;
        self.save_version_metadata_to_local(version_id, &metadata)
            .await?;

        Ok(metadata)
    }

    /// 检查版本是否已安装（源：IsVersionInstalled，同步）
    fn is_version_installed(&self, version_id: &str) -> bool {
        self.version_locator.is_version_installed(version_id)
    }

    /// 安装指定版本（源：InstallVersionAsync）。
    /// 1. 获取元数据；2. 处理继承（先装父版本，progress 原样转发）；
    /// 3. 创建版本目录 + 写 {id}.json；4. 资源补全（progress 转发）；
    /// 5. 刷新定位器缓存。
    async fn install_version(
        &self,
        version_id: &str,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        // 源：获取版本元数据
        let metadata = self.get_version_metadata(version_id).await?;

        // 源：检查是否需要处理版本继承（InheritsFrom 非空 → 递归安装父版本）
        if let Some(parent) = metadata.inherits_from.as_deref().filter(|s| !s.is_empty()) {
            self.install_version(parent, progress).await?;
        }

        // 源：创建版本目录 + 保存版本JSON文件（与 SaveVersionMetadataToLocal 体一致，
        // 合并复用私有 helper，行为等价，见日志 p29）
        self.save_version_metadata_to_local(version_id, &metadata)
            .await?;

        // 源：补全资源（使用资源补全器）
        self.resource_completer
            .complete_resources(&metadata, progress)
            .await?;

        // 源：刷新版本定位器缓存
        self.version_locator.refresh_cache();
        Ok(())
    }

    /// 卸载指定版本（源：UninstallVersionAsync）。
    /// 版本目录存在 → 递归删除；随后刷新定位器缓存。
    async fn uninstall_version(&self, version_id: &str) -> Result<(), Error> {
        let version_path = self.version_locator.get_version_path(version_id);
        if Path::new(&version_path).is_dir() {
            // 源：await Task.Run(() => Directory.Delete(versionPath, true))
            tokio::fs::remove_dir_all(&version_path)
                .await
                .map_err(|e| Error::DownloadFailed {
                    message: format!("删除版本目录失败: {version_path}"),
                    source: Some(Box::new(e)),
                })?;
        }
        self.version_locator.refresh_cache();
        Ok(())
    }

    /// 获取已安装的版本列表（源：GetInstalledVersions，同步）
    fn get_installed_versions(&self) -> Vec<LocalVersionInfo> {
        self.version_locator.get_all_versions()
    }
}






