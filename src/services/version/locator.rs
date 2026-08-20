//! 本地版本定位器——扫描部分（B6，对应源：Services/DefaultVersionLocator.cs）
//!
//! 拆分说明：DefaultVersionLocator 的"版本扫描"与"缺失文件检查"拆为两个文件：
//! - 本文件（locator.rs）：扫描部分——字段/构造函数/GetAllVersions/GetVersionMetadata/
//!   IsVersionInstalled/RefreshCache/GetVersionPath/GetMetaFromJson/GetLibraries/
//!   EnsureCacheFresh/GetVanillaVersion/GetModloaderType/GetJarPath/IsVersionComplete/
//!   CalculateVersionSize
//! - locator_miss.rs：缺失检查方法（GetMissFilesAsync 等 8 个 async，由另一 Translator 实现）
//!
//! 跨文件契约：`pub(crate) struct DefaultVersionLocator` 的所有字段均为 pub(crate)
//! （locator_miss.rs 在另一模块 impl 同一 struct）；`get_libraries` / `get_meta_from_json`
//! 同样 pub(crate)（GetMissLibrariesAsync 与 4 个 string 重载使用）。
//! 缺失检查方法见 locator_miss.rs。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::api::version::VersionLocator;
use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::models::installer::MissFileInfo;
use crate::models::local::{LocalVersionInfo, ModloaderInfo, ModloaderType};
use crate::models::version_metadata::{ArgumentItem, CompleteVersionMetadata, Library, VersionArguments};
use crate::net::NetworkConfig;
use crate::services::version::locator_miss::AssetIndexData;
use crate::util::file_helper::validate_file_hash;
use crate::util::json_helper;
use crate::util::lib_helper::{check_libs_ver, is_rules_suitable};
use crate::util::version_json;

/// 下载源镜像配置（源：DefaultVersionLocator 内嵌套私有类 DownloadSource；
/// 嵌套类无法跨模块访问，改为模块级结构）
/// 字段 pub(crate)：locator_miss.rs 的缺失检查方法（ReplaceLibraryUrl / GetMissAssetsAsync /
/// ReplaceMainJarUrl）需要读取各镜像源 URL
pub(crate) struct DownloadSource {
    pub(crate) libraries_source: String,
    pub(crate) main_jar_source: String,
    pub(crate) assets_index_source: String,
    pub(crate) assets_source: String,
}

impl Default for DownloadSource {
    /// 官方下载源（源 DownloadSource 字段初始化默认值）
    fn default() -> Self {
        Self {
            libraries_source: "https://libraries.minecraft.net/".to_string(),
            main_jar_source: String::new(),
            assets_index_source: String::new(),
            assets_source: "https://resources.download.minecraft.net/".to_string(),
        }
    }
}

/// BMCLAPI 镜像下载源（源构造函数 `mirror == DownloadMirror.BMCLAPI` 分支）
fn bmclapi_download_source() -> DownloadSource {
    DownloadSource {
        libraries_source: "https://bmclapi2.bangbang93.com/maven/".to_string(),
        main_jar_source: "https://bmclapi2.bangbang93.com/".to_string(),
        assets_index_source: "https://bmclapi2.bangbang93.com/".to_string(),
        assets_source: "https://bmclapi2.bangbang93.com/assets/".to_string(),
    }
}

/// 缓存状态（源：`_versionCache` / `_metadataCache` / `_isCacheDirty` / `_isRefreshing` 四个字段）
/// 合并为单一把 Mutex 保护的状态（同步方法用 std Mutex，见翻译日志 p28a 缓存设计）
pub(crate) struct ScanState {
    /// 版本缓存（源：`_versionCache`，Dictionary<string, LocalVersionInfo>）
    version_cache: HashMap<String, LocalVersionInfo>,
    /// 元数据缓存（源：`_metadataCache`，Dictionary<string, CompleteVersionMetadata>）
    metadata_cache: HashMap<String, CompleteVersionMetadata>,
    /// 缓存是否脏（源：`_isCacheDirty`，构造后仅首次扫描置 false，源此后不再置 true）
    is_cache_dirty: bool,
    /// 是否正在刷新（源：`_isRefreshing`，防 EnsureCacheFresh 与 GetVersionMetadata 相互递归）
    is_refreshing: bool,
}

/// 本地版本定位器（源：`internal class DefaultVersionLocator : IVersionLocator`）
/// ⚠️ 所有字段 pub(crate)：locator_miss.rs 在另一模块 impl 同一 struct，需访问全部字段
pub(crate) struct DefaultVersionLocator {
    /// 版本根目录（源：`_versionsRootPath = Path.Combine(gameRootPath, "versions")`）
    pub(crate) versions_root_path: String,
    /// 游戏根目录（源：`_gameRootPath`）
    pub(crate) game_root_path: String,
    /// 版本/元数据缓存与刷新标志（源：_versionCache/_metadataCache/_isCacheDirty/_isRefreshing）
    pub(crate) cache: Mutex<ScanState>,
    /// 下载源镜像配置（源：`_downloadSource`，嵌套私有类 DownloadSource）
    pub(crate) download_source: DownloadSource,
    /// HTTP 客户端（源：`_httpClient = httpClient ?? new HttpClient()`；
    /// 本文件扫描部分不使用，供 locator_miss.rs 的 DownloadAssetIndexAsync 使用）
    pub(crate) http_client: reqwest::Client,
}

impl DefaultVersionLocator {
    /// 创建版本定位器（源：构造函数 `DefaultVersionLocator(gameRootPath, mirror = Official, httpClient = null)`）。
    /// C# 可选参数按 p27 先例处理：`mirror` 由调用方显式传入（Rust 无默认参数）；
    /// `httpClient` 不注入，内部自建 `reqwest::Client::new()`（B4 定案 B13 再统一共享 client）。
    pub(crate) fn new(game_root_path: String, mirror: DownloadMirror) -> Self {
        let versions_root_path = Path::new(&game_root_path)
            .join("versions")
            .to_string_lossy()
            .into_owned();
        // 源 Directory.CreateDirectory(_versionsRootPath) 失败抛 IOException；
        // ⚠️ 构造签名返回 Self 无法传播错误 → 静默忽略（见翻译日志 p28a）
        // TD-4：创建失败记录日志（源 Directory.CreateDirectory 抛 IOException；Rust 构造无法传播）
        if let Err(e) = std::fs::create_dir_all(&versions_root_path) {
            eprintln!("创建版本目录失败（{}）：{e}", versions_root_path);
        }
        let download_source = if mirror == DownloadMirror::Bmclapi {
            bmclapi_download_source()
        } else {
            DownloadSource::default()
        };
        let locator = Self {
            versions_root_path,
            game_root_path,
            cache: Mutex::new(ScanState {
                version_cache: HashMap::new(),
                metadata_cache: HashMap::new(),
                is_cache_dirty: true,
                is_refreshing: false,
            }),
            download_source,
            // 内部自建客户端：应用全局 proxy/TLS 配置（启动器经 CoreOptions 注入）
            http_client: NetworkConfig::global()
                .apply(reqwest::Client::builder())
                .build()
                .expect("构建版本定位器 HTTP 客户端失败"),
        };
        // 源构造函数末尾调用 RefreshCache()（→ EnsureCacheFresh 首次全量扫描）。
        // ⚠️ 修复：不再构造时同步扫描。versions 目录可含数万条目（modpack 场景实测
        // 43,868 条目需 ~21.8s），每次 launch 重建 locator 都会阻塞 16-20s；
        // 扫描改为惰性，由 get_all_versions / is_version_installed 等查询入口触发一次。
        locator
    }

    /// 确保缓存最新（源：EnsureCacheFresh）。
    /// 锁内完成"标志检查 → 置 refreshing → 清空双缓存 → 收集版本目录列表"后立即释放锁，
    /// 再逐目录处理（循环内调 get_version_metadata 需加锁，避免 std Mutex 自死锁），
    /// 全部完成后复位 dirty/refreshing 标志（对应源 try/finally）。
    fn ensure_cache_fresh(&self) {
        let dirs = {
            let mut state = self.cache.lock().expect("版本缓存锁被毒化");
            if !state.is_cache_dirty || state.is_refreshing {
                return;
            }
            state.is_refreshing = true;
            state.version_cache.clear();
            state.metadata_cache.clear();

            if !Path::new(&self.versions_root_path).is_dir() {
                state.is_cache_dirty = false;
                state.is_refreshing = false;
                return;
            }

            // ⚠️ 源 Directory.GetDirectories 失败抛 IOException 传播到调用方；
            // Rust 侧 read_dir 失败静默视为空扫描（见翻译日志 p28a）
            let mut dirs = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&self.versions_root_path) {
                for entry in entries.flatten() {
                    let dir_path = entry.path();
                    if !dir_path.is_dir() {
                        continue;
                    }
                    let version_id = entry.file_name().to_string_lossy().into_owned();
                    dirs.push((version_id, dir_path));
                }
            }
            dirs
        };

        for (version_id, version_dir) in dirs {
            // 源：`{versionId}.json` 不存在 → continue
            let json_path = version_dir.join(format!("{version_id}.json"));
            if !json_path.is_file() {
                continue;
            }

            // 源 try/catch：不可解析的版本目录静默跳过
            // （C# 捕获 GetVersionMetadata 的 null 结果及 NRE 等全部异常；
            // Rust 侧由 Option/Result 链承载，无 panic 路径，见日志 p28a）
            let Some(metadata) = self.get_version_metadata(&version_id) else {
                continue;
            };

            let is_complete = self.is_version_complete(&version_id, &metadata);
            let total_size = calculate_version_size(&version_dir);

            let info = LocalVersionInfo {
                id: version_id.clone(),
                r#type: self.get_modloader_type(&metadata),
                release_time: metadata.release_time.clone(),
                is_complete,
                version_path: version_dir.to_string_lossy().into_owned(),
                vanilla_version: self.get_vanilla_version(&metadata, &version_id),
                total_size,
            };
            self.cache
                .lock()
                .expect("版本缓存锁被毒化")
                .version_cache
                .insert(version_id, info);
        }

        let mut state = self.cache.lock().expect("版本缓存锁被毒化");
        state.is_cache_dirty = false;
        state.is_refreshing = false;
    }

    /// 解析版本元数据 JSON（源：GetMetaFromJson；`ArgumentException("无效的版本 JSON 数据")`
    /// → Error::Params，见 manifest.rs ArgumentException 先例）
    /// pub(crate)：locator_miss.rs 的 4 个 `_from_json` 重载使用
    pub(crate) fn get_meta_from_json(
        &self,
        json_data: &str,
    ) -> Result<CompleteVersionMetadata, Error> {
        let metadata = json_helper::deserialize_version_metadata(json_data)
            .map_err(|e| Error::Params {
                message: "无效的版本 JSON 数据".to_string(),
                source: Some(Box::new(e)),
            })?
            .ok_or_else(|| Error::Params {
                message: "无效的版本 JSON 数据".to_string(),
                source: None,
            })?;
        Ok(metadata)
    }

    /// 获取库文件列表（源：GetLibraries；含规则适配与 InheritsFrom 父版本继承，收尾去重取高版本）。
    /// pub(crate)：locator_miss.rs 的 GetMissLibrariesAsync 使用
    pub(crate) fn get_libraries(&self, metadata: &CompleteVersionMetadata) -> Vec<Library> {
        let mut lib_list = Vec::new();

        // 源：`Rules is not { Count: > 0 } || IsRulesSuitable(Rules)` → 加入
        for lib in &metadata.libraries {
            let include = match &lib.rules {
                None => true,
                Some(rules) if rules.is_empty() => true,
                Some(rules) => is_rules_suitable(rules),
            };
            if include {
                lib_list.push(lib.clone());
            }
        }

        // 源：InheritsFrom 非空时读父版本 JSON，递归合并其库列表
        // （父 JSON 无效 → 源 catch (ArgumentException) 静默忽略）
        if let Some(parent) = &metadata.inherits_from {
            if !parent.is_empty() {
                if let Some(parent_json) = self.get_json_data(parent) {
                    if let Ok(parent_meta) = self.get_meta_from_json(&parent_json) {
                        lib_list.extend(self.get_libraries(&parent_meta));
                    }
                }
            }
        }

        // 源：LibHelper.CheckLibsVer → util::lib_helper::check_libs_ver
        check_libs_ver(lib_list)
    }

    /// 读取版本 JSON 文本（源：GetJsonData；`{versionsRoot}/{id}/{id}.json` 不存在 → null）
    fn get_json_data(&self, version_id: &str) -> Option<String> {
        let json_path = Path::new(&self.versions_root_path)
            .join(version_id)
            .join(format!("{version_id}.json"));
        if !json_path.is_file() {
            return None;
        }
        std::fs::read_to_string(&json_path).ok()
    }

    /// 获取对应的原版版本号（源：GetVanillaVersion，internal → pub(crate)）。
    /// 依次尝试：JAR 内版本（util::version_json::from_jar）→ arguments.game 的
    /// `--fml.mcVersion` 相邻元素 → "Unknown"
    pub(crate) fn get_vanilla_version(
        &self,
        meta: &CompleteVersionMetadata,
        version_id: &str,
    ) -> String {
        // 从 jar 读版本（源：GameVersionHelper.FromJar → util/version_json.rs 已移植）
        let jar_path = self.get_jar_path(version_id, meta);
        if let Some(version) = version_json::from_jar(&jar_path) {
            return version;
        }

        // 读 json：--fml.mcVersion（新版 Forge 1.13+ 写在 arguments.game 里）
        if let Some(VersionArguments::New { game, .. }) = &meta.arguments {
            // 源：`i < Count - 1`（空列表不进入循环）
            let mut i = 0;
            while i + 1 < game.len() {
                if let ArgumentItem::String(s) = &game[i] {
                    if s == "--fml.mcVersion" {
                        if let ArgumentItem::String(next) = &game[i + 1] {
                            return next.clone();
                        }
                        break;
                    }
                }
                i += 1;
            }
        }
        "Unknown".to_string()
    }

    /// 识别版本包含的 Mod 加载器（源：GetModloaderType；完整移植，不简化）
    fn get_modloader_type(&self, meta: &CompleteVersionMetadata) -> Vec<ModloaderInfo> {
        let mut types = Vec::new();
        let mut is_forge_found = false;
        let mut is_neo_forge_found = false;
        let mut is_fabric_found = false;
        let mut is_quilt_found = false;
        let mut is_optifine_found = false;
        let mut is_liteloader_found = false;
        let mut is_cleanroom_found = false;
        let mut is_babric_found = false;
        // 源 isLegacyFabricFound：仅置位、从未读取（源同）——
        // 保留状态以对齐行为，赋值处抑制 unused_assignments 告警
                let mut _is_legacy_fabric_found = false;

        // 源 `if (meta != null)`：Rust 引用非空，恒成立，直接执行

        // 检查 libraries（源逻辑：name 先整体小写，后续 Contains/Split 均作用于小写串）
        for lib in &meta.libraries {
            let name = lib.name.to_lowercase();
            if !name.is_empty() {
                // 识别 OptiFine
                if name.contains("optifine") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "optifine" {
                            is_optifine_found = true;
                            let ver = mod_version_from(name_parts[2]);
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::OptiFine,
                                version: ver,
                            });
                        }
                    }
                }
                // 识别 LiteLoader
                if name.contains("liteloader") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "liteloader" {
                            is_liteloader_found = true;
                            let ver = mod_version_from(name_parts[2]);
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::LiteLoader,
                                version: ver,
                            });
                        }
                    }
                }
                // 识别 Cleanroom（第一处；源 Trace.WriteLine → eprintln!）
                if name.contains("cleanroom") {
                    eprintln!("{name} Found");
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "cleanroom" {
                            is_cleanroom_found = true;
                            let ver = mod_version_from(name_parts[2]);
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::Cleanroom,
                                version: ver,
                            });
                        }
                    }
                }
                // 识别旧版本 Forge
                if name.contains("forge") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "forge" {
                            is_forge_found = true;
                            let ver = mod_version_from(name_parts[2]);
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::Forge,
                                version: ver,
                            });
                        }
                    }
                }
                // 识别新版本 Forge（fmlloader）
                if name.contains("minecraftforge") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "fmlloader" {
                            is_forge_found = true;
                            let ver = mod_version_from(name_parts[2]);
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::Forge,
                                version: ver,
                            });
                        }
                    }
                }
                // 识别 Babric
                if name.contains("babric") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 && name_parts[0] == "babric" {
                        is_babric_found = true;
                        types.push(ModloaderInfo {
                            r#type: ModloaderType::Babric,
                            version: "Unknown".to_string(),
                        });
                    }
                }
                // 识别 Fabric（源条件 `!isBabricFound`）
                if name.contains("fabric") && !is_babric_found {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "fabric" || name_parts[1] == "fabric-loader" {
                            // 判断是 LegacyFabric 还是正常的 Fabric（name 已小写）
                            if name_parts[0].contains("legacyfabric") {
                                _is_legacy_fabric_found = true;
                                types.push(ModloaderInfo {
                                    r#type: ModloaderType::LegacyFabric,
                                    version: name_parts[2].to_string(),
                                });
                            } else {
                                is_fabric_found = true;
                                types.push(ModloaderInfo {
                                    r#type: ModloaderType::Fabric,
                                    version: name_parts[2].to_string(),
                                });
                            }
                        }
                    }
                }
                // 识别 Quilt
                if name.contains("quilt") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1] == "quilt" || name_parts[1] == "quilt-loader" {
                            is_quilt_found = true;
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::Quilt,
                                version: name_parts[2].to_string(),
                            });
                        }
                    }
                }
                // 识别 Cleanroom（第二处，源 parts[1] 用 Contains 判定）
                if name.contains("cleanroom") {
                    let name_parts: Vec<&str> = name.split(':').collect();
                    if name_parts.len() == 3 {
                        if name_parts[1].contains("cleanroom") {
                            is_cleanroom_found = true;
                            types.push(ModloaderInfo {
                                r#type: ModloaderType::Cleanroom,
                                version: name_parts[2].to_string(),
                            });
                        }
                    }
                }
            }
        }

        // 检查 arguments（仅新格式；--fml.neoForgeVersion / --fml.forgeVersion 取当前元素值）
        if let Some(VersionArguments::New { game, .. }) = &meta.arguments {
            let mut can_get_version = false;
            for arg in game {
                let value = match arg {
                    ArgumentItem::String(s) => s.clone(),
                    _ => String::new(),
                };
                if value == "--fml.neoForgeVersion" {
                    can_get_version = true;
                }
                if value == "--fml.forgeVersion" {
                    can_get_version = true;
                }
                if can_get_version {
                    if !value.is_empty() {
                        types.push(ModloaderInfo {
                            r#type: ModloaderType::NeoForge,
                            version: value,
                        });
                    } else {
                        types.push(ModloaderInfo {
                            r#type: ModloaderType::NeoForge,
                            version: "Unknown".to_string(),
                        });
                    }
                    is_neo_forge_found = true;
                    break;
                }
            }
        }

        // 检查 mainClass（源：小写后比较）
        let main_class = meta.main_class.to_lowercase();
        if main_class == "net.minecraft.client.main.main" {
            return vec![ModloaderInfo {
                r#type: ModloaderType::Vanilla,
                version: String::new(),
            }];
        }
        if !is_quilt_found && main_class == "org.quiltmc.loader.impl.launch.knot.knotclient" {
            is_quilt_found = true;
            types.push(ModloaderInfo {
                r#type: ModloaderType::Quilt,
                version: "Unknown".to_string(),
            });
        }
        if !(is_neo_forge_found || is_forge_found) && main_class == "cpw.mods.bootstraplauncher.bootstraplauncher"
        {
            is_neo_forge_found = true;
            types.push(ModloaderInfo {
                r#type: ModloaderType::NeoForge,
                version: "Unknown".to_string(),
            });
        }
        if !is_fabric_found
            && !is_babric_found
            && main_class == "net.fabricmc.loader.impl.launch.knot.knotclient"
        {
            is_fabric_found = true;
            types.push(ModloaderInfo {
                r#type: ModloaderType::Fabric,
                version: "Unknown".to_string(),
            });
        }
        if !is_forge_found && main_class == "net.minecraftforge.bootstrap.bootstraplauncher" {
            is_forge_found = true;
            types.push(ModloaderInfo {
                r#type: ModloaderType::Forge,
                version: "Unknown".to_string(),
            });
        }
        if !is_cleanroom_found && main_class == "top.outlands.foundation.boot.foundation" {
            is_cleanroom_found = true;
            types.push(ModloaderInfo {
                r#type: ModloaderType::Cleanroom,
                version: "Unknown".to_string(),
            });
        }

        if !(is_optifine_found
            || is_forge_found
            || is_neo_forge_found
            || is_liteloader_found
            || is_fabric_found
            || is_quilt_found
            || is_cleanroom_found
            || is_babric_found)
        {
            if main_class == "net.minecraft.launchwrapper.Launch" {
                return vec![ModloaderInfo {
                    r#type: ModloaderType::Vanilla,
                    version: String::new(),
                }];
            }
        }
        if types.is_empty() {
            return vec![ModloaderInfo {
                r#type: ModloaderType::Unknown,
                version: "Unknown".to_string(),
            }];
        }

        types
    }

    /// 获取主客户端 jar 路径（源：GetJarPath）。
    /// 本版本 jar 存在 → 本版本；否则 InheritsFrom 非空 → 父版本 jar；否则空串
    fn get_jar_path(&self, version_id: &str, metadata: &CompleteVersionMetadata) -> String {
        let client_path = Path::new(&self.get_version_path(version_id))
            .join(format!("{version_id}.jar"));
        if !client_path.is_file() {
            match &metadata.inherits_from {
                Some(parent) if !parent.is_empty() => {
                    return Path::new(&self.get_version_path(parent))
                        .join(format!("{parent}.jar"))
                        .to_string_lossy()
                        .into_owned();
                }
                _ => return String::new(),
            }
        }
        client_path.to_string_lossy().into_owned()
    }

    /// 判断版本是否完整（源：IsVersionComplete；仅检查主 jar 是否存在）
    fn is_version_complete(&self, version_id: &str, metadata: &CompleteVersionMetadata) -> bool {
        Path::new(&self.get_jar_path(version_id, metadata)).is_file()
    }
}

#[async_trait]
impl VersionLocator for DefaultVersionLocator {
    // 缺失检查方法（get_miss_files / get_miss_files_from_json / get_miss_libraries /
    // get_miss_libraries_from_json / get_miss_main_jar / get_miss_main_jar_from_json /
    // get_miss_assets / get_miss_assets_from_json，8 个 async）已并入本 impl 块
    // （原在 locator_miss.rs，B6 合并，见 CHECKPOINT_BATCH_6）

    /// 获取全部本地版本（源：GetAllVersions）
    fn get_all_versions(&self) -> Vec<LocalVersionInfo> {
        self.ensure_cache_fresh();
        self.cache
            .lock()
            .expect("版本缓存锁被毒化")
            .version_cache
            .values()
            .cloned()
            .collect()
    }

    /// 获取本地版本元数据（源：GetVersionMetadata；C# `CompleteVersionMetadata?` → Option；
    /// 空 id → None；缓存未命中读 `{versionsRoot}/{id}/{id}.json`，成功写回缓存）
    fn get_version_metadata(&self, version_id: &str) -> Option<CompleteVersionMetadata> {
        if version_id.is_empty() {
            return None;
        }

        // ⚠️ 修复：此处不再调用 ensure_cache_fresh()（全量扫描 versions 目录）。
        // 元数据按 id 直读版本 JSON，缓存 miss 时直接读文件（下方已有逻辑），
        // 无需求助全量缓存；launch 完整性检查（InheritsFrom 链会经过本方法）
        // 因此不再被每次 locator 重建触发的扫描阻塞（见 p28a 修复记录）。
        if let Some(metadata) = self.cache.lock().expect("版本缓存锁被毒化").metadata_cache.get(version_id)
        {
            return Some(metadata.clone());
        }

        let version_path = self.get_version_path(version_id);
        let json_path = Path::new(&version_path).join(format!("{version_id}.json"));

        if !json_path.is_file() {
            return None;
        }

        // 源 try/catch → null：读取/解析失败均返回 None
        let metadata = std::fs::read_to_string(&json_path)
            .ok()
            .and_then(|json| json_helper::deserialize_version_metadata(&json).ok().flatten());
        if let Some(metadata) = &metadata {
            self.cache
                .lock()
                .expect("版本缓存锁被毒化")
                .metadata_cache
                .insert(version_id.to_string(), metadata.clone());
        }
        metadata
    }

    /// 判断指定版本是否已安装（源：IsVersionInstalled）
    fn is_version_installed(&self, version_id: &str) -> bool {
        self.ensure_cache_fresh();
        self.cache
            .lock()
            .expect("版本缓存锁被毒化")
            .version_cache
            .contains_key(version_id)
    }

    /// 刷新本地缓存（源：RefreshCache）
    fn refresh_cache(&self) {
        self.ensure_cache_fresh();
    }

    /// 获取版本目录路径（源：GetVersionPath；Path.Combine(_versionsRootPath, versionId)）
    fn get_version_path(&self, version_id: &str) -> String {
        Path::new(&self.versions_root_path)
            .join(version_id)
            .to_string_lossy()
            .into_owned()
    }
    /// 获取缺失文件列表（源：GetMissFilesAsync(CompleteVersionMetadata meta)，第 274-282 行）
    /// 组合：缺失库 + 缺失主 Jar（非空时）+ 缺失资源，顺序与源一致。
    async fn get_miss_files(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Vec<MissFileInfo>, Error> {
        let mut miss_files = self.get_miss_libraries(meta).await?;
        if let Some(miss_main_jar) = self.get_miss_main_jar(meta).await? {
            miss_files.push(miss_main_jar);
        }
        miss_files.extend(self.get_miss_assets(meta).await?);
        Ok(miss_files)
    }

    /// 获取缺失文件列表（源：GetMissFilesAsync(string jsonData)，第 284-285 行）
    /// 先解析 JSON（失败按源 ArgumentException 语义报错），再委托 meta 重载。
    async fn get_miss_files_from_json(
        &self,
        json_data: &str,
    ) -> Result<Vec<MissFileInfo>, Error> {
        let meta = self.get_meta_from_json(json_data)?;
        self.get_miss_files(&meta).await
    }

    /// 获取缺失库文件列表（源：GetMissLibrariesAsync(CompleteVersionMetadata meta)，
    /// 第 150-164 行）。
    /// 判定：库文件存在且（Sha1 为空或 SHA1 匹配）→ 跳过；否则记为缺失，
    /// Path 替换为本地绝对路径（gameRoot/libraries/{相对路径}，源 `item with { Path = localPath }`）。
    async fn get_miss_libraries(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Vec<MissFileInfo>, Error> {
        let mut miss_files = Vec::new();
        for lib in self.get_libraries(meta) {
            for item in self.get_library_check_items(&lib) {
                let local_path = Path::new(&self.game_root_path)
                    .join("libraries")
                    .join(&item.path);
                let local_path_str = local_path.to_string_lossy().into_owned();
                // 源：File.Exists(localPath) && (string.IsNullOrEmpty(item.Sha1) || ValidateFileHash(...))
                if local_path.is_file()
                    && (item.sha1.is_empty() || validate_file_hash(&local_path_str, &item.sha1))
                {
                    continue;
                }
                miss_files.push(MissFileInfo {
                    path: local_path_str,
                    ..item
                });
            }
        }
        Ok(miss_files)
    }

    /// 获取缺失库文件列表（源：GetMissLibrariesAsync(string jsonData)，第 166-167 行）
    async fn get_miss_libraries_from_json(
        &self,
        json_data: &str,
    ) -> Result<Vec<MissFileInfo>, Error> {
        let meta = self.get_meta_from_json(json_data)?;
        self.get_miss_libraries(&meta).await
    }

    /// 获取缺失主 Jar 文件（源：GetMissMainJarAsync(CompleteVersionMetadata meta)，
    /// 第 169-193 行）。
    /// 客户端 Jar 已存在且 SHA1 匹配 → None；否则返回 MissFileInfo
    /// （name = "{id}.jar"，url 经 ReplaceMainJarUrl 镜像替换，sha1 = client.Sha1，
    /// path = versions/{id}/{id}.jar）。
    /// 无 downloads 时沿 InheritsFrom 链向上查找父版本元数据并递归。
    async fn get_miss_main_jar(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Option<MissFileInfo>, Error> {
        if let Some(client) = meta.downloads.as_ref().map(|d| &d.client) {
            let jar_path = Path::new(&self.versions_root_path)
                .join(&meta.id)
                .join(format!("{}.jar", meta.id));
            let jar_path_str = jar_path.to_string_lossy().into_owned();
            // 源：File.Exists(jarPath) && ValidateFileHash(jarPath, client.Sha1)
            if jar_path.is_file() && validate_file_hash(&jar_path_str, &client.sha1) {
                return Ok(None);
            }
            return Ok(Some(MissFileInfo {
                name: format!("{}.jar", meta.id),
                url: self.replace_main_jar_url(&client.url),
                sha1: client.sha1.clone(),
                path: jar_path_str,
            }));
        }

        // 源：meta.InheritsFrom 非空 → GetVersionMetadata(父) → 递归
        if let Some(inherits) = &meta.inherits_from {
            if !inherits.is_empty() {
                if let Some(parent_meta) = self.get_version_metadata(inherits) {
                    return self.get_miss_main_jar(&parent_meta).await;
                }
            }
        }

        Ok(None)
    }

    /// 获取缺失主 Jar 文件（源：GetMissMainJarAsync(string jsonData)，第 195-196 行）
    async fn get_miss_main_jar_from_json(
        &self,
        json_data: &str,
    ) -> Result<Option<MissFileInfo>, Error> {
        let meta = self.get_meta_from_json(json_data)?;
        self.get_miss_main_jar(&meta).await
    }

    /// 获取缺失资源文件列表（源：GetMissAssetsAsync(CompleteVersionMetadata meta)，
    /// 第 198-231 行）。
    /// 无 AssetIndex 时沿 InheritsFrom 链递归；资源索引缺失或 SHA1 不匹配时先下载索引；
    /// 逐资源判定：objects/{前2位}/{hash} 存在且 SHA1 匹配 → 跳过，否则 URL =
    /// `{assetsSource}{hash[..2]}/{hash}`（整串 http:// → https:// 替换，源逐字规则）。
    async fn get_miss_assets(
        &self,
        meta: &CompleteVersionMetadata,
    ) -> Result<Vec<MissFileInfo>, Error> {
        let Some(asset_index) = meta.asset_index.as_ref() else {
            // 源：assetIndex == null → InheritsFrom 为空或父元数据不存在 → 空列表；否则递归
            if let Some(inherits) = &meta.inherits_from {
                if !inherits.is_empty() {
                    if let Some(parent_meta) = self.get_version_metadata(inherits) {
                        return self.get_miss_assets(&parent_meta).await;
                    }
                }
            }
            return Ok(Vec::new());
        };

        let index_path = Path::new(&self.game_root_path)
            .join("assets")
            .join("indexes")
            .join(format!("{}.json", asset_index.id));
        let index_path_str = index_path.to_string_lossy().into_owned();

        // 源：!File.Exists(indexPath) || !ValidateFileHash(indexPath, assetIndex.Sha1) → 下载索引
        if !index_path.is_file() || !validate_file_hash(&index_path_str, &asset_index.sha1) {
            self.download_asset_index(asset_index, &index_path_str).await?;
        }

        // 源：File.ReadAllTextAsync + JsonSerializer.Deserialize（解析失败向上传播）
        let index_json =
            tokio::fs::read_to_string(&index_path_str)
                .await
                .map_err(|e| Error::DownloadFailed {
                    message: format!("读取资源索引失败: {index_path_str}"),
                    source: Some(Box::new(e)),
                })?;
        let index_data: AssetIndexData = serde_json::from_str(&index_json).map_err(|e| {
            Error::DownloadFailed {
                message: format!("解析资源索引失败: {index_path_str}"),
                source: Some(Box::new(e)),
            }
        })?;
        // 源：indexData?.Objects == null → 返回空列表
        let Some(objects) = index_data.objects else {
            return Ok(Vec::new());
        };

        let mut miss_files = Vec::new();
        for obj in objects.values() {
            let hash = &obj.hash;
            // 源：hash[..2]（哈希长度 < 2 时 C# 抛 ArgumentOutOfRangeException ↔ Rust 切片 panic）
            let local_path = Path::new(&self.game_root_path)
                .join("assets")
                .join("objects")
                .join(&hash[..2])
                .join(hash);
            let local_path_str = local_path.to_string_lossy().into_owned();
            // 源：File.Exists(localPath) && ValidateFileHash(localPath, hash)
            if local_path.is_file() && validate_file_hash(&local_path_str, hash) {
                continue;
            }
            // 源：$"{_downloadSource.assetsSource}{hash[..2]}/{hash}".Replace("http://", "https://")
            // （String.Replace 全局替换 ↔ str::replace 全局替换，语义一致）
            let url = format!(
                "{}{}/{}",
                self.download_source.assets_source,
                &hash[..2],
                hash
            )
            .replace("http://", "https://");
            miss_files.push(MissFileInfo {
                name: hash.clone(),
                url,
                sha1: hash.clone(),
                path: local_path_str,
            });
        }
        Ok(miss_files)
    }

    /// 获取缺失资源文件列表（源：GetMissAssetsAsync(string jsonData)，第 233-234 行）
    async fn get_miss_assets_from_json(
        &self,
        json_data: &str,
    ) -> Result<Vec<MissFileInfo>, Error> {
        let meta = self.get_meta_from_json(json_data)?;
        self.get_miss_assets(&meta).await
    }
}

/// 加载器版本提取（源 GetModloaderType 内 5 处相同的内联逻辑：
/// 坐标第 3 段含 '-' 且 Split 恰好 2 段时取第 2 段，否则取整段。
/// 纯提取重构，行为逐字一致，非简化）
fn mod_version_from(part: &str) -> String {
    if let Some((_, second)) = part.split_once('-') {
        // C# `Split('-')` 长度 == 2 ⇔ 恰好一个 '-'
        if !second.contains('-') {
            return second.to_string();
        }
    }
    part.to_string()
}

/// 计算目录总大小（源：CalculateVersionSize，static → 模块级函数）。
/// 递归遍历全部文件求和（C# GetFiles("*", AllDirectories) + Sum(Length)）；
/// 任一错误 → 0（源 try/catch → 0）
fn calculate_version_size(version_path: &Path) -> i64 {
    let mut total: i64 = 0;
    let mut stack: Vec<PathBuf> = vec![version_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return 0,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // 元数据读取失败 → 0（对应源 catch → 0）
            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => return 0,
            };
            if metadata.is_dir() {
                stack.push(path);
            } else {
                total += metadata.len() as i64;
            }
        }
    }
    total
}






