//! GameCore 门面（Facade）：聚合全部核心服务，`Arc<dyn Trait>` 依赖注入（B4）
//!
//! 对应源文件：Core/DefaultGameCore.cs（Qomicex.Core.AOT）
//!
//! 映射说明：
//! - `internal DefaultGameCore(...)` 构造 → `pub fn new(...)`（全参数，Rust 无默认参数；
//!   默认值由 builder（src/builder.rs）侧承载）
//! - C# 属性（Version/Auth/...）→ 只读字段 + 访问器方法（Rust 惯例）
//! - `IDisposable` → 无对应（Rust 所有权 + Drop 自动释放，无需 Dispose）
//!
//! 待补清单（后续批次）：
//! - HttpClient 字段：本批不引入（B13 网络层定 reqwest 共享 client，届时统一注入）
//! - `create_modrinth_source` / `create_curseforge_source` / `create_ftb_source`：
//!   源实现依赖 Services/Expansion 具体类（B13 批次）→ 本批不实现
//!   （CreateModrinthSource 等 3 个工厂方法待扩展平台实现批次补充）

use std::sync::Arc;

use crate::api::auth::AuthProvider;
use crate::api::download::DownloadSourceManager;
use crate::api::installer::{InstallerFactory, InstallerProvider};
use crate::api::java::JavaProvider;
use crate::api::launch::LaunchExecutor;
use crate::api::local::LocalResourcesFactory;
use crate::api::options::OptionsProvider;
use crate::api::server::ServerManager;
use crate::api::version::{VersionLocator, VersionManagement};

/// 游戏核心门面（源：DefaultGameCore，sealed class）。
///
/// 聚合认证、版本管理、启动、Java 检测、安装器、资源定位、下载源、
/// 本地内容等全部核心服务，作为依赖注入的聚合根（aggregate root）。
/// 各服务以 `Arc<dyn Trait>` 注入，便于跨线程共享与解耦。
pub struct GameCore {
    /// 版本管理服务（源：Version 属性，IVersionManagementService）
    version: Arc<dyn VersionManagement + Send + Sync>,
    /// 认证提供方（源：Auth 属性，IAuthProvider）
    auth: Arc<dyn AuthProvider + Send + Sync>,
    /// 启动执行器（源：Launch 属性，ILaunchExecutor）
    launch: Arc<dyn LaunchExecutor + Send + Sync>,
    /// Java 提供方（源：JavaProvider 属性，IJavaProvider）
    java_provider: Arc<dyn JavaProvider + Send + Sync>,
    /// 安装器提供方（源：InstallerProvider 属性，IInstallerProvider）
    installer_provider: Arc<dyn InstallerProvider + Send + Sync>,
    /// 安装器工厂（源：Installer 属性，IInstallerFactory）
    installer_factory: Arc<dyn InstallerFactory + Send + Sync>,
    /// 版本定位器（源：Locator 属性，IVersionLocator）
    locator: Arc<dyn VersionLocator + Send + Sync>,
    /// 游戏选项提供方（源：Options 属性，IOptionsProvider?，可空）
    options: Option<Arc<dyn OptionsProvider + Send + Sync>>,
    /// 服务器管理器（源：ServerManager 属性，IServerManager?，可空）
    server: Option<Arc<dyn ServerManager + Send + Sync>>,
    /// 下载源管理器（源：DownloadManager 属性，IDownloadSourceManager；
    /// C# 构造参数可空但以 `!` 强制非空赋值 → Rust 侧为必填参数）
    download_manager: Arc<dyn DownloadSourceManager + Send + Sync>,
    /// 本地内容资源工厂（源：LocalResourceProvider 属性，ILocalResourcesFactory；
    /// C# 构造参数可空但以 `!` 强制非空赋值 → Rust 侧为必填参数）
    local_resource_provider: Arc<dyn LocalResourcesFactory + Send + Sync>,
    /// 游戏根目录（源：GameRoot 属性，string）
    game_root: String,
}

impl GameCore {
    /// 构造游戏核心门面（源：`internal DefaultGameCore(...)`）。
    ///
    /// 全参数构造，无默认值（C# 默认参数由调用方/builder 显式提供）：
    /// - `options` / `server`：C# 可空参数 `IOptionsProvider?` / `IServerManager?` → `Option`
    /// - `download_manager` / `local_resource_provider`：C# 可空参数但以 `!` 强制非空
    ///   赋值 → 必填
    /// - `http`（HttpClient）：本批不引入（B13 网络层定 reqwest 共享 client）
    /// - `installer_factory`（IInstallerFactory）：必填（源构造参数，见文件头）
    pub fn new(
        version: Arc<dyn VersionManagement + Send + Sync>,
        auth: Arc<dyn AuthProvider + Send + Sync>,
        launch: Arc<dyn LaunchExecutor + Send + Sync>,
        java_provider: Arc<dyn JavaProvider + Send + Sync>,
        installer_provider: Arc<dyn InstallerProvider + Send + Sync>,
        installer_factory: Arc<dyn InstallerFactory + Send + Sync>,
        locator: Arc<dyn VersionLocator + Send + Sync>,
        game_root: String,
        options: Option<Arc<dyn OptionsProvider + Send + Sync>>,
        server: Option<Arc<dyn ServerManager + Send + Sync>>,
        download_manager: Arc<dyn DownloadSourceManager + Send + Sync>,
        local_resource_provider: Arc<dyn LocalResourcesFactory + Send + Sync>,
    ) -> Self {
        Self {
            version,
            auth,
            launch,
            java_provider,
            installer_provider,
            installer_factory,
            locator,
            options,
            server,
            download_manager,
            local_resource_provider,
            game_root,
        }
    }

    /// 版本管理服务（源：`Version` 属性）
    pub fn version(&self) -> &dyn VersionManagement {
        self.version.as_ref()
    }

    /// 认证提供方（源：`Auth` 属性）
    pub fn auth(&self) -> &dyn AuthProvider {
        self.auth.as_ref()
    }

    /// 启动执行器（源：`Launch` 属性）
    pub fn launch(&self) -> &dyn LaunchExecutor {
        self.launch.as_ref()
    }

    /// Java 提供方（源：`JavaProvider` 属性）
    pub fn java_provider(&self) -> &dyn JavaProvider {
        self.java_provider.as_ref()
    }

    /// 安装器提供方（源：`InstallerProvider` 属性）
    pub fn installer_provider(&self) -> &dyn InstallerProvider {
        self.installer_provider.as_ref()
    }

    /// 安装器工厂（源：`Installer` 属性，IInstallerFactory）
    ///
    /// ⚠️ 可见性：`InstallerFactory` 为 `pub(crate)`（p35 日志 D1：其方法签名返回
    /// `Box<dyn Installer + Send + Sync>`，`Installer` 为 pub(crate)，trait 无法 pub；
    /// 对外导出待 Installer 可见性一并调整）→ 访问器同步为 `pub(crate)`。
    #[allow(dead_code)] // 过渡期：P22 builder 组装后使用
    pub(crate) fn installer_factory(&self) -> &dyn InstallerFactory {
        self.installer_factory.as_ref()
    }

    /// 版本定位器（源：`Locator` 属性）
    pub fn locator(&self) -> &dyn VersionLocator {
        self.locator.as_ref()
    }

    /// 游戏选项提供方（源：`Options` 属性，可空）
    pub fn options(&self) -> Option<&(dyn OptionsProvider + Send + Sync)> {
        self.options.as_deref()
    }

    /// 服务器管理器（源：`ServerManager` 属性，可空）
    pub fn server(&self) -> Option<&(dyn ServerManager + Send + Sync)> {
        self.server.as_deref()
    }

    /// 下载源管理器（源：`DownloadManager` 属性）
    pub fn download_manager(&self) -> &dyn DownloadSourceManager {
        self.download_manager.as_ref()
    }

    /// 本地内容资源工厂（源：`LocalResourceProvider` 属性）
    pub fn local_resource_provider(&self) -> &dyn LocalResourcesFactory {
        self.local_resource_provider.as_ref()
    }

    /// 游戏根目录（源：`GameRoot` 属性）
    pub fn game_root(&self) -> &str {
        &self.game_root
    }
}

// B13: CreateModrinthSource 等 3 个工厂方法待扩展平台实现批次补充
// （源实现依赖 Services/Expansion 的 ModrinthBase / CurseForgeBase / FTBBase 具体类，
//   且依赖 HttpClient —— 网络层定案后一并引入）

