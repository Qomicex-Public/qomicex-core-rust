//! GameCore 完整组装器（P59，对应源：Builder/GameCoreBuilder.cs + Builder/CoreOptions.cs）
//!
//! - `GameCoreBuilder`（sealed class）→ `GameCoreBuilder`（Builder 模式）
//! - `CoreOptions` record → `CoreOptions` struct（字段 + Default，源属性初始化值承载于 Default impl）
//! - `Build()` → `build(&self) -> Arc<GameCore>`（源 Build() 无失败返回路径 → 不返回 Result）
//! - `With*` 注入方法 → 可选字段 `Option<Arc<dyn Trait + Send + Sync>>`，build() 内以
//!   `unwrap_or_else` 承载 C# `??`（空合并）语义
//! - `Configure(Action<CoreOptions>)` → `configure(&mut self, FnOnce(&mut CoreOptions))`
//! - `CreateAuthProvider` → 私有 `create_auth_provider`，按 AuthMode 分派
//!
//! ⚠️ UNMAPPED / 签名差异（详见翻译日志 p59-builder.md）：
//! - `InstallerProvider` 默认实现（源 Services/InstallerProvider.cs）尚未移植 →
//!   未注入时 build() panic 并提示（注入 `with_installer_provider` 后正常工作）
//! - `DefaultVersionLocator::new` 签名不含 http 参数（内部自建 client）
//! - `JavaProviderService::new` 需 mirror 参数（源 JavaProvider(http) 仅 http）
//! - `CoreOptions.CacheExpiry` / `MaxConcurrentDownloads`：源 Build() 未使用（死选项），
//!   Rust 侧保留字段但未接入 VersionManagementService 签名
//!
//! Android 兼容性：纯 Rust（std + reqwest），无平台 API。

use std::sync::Arc;
use std::time::Duration;

use crate::api::auth::AuthProvider;
use crate::api::download::DownloadSourceManager;
use crate::api::installer::{InstallerFactory, InstallerProvider};
use crate::api::java::JavaProvider;
use crate::api::launch::LaunchExecutor;
use crate::api::local::LocalResourcesFactory;
use crate::api::options::OptionsProvider as OptionsProviderTrait;
use crate::api::server::ServerManager as ServerManagerTrait;
use crate::api::version::{VersionLocator, VersionManagement};
use crate::core::GameCore;
use crate::models::auth::{AuthMode, AuthOptions};
use crate::models::download::DownloadMirror;
use crate::services::auth::microsoft::MicrosoftAuthProvider;
use crate::services::auth::offline::OfflineAuthProvider;
use crate::services::auth::yggdrasil::YggdrasilAuthProvider;
use crate::services::download::mirror::DefaultDownloadSourceManager;
use crate::services::installers::factory::DefaultInstallerFactory;
use crate::services::installers::installer::InstallerBase;
use crate::services::java::provider::JavaProviderService;
use crate::services::launch::jvm_args::LaunchExecutor as ProcessLaunchExecutor;
use crate::services::local::factory::DefaultLocalResourcesFactory;
use crate::services::version::locator::DefaultVersionLocator;
use crate::services::version::version_management::VersionManagementService;

/// 核心配置项（源：`CoreOptions` record，Builder/CoreOptions.cs）。
///
/// 字段与默认值逐字对应源属性初始化器（`Default` impl 承载）：
/// - `LauncherName = "Qomicex.Core.AOT"` → `launcher_name`
/// - `GameRoot = ".minecraft"` → `game_root`
/// - `UserAgent` 长串 → `user_agent`
/// - `MaxConcurrentDownloads = 8` → `max_concurrent_downloads`
///   （⚠️ 源 Build() 未使用——VersionManagementService 内部补全器恒用默认 8，本字段为
///   死选项保留；接入需调整 VersionManagementService 签名，见翻译日志 p59）
/// - `CacheExpiry = TimeSpan.FromMinutes(5)` → `cache_expiry = Duration::from_secs(300)`
///   （⚠️ 同上未接入：VersionManifestCache 内部为常量 CACHE_DURATION=300s，接入需调整
///   VersionManagementService 签名，见翻译日志 p59）
/// - `DownloadMirror = Official` / `AuthMode = Offline` → 同名字段
/// - 三个可空路径字段 → `Option<String>`
#[derive(Debug, Clone, PartialEq)]
pub struct CoreOptions {
    /// 启动器名称（源：`LauncherName`）
    pub launcher_name: String,
    /// 游戏根目录（源：`GameRoot`）
    pub game_root: String,
    /// HTTP User-Agent（源：`UserAgent`）
    pub user_agent: String,
    /// 最大并发下载数（源：`MaxConcurrentDownloads`，⚠️ 源 Build() 未使用）
    pub max_concurrent_downloads: i32,
    /// 缓存有效期（源：`CacheExpiry` TimeSpan → Duration，⚠️ 未接入 VersionManagementService）
    pub cache_expiry: Duration,
    /// 下载镜像偏好（源：`DownloadMirror`）
    pub download_mirror: DownloadMirror,
    /// 认证模式（源：`AuthMode`）
    pub auth_mode: AuthMode,
    /// Microsoft 客户端 ID（源：`MicrosoftClientId`，可空）
    pub microsoft_client_id: Option<String>,
    /// Yggdrasil 服务器地址（源：`YggdrasilServerUrl`，可空）
    pub yggdrasil_server_url: Option<String>,
    /// 认证配置项（源：`AuthOptions`，含显式默认值）
    pub auth_options: AuthOptions,
    /// options.json 路径（源：`OptionsJsonPath`，可空）
    pub options_json_path: Option<String>,
    /// descriptions.json 路径（源：`DescriptionsJsonPath`，可空）
    pub descriptions_json_path: Option<String>,
    /// 版本清单 JSON 文件路径（源：`MinecraftManifestPath`，可空）
    pub minecraft_manifest_path: Option<String>,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            launcher_name: "Qomicex.Core.AOT".to_string(),
            game_root: ".minecraft".to_string(),
            user_agent: "Qomicex.Core/1.0 (tmoaminecraft@gmail.com; lenmei233@vip.qq.com)"
                .to_string(),
            max_concurrent_downloads: 8,
            // 源：TimeSpan.FromMinutes(5)
            cache_expiry: Duration::from_secs(300),
            download_mirror: DownloadMirror::Official,
            auth_mode: AuthMode::Offline,
            microsoft_client_id: None,
            yggdrasil_server_url: None,
            auth_options: AuthOptions::default(),
            options_json_path: None,
            descriptions_json_path: None,
            minecraft_manifest_path: None,
        }
    }
}

/// 游戏核心构建器（源：`GameCoreBuilder`，sealed class → 普通 struct）。
///
/// Builder 模式：方法链返回 `&mut Self`（源返回 `this`），`build()` 从 `&self` 组装
/// （不消费 builder，可重复调用，源 Build() 同样不修改状态）。
/// 各服务为 `Option<Arc<dyn Trait + Send + Sync>>`：未注入的按源 `??` 语义构造默认实现。
pub struct GameCoreBuilder {
    options: CoreOptions,
    version: Option<Arc<dyn VersionManagement + Send + Sync>>,
    auth: Option<Arc<dyn AuthProvider + Send + Sync>>,
    launch: Option<Arc<dyn LaunchExecutor + Send + Sync>>,
    http: Option<reqwest::Client>,
    source: Option<Arc<dyn DownloadSourceManager + Send + Sync>>,
    java_provider: Option<Arc<dyn JavaProvider + Send + Sync>>,
    installer_provider: Option<Arc<dyn InstallerProvider + Send + Sync>>,
    options_provider: Option<Arc<dyn OptionsProviderTrait + Send + Sync>>,
    server_manager: Option<Arc<dyn ServerManagerTrait + Send + Sync>>,
    installer_factory: Option<Arc<dyn InstallerFactory + Send + Sync>>,
    local_resource_provider: Option<Arc<dyn LocalResourcesFactory + Send + Sync>>,
}

impl GameCoreBuilder {
    /// 创建空构建器（源：无参构造，`_options = new CoreOptions()`）。
    pub fn new() -> Self {
        Self {
            options: CoreOptions::default(),
            version: None,
            auth: None,
            launch: None,
            http: None,
            source: None,
            java_provider: None,
            installer_provider: None,
            options_provider: None,
            server_manager: None,
            installer_factory: None,
            local_resource_provider: None,
        }
    }

    /// 批量配置（源：`Configure(Action<CoreOptions>)`，Action 可变修改 `_options`）。
    pub fn configure(&mut self, configure: impl FnOnce(&mut CoreOptions)) -> &mut Self {
        configure(&mut self.options);
        self
    }

    /// 设置游戏根目录（源：`UseGameRoot(string path)`）。
    pub fn use_game_root(&mut self, path: impl Into<String>) -> &mut Self {
        self.options.game_root = path.into();
        self
    }

    /// 启用离线认证（源：`UseOfflineAuth(string username)`）。
    ///
    /// `AuthMode = Offline`，`AuthOptions = { Name = username, Mode = Offline }`（record
    /// with 表达式 → 字段赋值）。
    pub fn use_offline_auth(&mut self, username: impl Into<String>) -> &mut Self {
        self.options.auth_mode = AuthMode::Offline;
        self.options.auth_options.name = Some(username.into());
        self.options.auth_options.mode = AuthMode::Offline;
        self
    }

    /// 启用 Microsoft 认证（源：`UseMicrosoftAuth(string clientId)`）。
    ///
    /// `AuthMode = Microsoft`，`MicrosoftClientId = clientId`，
    /// `AuthOptions = { Mode = Microsoft }`。
    pub fn use_microsoft_auth(&mut self, client_id: impl Into<String>) -> &mut Self {
        self.options.auth_mode = AuthMode::Microsoft;
        self.options.microsoft_client_id = Some(client_id.into());
        self.options.auth_options.mode = AuthMode::Microsoft;
        self
    }

    /// 启用 Yggdrasil 认证（源：`UseYggdrasilAuth(string serverUrl, string? email = null)`）。
    ///
    /// `AuthMode = Yggdrasil`，`YggdrasilServerUrl = serverUrl`，
    /// `AuthOptions = { Mode = Yggdrasil, ServerUrl = serverUrl }`。
    /// ⚠️ 源 `email` 参数从未被使用（签名保留），Rust 侧以 `_email` 对齐 API 形态。
    pub fn use_yggdrasil_auth(
        &mut self,
        server_url: impl Into<String>,
        _email: Option<&str>,
    ) -> &mut Self {
        let server_url = server_url.into();
        self.options.auth_mode = AuthMode::Yggdrasil;
        self.options.yggdrasil_server_url = Some(server_url.clone());
        self.options.auth_options.mode = AuthMode::Yggdrasil;
        self.options.auth_options.server_url = Some(server_url);
        self
    }

    /// 设置下载镜像偏好（源：`UseDownloadMirror(DownloadMirror mirror)`）。
    pub fn use_download_mirror(&mut self, mirror: DownloadMirror) -> &mut Self {
        self.options.download_mirror = mirror;
        self
    }

    /// 注入版本管理服务（源：`WithVersionService(IVersionManagementService)`）。
    pub fn with_version_service(
        &mut self,
        version: Arc<dyn VersionManagement + Send + Sync>,
    ) -> &mut Self {
        self.version = Some(version);
        self
    }

    /// 注入认证提供方（源：`WithAuthProvider(IAuthProvider)`）。
    pub fn with_auth_provider(&mut self, auth: Arc<dyn AuthProvider + Send + Sync>) -> &mut Self {
        self.auth = Some(auth);
        self
    }

    /// 注入启动执行器（源：`WithLaunchExecutor(ILaunchExecutor)`）。
    pub fn with_launch_executor(
        &mut self,
        launch: Arc<dyn LaunchExecutor + Send + Sync>,
    ) -> &mut Self {
        self.launch = Some(launch);
        self
    }

    /// 注入 HTTP 客户端（源：`WithHttpClient(HttpClient)`；HttpClient → reqwest::Client）。
    ///
    /// ⚠️ 差异：注入后源仍会补写 User-Agent 头（`ParseAdd`），Rust 侧 `reqwest::Client` 为
    /// 不可变构建对象，无法事后补头 → 注入客户端的 User-Agent 由调用方自行设置
    /// （见翻译日志 p59）。
    pub fn with_http_client(&mut self, http: reqwest::Client) -> &mut Self {
        self.http = Some(http);
        self
    }

    /// 注入下载源管理器（源：`WithDownloadSourceManager(IDownloadSourceManager)`）。
    pub fn with_download_source_manager(
        &mut self,
        source: Arc<dyn DownloadSourceManager + Send + Sync>,
    ) -> &mut Self {
        self.source = Some(source);
        self
    }

    /// 注入 Java 提供方（源：`WithJavaProvider(IJavaProvider)`）。
    pub fn with_java_provider(
        &mut self,
        java_provider: Arc<dyn JavaProvider + Send + Sync>,
    ) -> &mut Self {
        self.java_provider = Some(java_provider);
        self
    }

    /// 注入安装器提供方（源：`WithInstallerProvider(IInstallerProvider)`）。
    ///
    /// ⚠️ 默认实现尚未移植（源 Services/InstallerProvider.cs）→ 未注入时 `build()` panic。
    pub fn with_installer_provider(
        &mut self,
        installer_provider: Arc<dyn InstallerProvider + Send + Sync>,
    ) -> &mut Self {
        self.installer_provider = Some(installer_provider);
        self
    }

    /// 注入安装器工厂（源：`WithInstallerFactory(IInstallerFactory)`）。
    pub fn with_installer_factory(
        &mut self,
        installer_factory: Arc<dyn InstallerFactory + Send + Sync>,
    ) -> &mut Self {
        self.installer_factory = Some(installer_factory);
        self
    }

    /// 注入本地资源工厂（源：`WithLocalResourceProvider(ILocalResourcesFactory)`）。
    pub fn with_local_resource_provider(
        &mut self,
        local_resource_provider: Arc<dyn LocalResourcesFactory + Send + Sync>,
    ) -> &mut Self {
        self.local_resource_provider = Some(local_resource_provider);
        self
    }

    /// 注入游戏选项提供方（源：`WithOptionsProvider(IOptionsProvider)`）。
    pub fn with_options_provider(
        &mut self,
        options_provider: Arc<dyn OptionsProviderTrait + Send + Sync>,
    ) -> &mut Self {
        self.options_provider = Some(options_provider);
        self
    }

    /// 注入服务器管理器（源：`WithServerManager(IServerManager)`）。
    pub fn with_server_manager(
        &mut self,
        server_manager: Arc<dyn ServerManagerTrait + Send + Sync>,
    ) -> &mut Self {
        self.server_manager = Some(server_manager);
        self
    }

    /// 组装游戏核心（源：`Build()`）。
    ///
    /// 默认服务创建顺序逐字保留：http → UA 写入 → InstallerBase 全局 UA → 下载源 →
    /// 版本管理 → 认证（AuthMode 分派）→ 启动执行器 → Java 提供方 → 安装器提供方 →
    /// 版本定位器 → 安装器工厂 → 本地资源工厂 → 选项提供方（三路径齐备才建）→
    /// 服务器管理器 → `GameCore::new` 全参数。
    /// 源 Build() 无失败返回路径 → 返回 `Arc<GameCore>`（不返回 Result）。
    pub fn build(&self) -> Arc<GameCore> {
        // 源：var http = _http ?? new HttpClient();
        //      http.DefaultRequestHeaders.UserAgent.ParseAdd(_options.UserAgent);
        let http = match &self.http {
            Some(client) => client.clone(),
            None => build_http_client(&self.options.user_agent),
        };

        // 源：InstallerBase.DefaultUserAgent ??= _options.UserAgent
        // （静态属性 ??= 首次赋值生效 ↔ OnceLock::set 首次写入生效）
        InstallerBase::set_default_user_agent(self.options.user_agent.clone());

        // 源：var downloadSource = _source ?? new DefaultDownloadSourceManager(_options.DownloadMirror)
        let download_source = match &self.source {
            Some(source) => source.clone(),
            None => Arc::new(DefaultDownloadSourceManager::new(self.options.download_mirror)),
        };

        // 源：var version = _version ?? new VersionManagementService(_options.GameRoot, http, downloadSource)
        let version = match &self.version {
            Some(version) => version.clone(),
            None => Arc::new(VersionManagementService::new(
                self.options.game_root.clone(),
                Some(http.clone()),
                Some(download_source.clone()),
            )),
        };

        // 源：var auth = _auth ?? CreateAuthProvider(http)
        let auth = match &self.auth {
            Some(auth) => auth.clone(),
            None => self.create_auth_provider(http.clone()),
        };

        // 源：var launch = _launch ?? new LaunchExecutor(_options.LauncherName, _options.GameRoot)
        let launch = match &self.launch {
            Some(launch) => launch.clone(),
            None => Arc::new(ProcessLaunchExecutor::new(
                self.options.launcher_name.clone(),
                self.options.game_root.clone(),
            )),
        };

        // 源：var javaProvider = _javaProvider ?? new JavaProvider(http)
        // ⚠️ 签名增强：Rust JavaProviderService::new(http, mirror) 需镜像偏好
        //   （源仅注入 http；mirror 用于本地扫描器，见翻译日志 p59）
        let java_provider = match &self.java_provider {
            Some(provider) => provider.clone(),
            None => Arc::new(JavaProviderService::new(
                http.clone(),
                self.options.download_mirror,
            )),
        };

        // 源：var installerProvider = _installerProvider ?? new InstallerProvider(http, _options.DownloadMirror)
        // ⚠️ UNMAPPED：Rust 侧尚无 InstallerProvider 默认实现（源 Services/InstallerProvider.cs
        //   未移植）→ 未注入时 panic 并提示；注入后正常工作
        let installer_provider = match &self.installer_provider {
            Some(provider) => provider.clone(),
            None => Arc::new(crate::services::installers::provider::InstallerProviderService::new(
                http.clone(),
                self.options.download_mirror,
            )),
        };

        // 源：var locator = new DefaultVersionLocator(_options.GameRoot, _options.DownloadMirror, http)
        // ⚠️ 签名差异：Rust DefaultVersionLocator::new(game_root, mirror) 无 http 参数
        //   （内部自建 reqwest::Client::new()，B4 定案 B13 再统一共享 client）
        let locator: Arc<dyn VersionLocator + Send + Sync> = Arc::new(DefaultVersionLocator::new(
            self.options.game_root.clone(),
            self.options.download_mirror,
        ));

        // 源：_installerFactory ??= new DefaultInstallerFactory()
        let installer_factory = match &self.installer_factory {
            Some(factory) => factory.clone(),
            None => Arc::new(DefaultInstallerFactory),
        };

        // 源：_localResourceProvider ??= new DefaultLocalResourcesFactory(http, _options.GameRoot)
        let local_resource_provider = match &self.local_resource_provider {
            Some(provider) => provider.clone(),
            None => Arc::new(DefaultLocalResourcesFactory::new(
                http.clone(),
                self.options.game_root.clone(),
            )),
        };

        // 源：if (OptionsJsonPath is not null && DescriptionsJsonPath is not null
        //          && MinecraftManifestPath is not null)
        //      {
        //          var manifest = File.ReadAllText(MinecraftManifestPath);
        //          _optionsProvider ??= new OptionsProvider(OptionsJsonPath, DescriptionsJsonPath,
        //              manifest, GameRoot, string.Empty, false);
        //      }
        // 注意：File.ReadAllText 在 if 内、??= 外 → 即使已注入 optionsProvider，只要三路径
        // 齐备仍会读文件（读取失败源抛 IOException）→ Rust 逐字保留该行为（失败 panic!，
        // 同 options_txt.rs 源异常 ↔ panic! 先例）
        let options_provider = if self.options.options_json_path.is_some()
            && self.options.descriptions_json_path.is_some()
            && self.options.minecraft_manifest_path.is_some()
        {
            let minecraft_manifest_path = self
                .options
                .minecraft_manifest_path
                .as_deref()
                .expect("三路径齐备分支必然存在 minecraft_manifest_path");
            let manifest = std::fs::read_to_string(minecraft_manifest_path).unwrap_or_else(|e| {
                panic!("读取版本清单失败（{minecraft_manifest_path}）: {e}")
            });
            match &self.options_provider {
                Some(provider) => Some(provider.clone()),
                None => Some(Arc::new(crate::services::options::options_txt::OptionsProvider::new(
                    self.options.options_json_path.as_deref().unwrap_or_default(),
                    self.options
                        .descriptions_json_path
                        .as_deref()
                        .unwrap_or_default(),
                    &manifest,
                    self.options.game_root.clone(),
                    String::new(),
                    false,
                )) as Arc<dyn crate::api::options::OptionsProvider + Send + Sync>),
            }
        } else {
            self.options_provider.clone()
        };

        // 源：_serverManager ??= new ServerManager(_options.GameRoot, string.Empty, false)
        let server_manager = match &self.server_manager {
            Some(manager) => Some(manager.clone()),
            None => Some(Arc::new(crate::services::server::servers_dat::ServerManager::new(
                self.options.game_root.clone(),
                String::new(),
                false,
            )) as Arc<dyn crate::api::server::ServerManager + Send + Sync>),
        };

        // 源：return new DefaultGameCore(version, auth, launch, javaProvider, installerProvider,
        //     locator, http, _options.GameRoot, _optionsProvider, _serverManager,
        //     downloadSource, _installerFactory, _localResourceProvider);
        // Rust GameCore::new 无 http 参数（B4 定案，B13 网络层统一共享 client），参数顺序
        // 见 core.rs 头注释；download_manager 传解析后的源变量（注入或默认，源同逻辑）
        Arc::new(GameCore::new(
            version,
            auth,
            launch,
            java_provider,
            installer_provider,
            installer_factory,
            locator,
            self.options.game_root.clone(),
            options_provider,
            server_manager,
            download_source,
            local_resource_provider,
            http,
        ))
    }

    /// 创建认证提供方（源：`CreateAuthProvider(HttpClient)`，私有）。
    ///
    /// AuthMode 分派（switch 逐字保留）：
    /// - `Microsoft` → `MicrosoftAuthProvider(http, MicrosoftClientId ?? "")`
    /// - `Yggdrasil` → `YggdrasilAuthProvider(http, YggdrasilServerUrl ?? "https://authserver.mojang.com")`
    /// - 其余（源 `_ => new DefaultAuthProvider()`，即离线提供方）→ `OfflineAuthProvider`
    fn create_auth_provider(&self, http: reqwest::Client) -> Arc<dyn AuthProvider + Send + Sync> {
        match self.options.auth_mode {
            AuthMode::Microsoft => Arc::new(MicrosoftAuthProvider::new(
                http,
                self.options.microsoft_client_id.clone().unwrap_or_default(),
            )),
            AuthMode::Yggdrasil => Arc::new(YggdrasilAuthProvider::new(
                http,
                self.options
                    .yggdrasil_server_url
                    .clone()
                    .unwrap_or_else(|| "https://authserver.mojang.com".to_string()),
            )),
            AuthMode::Offline => Arc::new(OfflineAuthProvider),
        }
    }
}

impl Default for GameCoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// 构建共享 HTTP 客户端（源：`new HttpClient()` + `DefaultRequestHeaders.UserAgent.ParseAdd`）。
///
/// reqwest 在 `build()` 时校验 User-Agent；失败处理决策（翻译日志 p59）：
/// 源 `ParseAdd` 对非法 UA 抛 `FormatException`，且 Build() 无失败返回路径 →
/// Rust 侧 `panic!`（消息含 UA），等价"构建即失败"语义；build() 保持无 Result 通道。
fn build_http_client(user_agent: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .build()
        .unwrap_or_else(|e| panic!("构建 HTTP 客户端失败（UserAgent: {user_agent}）: {e}"))
}



