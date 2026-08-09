//! InstallerFactory 实现：DefaultInstallerFactory（B9，P43）
//!
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/DefaultInstallerFactory.cs（42 行）
//!
//! 设计说明：
//! - 源 `internal sealed class DefaultInstallerFactory : IInstallerFactory` 无状态 →
//!   `pub(crate) struct DefaultInstallerFactory`（空结构体，自动 Send + Sync）；
//! - 12 个 create 方法全部同步（源无 Task）→ 本实现无需 `#[async_trait]`
//!   （InstallerFactory trait 本身亦无 async 方法，见 api/installer.rs）；
//! - C# 每次调用 `new` 返回新实例 → Rust 直接构造各安装器 struct 装箱返回
//!   （所有权转移 `Box<dyn Installer + Send + Sync>`，同 api/local.rs 决策）；
//! - 前 9 个方法分派到各安装器 struct；后 3 个（Modpacks 整合包）源安装器类位于
//!   Services/Installers/Modpacks/，B13 批次才移植 → 本批返回占位实现
//!   （`ModpackInstallerPlaceholder`，install 返回「整合包安装器将在后续版本提供」）。
//!
//! ⚠️ 并行契约：Liteloader/OptiFine/Cleanroom 安装器由并行子任务按约定写入
//! （liteloader/install.rs、optifine/install.rs、cleanroom/install.rs，构造签名同构：
//! LiteLoader/OptiFine 为 (i32, String, String)，Cleanroom 为 (i32, String)），
//! 本文件按约定签名引用，以实际落地为准。


use crate::api::installer::InstallerFactory;
use crate::models::download::DownloadMirror;
use crate::services::installers::babric::BabricInstaller;
use crate::services::installers::fabric::install::FabricInstaller;
use crate::services::installers::forge::install::ForgeInstaller;
use crate::services::installers::installer::Installer;
use crate::services::installers::legacy_fabric::LegacyFabricInstaller;
use crate::services::installers::neoforge::install::NeoForgeInstaller;
use crate::services::installers::quilt::install::QuiltInstaller;

/// 默认安装器工厂（源：`internal sealed class DefaultInstallerFactory : IInstallerFactory`）。
///
/// 无状态 → 空结构体；按类型创建具体安装器实例。
pub struct DefaultInstallerFactory;

impl InstallerFactory for DefaultInstallerFactory {
    /// 创建 Fabric 安装器（源：`CreateFabric(int downloadSource, string gameDir)`）
    fn create_fabric(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync> {
        Box::new(FabricInstaller::new(download_source, game_dir.to_string()))
    }

    /// 创建 Quilt 安装器（源：`CreateQuilt(int downloadSource, string gameDir)`）
    fn create_quilt(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync> {
        Box::new(QuiltInstaller::new(download_source, game_dir.to_string()))
    }

    /// 创建 Forge 安装器（源：`CreateForge(int downloadSource, string gameDir, string gameVersion)`）
    fn create_forge(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(ForgeInstaller::new(
            download_source,
            game_dir.to_string(),
            game_version.to_string(),
        ))
    }

    /// 创建 NeoForge 安装器（源：`CreateNeoForge(int downloadSource, string gameDir, string gameVersion)`）
    fn create_neoforge(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(NeoForgeInstaller::new(
            download_source,
            game_dir.to_string(),
            game_version.to_string(),
        ))
    }

    /// 创建 LiteLoader 安装器（源：`CreateLiteLoader(int downloadSource, string gameDir,
    /// string gameVersion)`；LiteloaderInstaller 由并行子任务按约定写入）
    fn create_liteloader(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(crate::services::installers::liteloader::install::LiteloaderInstaller::new(
            download_source,
            game_dir.to_string(),
            game_version.to_string(),
        ))
    }

    /// 创建 OptiFine 安装器（源：`CreateOptiFine(int downloadSource, string gameDir,
    /// string gameVersion)`；OptiFineInstaller 由并行子任务按约定写入）
    fn create_optifine(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(crate::services::installers::optifine::install::OptiFineInstaller::new(
            download_source,
            game_dir.to_string(),
            game_version.to_string(),
        ))
    }

    /// 创建 Cleanroom 安装器（源：`CreateCleanroom(int downloadSource, string gameDir)`；
    /// CleanroomInstaller 由并行子任务按约定写入）
    fn create_cleanroom(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync> {
        Box::new(crate::services::installers::cleanroom::CleanroomInstaller::new(
            download_source,
            game_dir.to_string(),
        ))
    }

    /// 创建 LegacyFabric 安装器（源：`CreateLegacyFabric(int downloadSource, string gameDir)`；
    /// 源构造参数被忽略，按映射表 downloadSource int → DownloadMirror 收参）
    fn create_legacy_fabric(
        &self,
        download_source: i32,
        game_dir: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(LegacyFabricInstaller::new(
            mirror_from_download_source(download_source),
            game_dir,
        ))
    }

    /// 创建 Babric 安装器（源：`CreateBabric(int downloadSource, string gameDir)`；
    /// 源构造参数被忽略，按映射表 downloadSource int → DownloadMirror 收参）
    fn create_babric(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync> {
        Box::new(BabricInstaller::new(
            mirror_from_download_source(download_source),
            game_dir,
        ))
    }

    /// 创建 CurseForge 整合包安装器（源：`CreateCurseForgeModpack(string gameDir, bool
    /// versionIsolation, string modpackFilePath)`）。
    ///
    /// ⚠️ 源安装器（Services/Installers/Modpacks/CurseForgeModpackInstaller.cs）未移植
    /// （B13 批次）→ 返回占位实现，见 `ModpackInstallerPlaceholder`。
    fn create_curseforge_modpack(
        &self,
        game_dir: &str,
        version_isolation: bool,
        modpack_file_path: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(crate::services::installers::modpacks::curseforge::CurseForgeModpackInstaller::new(
            game_dir,
            version_isolation,
            modpack_file_path,
        ))
    }

    /// 创建 Modrinth 整合包安装器（源：`CreateModrinthModpack(string gameDir, bool
    /// versionIsolation, string modpackFilePath)`）。
    ///
    /// ⚠️ 源安装器（Services/Installers/Modpacks/ModrinthModpackInstaller.cs）未移植
    /// （B13 批次）→ 返回占位实现，见 `ModpackInstallerPlaceholder`。
    fn create_modrinth_modpack(
        &self,
        game_dir: &str,
        version_isolation: bool,
        modpack_file_path: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(crate::services::installers::modpacks::modrinth::ModrinthModpackInstaller::new(
            game_dir,
            version_isolation,
            modpack_file_path,
        ))
    }

    /// 创建 FTB 整合包安装器（源：`CreateFtbModpack(string gameDir, bool versionIsolation,
    /// HttpClient httpClient, string cfApiKey)`；`HttpClient` → `reqwest::Client` 按值）。
    ///
    /// ⚠️ 源安装器（Services/Installers/Modpacks/FTBModpackInstaller.cs）未移植
    /// （B13 批次）→ 返回占位实现，见 `ModpackInstallerPlaceholder`。
    fn create_ftb_modpack(
        &self,
        game_dir: &str,
        version_isolation: bool,
        http_client: reqwest::Client,
        cf_api_key: &str,
    ) -> Box<dyn Installer + Send + Sync> {
        Box::new(crate::services::installers::modpacks::ftb::FtbModpackInstaller::new(
            game_dir,
            version_isolation,
            http_client,
            cf_api_key,
        ))
    }
}

/// 整合包安装器占位实现（B13 批次移植 Modpacks/ 三安装器后移除）。
/// 见各安装器文件头注释）。
fn mirror_from_download_source(download_source: i32) -> DownloadMirror {
    if download_source == 1 {
        DownloadMirror::Bmclapi
    } else {
        DownloadMirror::Official
    }
}




