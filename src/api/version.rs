//! 版本域公开 API traits（B3）：对应源项目 Public/ 下 4 个接口
//!
//! | 源接口 | 源文件 | Rust trait |
//! |--------|--------|------------|
//! | IVersionManagementService | Public/Services/IVersionManagementService.cs | `VersionManagement` |
//! | IVersionManifestService | Public/Services/IVersionManifestService.cs | `VersionManifest` |
//! | IVersionLocator | Public/Core/IVersionLocator.cs | `VersionLocator` |
//! | IResourceCompleter | Public/Core/IResourceCompleter.cs | `ResourceCompleter` |
//!
//! 方法映射规则（见翻译日志 p16-api-version.md）：
//! - `Task<T>` → `async fn ... -> Result<T, crate::error::Error>`
//! - `Task<T?>` → `Result<Option<T>, Error>`；`Task<List<T>>` → `Result<Vec<T>, Error>`
//! - `Task`（无返回值）→ `Result<(), Error>`
//! - 同步方法（GetAllVersions / IsVersionInstalled / RefreshCache / GetVersionPath
//!   / GetInstalledVersions / GetVersionMetadata）→ 普通方法
//! - `IProgress<DownloadProgress>?` → `Option<&dyn ProgressReporter>`（见 src/event.rs）
//! - C# 重载方法（meta 与 jsonData 两个变体）→ `_from_json` 后缀区分（见日志重命名决策）

use crate::error::Error;
use crate::event::ProgressReporter;
use crate::models::installer::MissFileInfo;
use crate::models::local::LocalVersionInfo;
use crate::models::version_manifest::{LatestVersionInfo, ManifestVersionInfo, VersionManifestRoot};
use crate::models::version_metadata::CompleteVersionMetadata;

/// 版本管理服务（源：IVersionManagementService，版本安装/卸载/查询总入口）
pub trait VersionManagement: Send + Sync {
    /// 获取版本清单（对应 GetManifestAsync；C# 参数 forceRefresh 默认 false，
    /// Rust 无默认参数，调用方需显式传入）
    async fn get_manifest(&self, force_refresh: bool) -> Result<VersionManifestRoot, Error>;

    /// 获取可用版本列表（对应 GetAvailableVersionsAsync；参数同上）
    async fn get_available_versions(
        &self,
        force_refresh: bool,
    ) -> Result<Vec<ManifestVersionInfo>, Error>;

    /// 获取最新版本信息（对应 GetLatestVersionsAsync；参数同上）
    async fn get_latest_versions(&self, force_refresh: bool) -> Result<LatestVersionInfo, Error>;

    /// 获取指定版本的完整元数据（对应 GetVersionMetadataAsync(string versionId)）
    async fn get_version_metadata(&self, version_id: &str)
        -> Result<CompleteVersionMetadata, Error>;

    /// 判断指定版本是否已安装（对应 IsVersionInstalled，同步方法）
    fn is_version_installed(&self, version_id: &str) -> bool;

    /// 安装指定版本（对应 InstallVersionAsync；
    /// C# `IProgress<DownloadProgress>? progress = null` → `Option<&dyn ProgressReporter>`）
    async fn install_version(
        &self,
        version_id: &str,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error>;

    /// 卸载指定版本（对应 UninstallVersionAsync）
    async fn uninstall_version(&self, version_id: &str) -> Result<(), Error>;

    /// 获取已安装版本列表（对应 GetInstalledVersions，同步方法）
    fn get_installed_versions(&self) -> Vec<LocalVersionInfo>;
}

/// 版本清单服务（源：IVersionManifestService，清单下载/元数据获取）
pub trait VersionManifest: Send + Sync {
    /// 获取版本清单（对应 GetVersionManifestAsync）
    async fn get_version_manifest(&self) -> Result<VersionManifestRoot, Error>;

    /// 从指定 URL 获取版本元数据（对应 GetVersionMetadataAsync(string url)。
    /// 注意与 VersionManagement::get_version_metadata 参数语义不同：
    /// 此处是版本 JSON 下载地址 url，非版本 ID）
    async fn get_version_metadata(&self, url: &str) -> Result<CompleteVersionMetadata, Error>;
}

/// 版本定位器（源：IVersionLocator，本地版本目录/缺失文件扫描）
pub trait VersionLocator: Send + Sync {
    /// 获取全部本地版本（对应 GetAllVersions，同步方法）
    fn get_all_versions(&self) -> Vec<LocalVersionInfo>;

    /// 获取本地版本元数据（对应 GetVersionMetadata(string versionId)，同步方法；
    /// C# 返回 `CompleteVersionMetadata?` → `Option<CompleteVersionMetadata>`）
    fn get_version_metadata(&self, version_id: &str) -> Option<CompleteVersionMetadata>;

    /// 判断指定版本是否已安装（对应 IsVersionInstalled，同步方法）
    fn is_version_installed(&self, version_id: &str) -> bool;

    /// 刷新本地缓存（对应 RefreshCache，同步方法）
    fn refresh_cache(&self);

    /// 获取版本目录路径（对应 GetVersionPath，同步方法）
    fn get_version_path(&self, version_id: &str) -> String;

    /// 获取缺失文件列表（对应 GetMissFilesAsync(CompleteVersionMetadata meta) 重载，
    /// 传入已解析的元数据对象）
    async fn get_miss_files(&self, meta: &CompleteVersionMetadata) -> Result<Vec<MissFileInfo>, Error>;

    /// 获取缺失文件列表（对应 GetMissFilesAsync(string jsonData) 重载，
    /// 传入元数据 JSON 字符串；C# 重载无法直接映射 → 重命名为 `_from_json`，
    /// 重命名决策见翻译日志）
    async fn get_miss_files_from_json(&self, json_data: &str) -> Result<Vec<MissFileInfo>, Error>;

    /// 获取缺失库文件列表（对应 GetMissLibrariesAsync(CompleteVersionMetadata meta) 重载）
    async fn get_miss_libraries(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Vec<MissFileInfo>, Error>;

    /// 获取缺失库文件列表（对应 GetMissLibrariesAsync(string jsonData) 重载，
    /// 重命名规则同 `get_miss_files_from_json`）
    async fn get_miss_libraries_from_json(&self, json_data: &str)
        -> Result<Vec<MissFileInfo>, Error>;

    /// 获取缺失主 Jar 文件（对应 GetMissMainJarAsync(CompleteVersionMetadata meta) 重载；
    /// C# 返回 `MissFileInfo?` → `Option<MissFileInfo>`）
    async fn get_miss_main_jar(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Option<MissFileInfo>, Error>;

    /// 获取缺失主 Jar 文件（对应 GetMissMainJarAsync(string jsonData) 重载，
    /// 重命名规则同 `get_miss_files_from_json`）
    async fn get_miss_main_jar_from_json(&self, json_data: &str)
        -> Result<Option<MissFileInfo>, Error>;

    /// 获取缺失资源文件列表（对应 GetMissAssetsAsync(CompleteVersionMetadata meta) 重载）
    async fn get_miss_assets(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Vec<MissFileInfo>, Error>;

    /// 获取缺失资源文件列表（对应 GetMissAssetsAsync(string jsonData) 重载，
    /// 重命名规则同 `get_miss_files_from_json`）
    async fn get_miss_assets_from_json(&self, json_data: &str) -> Result<Vec<MissFileInfo>, Error>;
}

/// 资源补全器（源：IResourceCompleter，补齐缺失资源并校验完整性）
pub trait ResourceCompleter: Send + Sync {
    /// 补全缺失资源（对应 CompleteResourcesAsync；
    /// `IProgress<DownloadProgress>? progress = null` → `Option<&dyn ProgressReporter>`）
    async fn complete_resources(
        &self,
        metadata: &CompleteVersionMetadata,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error>;

    /// 检查资源是否完整（对应 CheckResourcesCompleteAsync）
    async fn check_resources_complete(&self, metadata: &CompleteVersionMetadata)
        -> Result<bool, Error>;
}
