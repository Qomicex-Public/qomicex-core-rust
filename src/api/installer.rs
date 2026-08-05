//! InstallerProvider trait：可用模组加载器查询（B3）
//!
//! 对应源文件：Public/Services/IInstallerProvider.cs（Qomicex.Core.AOT.Public.Services）
//!
//! 方法映射表：
//! - `Task<List<ModLoaderResult>> GetAvailableModLoaders(string gameVersion, ModLoaderType type = ModLoaderType.All)` → `get_available_mod_loaders(&self, game_version: &str, r#type: ModLoaderType) -> Result<Vec<ModLoaderResult>, Error>`
//!
//! B9 追加：InstallerFactory trait（源 IInstallerFactory.cs，12 种安装器创建，
//! 同步方法返回 Box<dyn Installer + Send + Sync>，详见下方定义）。

use async_trait::async_trait;
use crate::error::Error;
use crate::models::installer::{ModLoaderResult, ModLoaderType};
use crate::services::installers::installer::Installer;

/// 安装器提供商（源：IInstallerProvider 接口）。
///
/// 负责查询指定游戏版本可用的模组加载器列表。
#[async_trait]
pub trait InstallerProvider: Send + Sync {
    /// 获取指定游戏版本可用的模组加载器列表（源：`GetAvailableModLoaders`）。
    ///
    /// C# 参数 `type` 有默认值 `ModLoaderType.All`，Rust 无默认参数，调用方需显式
    /// 传入；`type` 为 Rust 关键字，参数命名 `r#type`（与 ModLoaderResult 模型
    /// 字段命名一致）。
    async fn get_available_mod_loaders(
        &self,
        game_version: &str,
        r#type: ModLoaderType,
    ) -> Result<Vec<ModLoaderResult>, Error>;
}

/// 安装器工厂（源：`IInstallerFactory` 接口，Services/Installers/IInstallerFactory.cs）。
///
/// 按类型创建具体安装器实例。C# 工厂每次调用 `new` 返回新实例 → Rust
/// 所有权转移 `Box<dyn Installer + Send + Sync>`（同 LocalResourcesFactory 决策，
/// 见 api/local.rs 文件头注释）。
///
/// 方法映射表（12 个 create 方法，全部同步，对应源无 Task 的同步工厂方法）：
/// - `CreateFabric(int downloadSource, string gameDir)` → `create_fabric(download_source, game_dir)`
/// - `CreateQuilt(...)` → `create_quilt(...)`（参数同 CreateFabric）
/// - `CreateForge(int downloadSource, string gameDir, string gameVersion)` → `create_forge(download_source, game_dir, game_version)`
/// - `CreateNeoForge / CreateLiteLoader / CreateOptiFine(...)` → 同 Forge 参数形态
/// - `CreateCleanroom / CreateLegacyFabric / CreateBabric(int, string)` → 同 Fabric 参数形态
/// - `CreateCurseForgeModpack(string gameDir, bool versionIsolation, string modpackFilePath)` → `create_curseforge_modpack(game_dir, version_isolation, modpack_file_path)`
/// - `CreateModrinthModpack(...)` → 同 CurseForgeModpack 参数形态
/// - `CreateFtbModpack(string gameDir, bool versionIsolation, HttpClient httpClient, string cfApiKey)` → `create_ftb_modpack(game_dir, version_isolation, http_client, cf_api_key)`
///   （`HttpClient` → `reqwest::Client`（MAPPING_TABLE runtime 映射），按值传递，
///   reqwest Client 为 Arc 包装的轻量克隆，语义对应"将客户端交给安装器持有"）
///
/// ⚠️ 可见性决策：`Installer` trait 在 services/installers/installer.rs 中为
/// `pub(crate)`（服务实现层不对外）；若本 trait 声明为 `pub`，其方法签名中的
/// `dyn Installer` 将触发 Rust 2024 `private_interfaces`（deny-by-default）编译错误。
/// 故本 trait 同步为 `pub(crate)`，对外导出待后续批次与 Installer 可见性一并调整
/// （见 b9-logs/p35-installer-base.md 决策 D1；对比 api/local.rs 的占位 trait 因
/// 声明在同文件而采用 `pub` 的先例）。
pub trait InstallerFactory: Send + Sync {
    /// 创建 Fabric 安装器（源：`CreateFabric(int downloadSource, string gameDir)`）
    fn create_fabric(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync>;

    /// 创建 Quilt 安装器（源：`CreateQuilt(int downloadSource, string gameDir)`）
    fn create_quilt(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync>;

    /// 创建 Forge 安装器（源：`CreateForge(int downloadSource, string gameDir, string gameVersion)`）
    fn create_forge(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 NeoForge 安装器（源：`CreateNeoForge(int downloadSource, string gameDir, string gameVersion)`）
    fn create_neoforge(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 LiteLoader 安装器（源：`CreateLiteLoader(int downloadSource, string gameDir, string gameVersion)`）
    fn create_liteloader(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 OptiFine 安装器（源：`CreateOptiFine(int downloadSource, string gameDir, string gameVersion)`）
    fn create_optifine(
        &self,
        download_source: i32,
        game_dir: &str,
        game_version: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 Cleanroom 安装器（源：`CreateCleanroom(int downloadSource, string gameDir)`）
    fn create_cleanroom(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync>;

    /// 创建 LegacyFabric 安装器（源：`CreateLegacyFabric(int downloadSource, string gameDir)`）
    fn create_legacy_fabric(
        &self,
        download_source: i32,
        game_dir: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 Babric 安装器（源：`CreateBabric(int downloadSource, string gameDir)`）
    fn create_babric(&self, download_source: i32, game_dir: &str) -> Box<dyn Installer + Send + Sync>;

    /// 创建 CurseForge 整合包安装器（源：`CreateCurseForgeModpack(string gameDir, bool versionIsolation, string modpackFilePath)`）
    fn create_curseforge_modpack(
        &self,
        game_dir: &str,
        version_isolation: bool,
        modpack_file_path: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 Modrinth 整合包安装器（源：`CreateModrinthModpack(string gameDir, bool versionIsolation, string modpackFilePath)`）
    fn create_modrinth_modpack(
        &self,
        game_dir: &str,
        version_isolation: bool,
        modpack_file_path: &str,
    ) -> Box<dyn Installer + Send + Sync>;

    /// 创建 FTB 整合包安装器（源：`CreateFtbModpack(string gameDir, bool versionIsolation,
    /// HttpClient httpClient, string cfApiKey)`；`HttpClient` → `reqwest::Client` 按值）
    fn create_ftb_modpack(
        &self,
        game_dir: &str,
        version_isolation: bool,
        http_client: reqwest::Client,
        cf_api_key: &str,
    ) -> Box<dyn Installer + Send + Sync>;
}


