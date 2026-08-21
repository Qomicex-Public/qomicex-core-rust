//! JVM/游戏参数组装（B8，对应源：Services/LaunchExecutor.cs）
//!
//! 拆分说明：源 LaunchExecutor 类的"参数组装"与"进程/原生库"两部分拆为两个文件
//! （同一 struct，两个 impl 块）：
//! - 本文件（jvm_args.rs）：参数组装——SelectParams / GetClassPath / GetJVMParams
//!   （双重载）/ GetGameParams / GetMainClass / NameToUuid / GetDataDir 及私有辅助
//! - process.rs：launch / kill / natives（UnzipNatives / GetNatives /
//!   ParseJavaLibraryPath 等），由另一 Translator 并行编写
//!
//! 跨文件契约：`pub(crate) struct LaunchExecutor`，字段 `launch_name` / `game_dir`
//! 均为 pub(crate)（process.rs 的另一 impl 块需访问），构造签名
//! `new(launch_name: String, game_dir: String)` 对应源构造器
//! `LaunchExecutor(string launchName, string gameDir)`。
//!
//! ⚠️ 关键决策（详见翻译日志 p33-jvm-args.md）：
//! - 源 ParseGameJson / GetJVMParams / GetGameParams 用 ParamsMeta.Config 解析版本 JSON
//!   （C# 缺键 → null）；B1 的 params_meta::Config / Arguments 为全必填 serde 模型，
//!   无法表达缺键语义（legacy JSON 无 arguments 键、新版本 JSON 无 minecraftArguments 键）
//!   → 本文件经 serde_json::Value 手工访问字段复刻 C# null 语义
//!   （见 ParsedConfig / RawArguments），参数项对象仍用 params_meta::ParamEntry 反序列化
//!   （对应源 `ParamsJsonContent.Default.ParamEntry`）。
//! - GetClassPath / GetMainClass 与源一致，经 DefaultVersionLocator 读取
//!   CompleteVersionMetadata。
//! - 参数顺序 / 内容 / 替换令牌 / 分隔符逐字保留（特殊兼容点）。

use std::path::Path;

use serde_json::Value;

use crate::api::version::VersionLocator;
use crate::error::Error;
use crate::models::auth::AuthMode;
use crate::models::download::DownloadMirror;
use crate::models::launch::LaunchOptions;
use crate::models::params_meta::ParamEntry;
use crate::models::version_metadata::Library;
use crate::services::version::locator::DefaultVersionLocator;
use crate::util::file_helper::format_dir_path;
use crate::util::lib_helper::{
    check_libs_ver, is_class_path, is_rule_suitable, maven_to_path, remove_conflicting_libraries,
};
use crate::util::platform::{generate_uuid, get_separator};

/// 启动执行器（源：`internal sealed class LaunchExecutor : ILaunchExecutor`）
/// 本文件承载参数组装方法；进程启动 / 原生库解压方法在 process.rs 的另一 impl 块
pub(crate) struct LaunchExecutor {
    /// 启动器名（源：`_launchName`）
    pub(crate) launch_name: String,
    /// 游戏根目录（源：`_gameDir`）
    pub(crate) game_dir: String,
}

/// 版本 JSON 顶层解析载体（对应源 ParamsJsonContent.Config 的解析语义，
/// C# 各字段缺键 → null，此处以 Option 表达，见文件头决策说明）
struct ParsedConfig {
    /// 继承的父版本 ID（源：`Config.InheritsFrom`）
    inherits_from: Option<String>,
    /// 资源索引 ID（源：`config.AssetIndex?.Id`；本文件仅消费 Id）
    asset_index_id: Option<String>,
    /// 新版 arguments 对象（源：`Config.Arguments`，缺键 → null）
    arguments: Option<RawArguments>,
    /// 旧版 minecraftArguments（源：`Config.MinecraftArguments`）
    minecraft_arguments: Option<String>,
}

/// 新版 arguments 的 jvm/game 列表（对应源 ParamsJsonContent.Arguments）
/// 缺键 → None（源 `List<JsonElement>` 缺键 → null）
struct RawArguments {
    jvm: Option<Vec<Value>>,
    game: Option<Vec<Value>>,
}

impl LaunchExecutor {
    /// 创建启动执行器（源构造函数 `LaunchExecutor(string launchName, string gameDir)`）
    pub(crate) fn new(launch_name: String, game_dir: String) -> Self {
        Self {
            launch_name,
            game_dir,
        }
    }

    /// 拼接完整启动参数字符串（源：SelectParams，353-482 行）。
    /// 顺序逐字保留：JVM → mainClass → 游戏参数 → assetIndex → 账户 → classpath →
    /// 主 jar（Forge/NeoForge 跳过逻辑）→ 版本隔离目录 → OptiFine 兼容改写 → 令牌替换。
    pub(crate) fn select_params(&self, options: &LaunchOptions) -> Result<String, Error> {
        let game_dir = self.effective_game_dir(options);
        let mut param_list: Vec<String> = Vec::new();
        let config = self.parse_game_json(&game_dir, &options.version)?;

        // 拼接 JVM
        param_list.extend(self.get_jvm_params(options)?);

        // 拼接 mainClass
        param_list.push(self.get_main_class(options)?);

        // 拼接 Game 参数
        param_list.extend(self.get_game_params(options)?);

        // 处理参数：获取 assetIndex（源 369-376 行；父版本为 null 时抛 ParamsException"获取AssetIndex错误"）
        let mut assets_index = config.asset_index_id.clone().unwrap_or_default();
        if assets_index.is_empty() {
            let Some(parent) = &config.inherits_from else {
                return Err(Error::Params {
                    message: "获取AssetIndex错误".to_string(),
                    source: None,
                });
            };
            let parent_config = self.parse_game_json(&game_dir, parent)?;
            // 源此处直接 `inheritsFromConfig.AssetIndex.Id`，父版本无 assetIndex 时 NRE
            // → 启动失败；Rust 以 Error::Params 表达同语义（见日志 p33 §7）
            assets_index = parent_config.asset_index_id.ok_or_else(|| Error::Params {
                message: format!(
                    "父版本 {parent} 缺少 assetIndex（源此处置 NullReferenceException）"
                ),
                source: None,
            })?;
        }

        // 处理账户（源 377-385 行）
        // 源：`AuthOptions?.Mode != AuthMode.Offline`（AuthOptions 为 null 时判为 Microsoft）
        let login_mode = if options.auth_options.as_ref().map(|a| a.mode) != Some(AuthMode::Offline)
        {
            "Microsoft"
        } else {
            "Legacy"
        };
        let mut options = options.clone();
        if login_mode == "Legacy" {
            // 源 `options.AuthOptions!` 空值容忍符：login_mode == Legacy 时 AuthOptions 必非空
            let auth = options
                .auth_options
                .as_mut()
                .expect("login_mode == Legacy 时 AuthOptions 必非空（源空值容忍符 `!`）");
            auth.uuid = Some(name_to_uuid(auth.name.as_deref().unwrap_or("")));
        }

        // 处理 ClassPath（源 387 行）
        let cp_libs = self.get_class_path(&options)?;

        let separator = get_separator();

        // 处理主 jar 路径（源 389-418 行，Forge/NeoForge 跳过逻辑逐字）
        let base_main_jar = Path::new(&game_dir)
            .join("versions")
            .join(&options.version)
            .join(format!("{}.jar", options.version));
        let main_jar_path = if !base_main_jar.is_file() {
            let mut skip = false;
            let main_class = self.get_main_class(&options)?.to_lowercase();
            if main_class.contains("bootstraplauncher.bootstrapper")
                || main_class.contains("bootstraplauncher.bootstraplauncher")
            {
                // 为 Forge/NeoForge 版本
                if assets_index.contains('.') {
                    let indexs: Vec<&str> = assets_index.split('.').collect();
                    if indexs.len() >= 2 {
                        // 源：Regex.Match(indexs[1] ?? "", @"^\d+") 取前导数字 → int.TryParse
                        let digits: String = indexs[1]
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect();
                        if let Ok(minor) = digits.parse::<i32>() {
                            // >= 1.17
                            skip = minor >= 17;
                        }
                    }
                } else if assets_index.chars().all(|c| c.is_ascii_digit()) {
                    // 22w42a+ 纯数字索引
                    skip = true;
                }
            }
            if skip {
                base_main_jar.to_string_lossy().into_owned()
            } else {
                // 源 `config!.InheritsFrom`：为 null 时 NRE → 启动失败；Rust 以 Error 表达
                let Some(parent) = &config.inherits_from else {
                    return Err(Error::Params {
                        message: "主 jar 不存在且无继承版本（源此处 config.InheritsFrom 为 null 抛 NullReferenceException）".to_string(),
                        source: None,
                    });
                };
                Path::new(&game_dir)
                    .join("versions")
                    .join(parent)
                    .join(format!("{parent}.jar"))
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            base_main_jar.to_string_lossy().into_owned()
        };

        // 拼接 classpath（源 420-428 行：每库后接路径分隔符，最后接主 jar）
        let mut cp_libs_str = String::new();
        for cp in &cp_libs {
            let path = Path::new(&game_dir)
                .join("libraries")
                .join(maven_to_path(&cp.name));
            cp_libs_str.push_str(&path.to_string_lossy());
            cp_libs_str.push_str(separator);
        }
        cp_libs_str.push_str(&main_jar_path);

        // 处理版本隔离路径（源 430-439 行）
        let game_version_dir = if options.version_isolation {
            Path::new(&game_dir)
                .join("versions")
                .join(&options.version)
                .to_string_lossy()
                .into_owned()
        } else {
            game_dir.clone()
        };

        // 处理 OptiFine 与 Forge 兼容（源 441-457 行）：移除 tweaker 及其前一参数，
        // 追加 --tweakClass 形式（若同时含两者，仅第一个分支生效，与源一致）
        if let Some(index) = param_list
            .iter()
            .position(|p| p == "optifine.OptiFineTweaker")
        {
            param_list.remove(index);
            param_list.remove(index - 1);
            param_list.push("--tweakClass".to_string());
            param_list.push("optifine.OptiFineTweaker".to_string());
        } else if let Some(index) = param_list
            .iter()
            .position(|p| p == "optifine.OptiFineForgeTweaker")
        {
            param_list.remove(index);
            param_list.remove(index - 1);
            param_list.push("--tweakClass".to_string());
            param_list.push("optifine.OptiFineForgeTweaker".to_string());
        }

        // 替换参数（源 459-480 行，令牌与顺序逐字保留）
        let mut param_string = param_list.join(" ");

        let max_memory = options
            .java_options
            .as_ref()
            .map(|j| j.max_memory_mb)
            .unwrap_or(512);
        let natives_dir = Path::new(&game_dir)
            .join("versions")
            .join(&options.version)
            .join(format!("{}-natives", options.version))
            .to_string_lossy()
            .into_owned();
        let game_assets = Path::new(&game_dir)
            .join("assets")
            .to_string_lossy()
            .into_owned();
        let libraries_dir = Path::new(&game_dir)
            .join("libraries")
            .to_string_lossy()
            .into_owned();

        let uuid = options
            .auth_options
            .as_ref()
            .and_then(|a| a.uuid.clone())
            .unwrap_or_default();
        let access_token = options
            .auth_options
            .as_ref()
            .and_then(|a| a.access_token.clone())
            .unwrap_or_default();
        let player_name = options
            .auth_options
            .as_ref()
            .and_then(|a| a.name.clone())
            .unwrap_or_default();
        let authlib_injector_param = options
            .auth_options
            .as_ref()
            .and_then(|a| a.authlib_injector_param.clone());

        param_string = param_string
            .replace("${max_memory}", &max_memory.to_string())
            .replace(
                "${natives_directory}",
                &normalize_arg(&format_dir_path(natives_dir.trim_end_matches(separator))),
            )
            .replace("${launcher_name}", &normalize_arg(&self.launch_name))
            .replace("${classpath_separator}", separator)
            .replace(
                "${game_assets}",
                &normalize_arg(&format_dir_path(game_assets.trim_end_matches(separator))),
            )
            .replace("${uuid}", &uuid)
            .replace("${user_properties}", "{}")
            .replace("${version_type}", &normalize_arg(&self.launch_name))
            .replace("${user_type}", login_mode)
            .replace("${auth_access_token}", &normalize_arg(&access_token))
            .replace("${assets_index_name}", &normalize_arg(&assets_index))
            .replace("${assets_root}", &format_dir_path(&game_assets))
            .replace("${classpath}", &normalize_arg(&cp_libs_str))
            .replace(
                "${game_directory}",
                &normalize_arg(&format_dir_path(
                    game_version_dir.trim_end_matches(separator),
                )),
            )
            .replace("${version_name}", &format!("\"{}\"", options.version))
            .replace("${auth_uuid}", &uuid)
            .replace("${auth_player_name}", &player_name)
            .replace(
                "${library_directory}",
                &format_dir_path(libraries_dir.trim_end_matches(separator)),
            )
            .replace("${launcher_version}", "23")
            // 源：`Replace("${authlib_injector_param}", options.AuthOptions?.AuthlibInjectorParam)`
            // C# 对 null 替换值抛 ArgumentNullException；Rust 以空串替换（见日志 ⚠️ 偏差）
            .replace(
                "${authlib_injector_param}",
                authlib_injector_param.as_deref().unwrap_or_default(),
            );

        Ok(param_string)
    }

    /// 获取 classpath 库列表（源：GetClassPath，484-519 行）
    /// 有规则时逐规则 IsRuleSuitable（任一适合即加入，循环无 break，与源一致）+
    /// IsClassPath + 去重；无规则直接判定；沿 InheritsFrom 递归；
    /// 收尾 CheckLibsVer + RemoveConflictingLibraries
    pub(crate) fn get_class_path(&self, options: &LaunchOptions) -> Result<Vec<Library>, Error> {
        let game_dir = self.effective_game_dir(options);
        let locator = DefaultVersionLocator::new(game_dir, DownloadMirror::Official);
        let meta = locator
            .get_version_metadata(&options.version)
            .ok_or_else(|| Error::Params {
                message: format!("版本元数据未找到: {}", options.version),
                source: None,
            })?;

        let mut lib_list: Vec<Library> = Vec::new();
        for lib in &meta.libraries {
            match &lib.rules {
                Some(rules) if !rules.is_empty() => {
                    for rule in rules {
                        if is_rule_suitable(Some(rule)) {
                            if is_class_path(lib) && !lib_list.contains(lib) {
                                lib_list.push(lib.clone());
                            }
                        }
                    }
                }
                _ => {
                    if is_class_path(lib) && !lib_list.contains(lib) {
                        lib_list.push(lib.clone());
                    }
                }
            }
        }

        // 源：InheritsFrom 非空时递归父版本（options 仅替换 Version）
        if let Some(parent) = &meta.inherits_from {
            if !parent.is_empty() {
                let mut parent_options = options.clone();
                parent_options.version = parent.clone();
                lib_list.extend(self.get_class_path(&parent_options)?);
            }
        }

        // 源：LibHelper.RemoveConflictingLibraries(LibHelper.CheckLibsVer(LibList))
        Ok(remove_conflicting_libraries(check_libs_ver(lib_list)))
    }

    /// 获取主类名（源：GetMainClass，540-554 行）
    /// 经 DefaultVersionLocator 读取元数据；MainClass 为空 → 沿 InheritsFrom 递归；
    /// 仍无 → ParamsException("MainClass键不存在 (Version: {version})")
    pub(crate) fn get_main_class(&self, options: &LaunchOptions) -> Result<String, Error> {
        let game_dir = self.effective_game_dir(options);
        self.get_main_class_inner(&game_dir, &options.version)
    }

    fn get_main_class_inner(&self, game_dir: &str, version: &str) -> Result<String, Error> {
        let locator = DefaultVersionLocator::new(game_dir.to_string(), DownloadMirror::Official);
        let Some(meta) = locator.get_version_metadata(version) else {
            // 源：meta 为 null → mainClass 为 null → 无继承可查 → 抛 ParamsException
            return Err(Error::Params {
                message: format!("MainClass键不存在 (Version: {version})"),
                source: None,
            });
        };
        if !meta.main_class.is_empty() {
            return Ok(meta.main_class);
        }
        if let Some(parent) = &meta.inherits_from {
            if !parent.is_empty() {
                return self.get_main_class_inner(game_dir, parent);
            }
        }
        Err(Error::Params {
            message: format!("MainClass键不存在 (Version: {version})"),
            source: None,
        })
    }

    /// 获取 JVM 参数列表（源：GetJVMParams(options)，即双重载的 addDefaultParams=true）
    /// pub(crate)：process.rs 的 ParseJavaLibraryPath 使用
    pub(crate) fn get_jvm_params(&self, options: &LaunchOptions) -> Result<Vec<String>, Error> {
        self.get_jvm_params_inner(options, true)
    }

    /// 获取 JVM 参数列表（源：GetJVMParams(LaunchOptions, bool addDefaultParams)，576-698 行）
    /// 默认参数集 / InheritsFrom 递归 / legacy 分支 / 元素规则判定的顺序逐字保留
    pub(crate) fn get_jvm_params_inner(
        &self,
        options: &LaunchOptions,
        add_default_params: bool,
    ) -> Result<Vec<String>, Error> {
        let game_dir = self.effective_game_dir(options);
        let mut jvm_list: Vec<String> = Vec::new();
        if options.version.is_empty() {
            // 源：ArgumentException("Version cannot be null or empty.", nameof(options.Version))
            return Err(Error::Params {
                message: "Version cannot be null or empty.".to_string(),
                source: None,
            });
        }

        let config = self.parse_game_json(&game_dir, &options.version)?;

        if add_default_params {
            // 添加默认参数（源 607-632 行，顺序逐字）
            jvm_list.push("-XX:+UseG1GC".to_string());
            jvm_list.push("-XX:-UseAdaptiveSizePolicy".to_string());
            jvm_list.push("-XX:-OmitStackTraceInFastThrow".to_string());
            jvm_list.push("-Dfml.ignoreInvalidMinecraftCertificates=True".to_string());
            jvm_list.push("-Dfml.ignorePatchDiscrepancies=True".to_string());
            jvm_list.push("-Dlog4j2.formatMsgNoLookups=true".to_string());

            if let Some(extra) = &options.java_options {
                if let Some(args) = &extra.extra_jvm_args {
                    jvm_list.extend(args.iter().cloned());
                }
            }

            // Windows 适配（源 619-628 行：IsWindows 且 OS 主版本 >= 10）
            // 精确实现：读注册表 CurrentMajorVersionNumber（Win10/11 = 10），
            // 与源 Environment.OSVersion.Version.Major 语义一致；读取失败按 0 处理
            // （Windows 7/8/8.1 源不加这两个参数）
            if cfg!(windows) && windows_major_version().unwrap_or(0) >= 10 {
                jvm_list.push("-Dos.name=\"Windows 10\"".to_string());
                jvm_list.push("-Dos.version=\"10.0\"".to_string());
            }

            jvm_list.push(format!(
                "-Dminecraft.launcher.brand=\"{}\"",
                self.launch_name
            ));
            jvm_list.push("-Dminecraft.launcher.version=23".to_string());
        }

        // 处理 InheritsFrom（源 635-640 行，位于 legacy 检查之前，顺序逐字）
        if let Some(parent) = &config.inherits_from {
            if !parent.is_empty() {
                let mut parent_options = options.clone();
                parent_options.version = parent.clone();
                jvm_list.extend(self.get_jvm_params_inner(&parent_options, false)?);
            }
        }

        // 旧版 Modloader Json（LiteLoader 等）兼容（源 642-653 行）：
        // `config!.Arguments?.Jvm is null` —— arguments 缺失或 jvm 缺失均进入该分支
        let Some(jvm_items) = config.arguments.as_ref().and_then(|a| a.jvm.as_ref()) else {
            jvm_list.push("-Djava.library.path=${natives_directory}".to_string());
            jvm_list.push("-cp".to_string());
            jvm_list.push("${classpath}".to_string());
            jvm_list.push("${authlib_injector_param}".to_string());
            jvm_list.push("-Xmn256m".to_string());
            jvm_list.push("-Xmx${max_memory}m".to_string());
            return Ok(jvm_list); // 旧版兼容
        };

        // 处理当前 json 的 jvm（源 655-696 行）
        for element in jvm_items {
            if element.is_object() {
                // 源：JsonSerializer.Deserialize(element, ParamsJsonContent.Default.ParamEntry)
                let entry: ParamEntry =
                    serde_json::from_value(element.clone()).map_err(|e| Error::Params {
                        message: "JVM 参数项解析失败".to_string(),
                        source: Some(Box::new(e)),
                    })?;

                // 源：`entry?.Rules is { Count: > 0 }` 时逐规则判定，任一适合 → shouldAdd；
                // 无规则/规则为空 → 保持 false（该对象不加入，quirk 保留）
                let mut should_add = false;
                if let Some(rules) = &entry.rules {
                    if !rules.is_empty() {
                        for rule in rules {
                            if is_rule_suitable(Some(rule)) {
                                should_add = true;
                                break;
                            }
                        }
                    }
                }

                if should_add {
                    match &entry.value {
                        Value::String(s) => {
                            if !s.contains("-Dos.version=") && !s.contains("-Dos.name=") {
                                jvm_list.push(normalize_arg(s));
                            }
                        }
                        Value::Array(items) => {
                            for v in items {
                                // 源 `v.GetString()!`：非字符串元素 → null → NRE
                                let Some(s) = v.as_str() else {
                                    return Err(Error::Params {
                                        message: "JVM 参数值元素非字符串（源此处 GetString() 为 null → NullReferenceException）".to_string(),
                                        source: None,
                                    });
                                };
                                if !s.contains("-Dos.version=") && !s.contains("-Dos.name=") {
                                    jvm_list.push(normalize_arg(s));
                                }
                            }
                        }
                        // 源：value 为其他 ValueKind（Undefined/Number 等）时两分支均不进入
                        _ => {}
                    }
                }
            } else if let Some(s) = element.as_str() {
                if !s.contains("-Dos.version=") && !s.contains("-Dos.name=") {
                    jvm_list.push(normalize_arg(s));
                }
            }
        }
        Ok(jvm_list)
    }

    /// 获取游戏参数列表（源：GetGameParams，700-784 行）。
    /// 顺序逐字保留：InheritsFrom 递归（JoinServer/JoinWorld 置 null）→ legacy
    /// minecraftArguments → game 列表 → JoinServer/JoinWorld（quickPlay 判定）
    pub(crate) fn get_game_params(&self, options: &LaunchOptions) -> Result<Vec<String>, Error> {
        let game_dir = self.effective_game_dir(options);
        let mut game_list: Vec<String> = Vec::new();
        if options.version.is_empty() {
            // 源：ArgumentException("Version cannot be null or empty.", nameof(options.Version))
            return Err(Error::Params {
                message: "Version cannot be null or empty.".to_string(),
                source: None,
            });
        }

        let config = self.parse_game_json(&game_dir, &options.version)?;

        // 处理 InheritsFrom（源 723-729 行：JoinServer/JoinWorld 置 null 后递归）
        if let Some(parent) = &config.inherits_from {
            if !parent.is_empty() {
                let mut parent_options = options.clone();
                parent_options.version = parent.clone();
                parent_options.join_server = None;
                parent_options.join_world = None;
                game_list.extend(self.get_game_params(&parent_options)?);
            }
        }

        // 处理当前 json 的 game（源 730-755 行）
        match &config.arguments {
            None => {
                // 源：`config!.Arguments is null` → MinecraftArguments.Split(' ')
                let Some(minecraft_arguments) = &config.minecraft_arguments else {
                    // 源：MinecraftArguments 为 null → NullReferenceException → 启动失败
                    return Err(Error::Params {
                        message:
                            "缺少 minecraftArguments（源此处为 null → NullReferenceException）"
                                .to_string(),
                        source: None,
                    });
                };
                // C# string.Split(' ') 保留空元素（无 RemoveEmptyEntries），split(' ') 同语义
                game_list.extend(minecraft_arguments.split(' ').map(String::from));
                return Ok(game_list);
            }
            Some(args) => {
                let Some(game_items) = &args.game else {
                    // 源：Arguments.Game 为 null → foreach → NullReferenceException → 启动失败
                    return Err(Error::Params {
                        message:
                            "arguments 缺少 game 列表（源此处为 null → NullReferenceException）"
                                .to_string(),
                        source: None,
                    });
                };
                for element in game_items {
                    if element.is_object() {
                        // 源：JsonSerializer.Deserialize(element, ParamsJsonContent.Default.ParamEntry)
                        let entry: ParamEntry =
                            serde_json::from_value(element.clone()).map_err(|e| Error::Params {
                                message: "游戏参数项解析失败".to_string(),
                                source: Some(Box::new(e)),
                            })?;
                        // 源：`entry?.Rules is { Count: > 0 } → continue`（带规则的对象整体跳过）
                        if let Some(rules) = &entry.rules {
                            if !rules.is_empty() {
                                continue;
                            }
                        }
                        match &entry.value {
                            Value::String(s) => game_list.push(normalize_arg(s)),
                            Value::Array(items) => {
                                for v in items {
                                    // 源 `v.GetString()!`：非字符串元素 → null → NRE
                                    let Some(s) = v.as_str() else {
                                        return Err(Error::Params {
                                            message: "游戏参数值元素非字符串（源此处 GetString() 为 null → NullReferenceException）".to_string(),
                                            source: None,
                                        });
                                    };
                                    game_list.push(normalize_arg(s));
                                }
                            }
                            // 源：value 为其他 ValueKind 时两分支均不进入
                            _ => {}
                        }
                    } else if let Some(s) = element.as_str() {
                        game_list.push(normalize_arg(s));
                    }
                }
            }
        }

        // JoinServer / JoinWorld（源 757-780 行）
        if let Some(join_server) = &options.join_server {
            if !join_server.is_empty() {
                if is_quick_play_supported(config.asset_index_id.as_deref()) {
                    game_list.push("--quickPlayMultiplayer".to_string());
                    // 源此处未 NormalizeArg（逐字保留）
                    game_list.push(join_server.clone());
                } else {
                    let (server, port) = parse_server_address(join_server);
                    game_list.push("--server".to_string());
                    game_list.push(server);
                    game_list.push("--port".to_string());
                    game_list.push(port.to_string());
                }
            }
        }
        let normalized_world = normalize_arg(options.join_world.as_deref().unwrap_or(""));
        if !normalized_world.is_empty() {
            if is_quick_play_supported(config.asset_index_id.as_deref()) {
                game_list.push("--quickPlaySingleplayer".to_string());
                game_list.push(normalized_world);
            }
        }

        Ok(game_list)
    }

    /// 获取数据目录（源：GetDataDir，815-833 行，static → 关联函数）
    /// 优先 QOMICEX_HOME 环境变量；其次 LocalApplicationData/qomicex-launcher；
    /// 若该目录存在 .qomicex-bootstrap 文件则读取其内容（Trim）作为自定义目录。
    /// pub(crate)：process.rs 启动失败日志路径使用

    /// 解析版本 JSON（对应源 ParseGameJson + JsonSerializer.Deserialize<Config>，556-574 行）
    /// ⚠️ B1 的 params_meta::Config 为全必填 serde 模型，无法表达 C# "缺键 → null" 语义
    /// （legacy JSON 无 arguments 键、新版本 JSON 无 minecraftArguments 键），故按
    /// serde_json::Value 手工访问字段，复刻 C# null 语义（见文件头决策说明）
    fn parse_game_json(&self, game_dir: &str, version: &str) -> Result<ParsedConfig, Error> {
        let json = self.read_version_json(game_dir, version)?;

        // 源：JSON "null" → Config null → ParamsException("版本Json解析失败")；
        // 其他非对象形态 → JsonException（同样以"版本Json解析失败"承载）
        let root: Value = serde_json::from_str(&json).map_err(|e| Error::Params {
            message: "版本Json解析失败".to_string(),
            source: Some(Box::new(e)),
        })?;
        let Some(obj) = root.as_object() else {
            return Err(parse_json_failed());
        };

        let inherits_from = match obj.get("inheritsFrom") {
            None | Some(Value::Null) => None, // 源：缺键 / null → InheritsFrom null
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(parse_json_failed()), // 源 JsonException
        };

        // 源仅消费 `config.AssetIndex?.Id`；此处只提取 Id（缺键 → null）
        let asset_index_id = match obj.get("assetIndex") {
            None | Some(Value::Null) => None,
            Some(Value::Object(a)) => match a.get("id") {
                None | Some(Value::Null) => None, // 源：AssetIndex.Id 为 null
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err(parse_json_failed()),
            },
            Some(_) => return Err(parse_json_failed()), // 源 JsonException
        };

        // 源 Arguments 记录：缺键 → null；jvm/game 缺键 → null；非数组 → JsonException
        let arguments = match obj.get("arguments") {
            None | Some(Value::Null) => None, // 源：Arguments 为 null
            Some(Value::Object(a)) => {
                let jvm = match a.get("jvm") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(items)) => Some(items.clone()),
                    Some(_) => return Err(parse_json_failed()),
                };
                let game = match a.get("game") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(items)) => Some(items.clone()),
                    Some(_) => return Err(parse_json_failed()),
                };
                Some(RawArguments { jvm, game })
            }
            // 源：arguments 为字符串等非对象 → JsonException（B1 的 VersionArguments::Old
            // 形态仅存在于 CompleteVersionMetadata 解析路径，本文件不采用，见日志 p33）
            Some(_) => return Err(parse_json_failed()),
        };

        let minecraft_arguments = match obj.get("minecraftArguments") {
            None | Some(Value::Null) => None, // 源：缺键 → null
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(parse_json_failed()), // 源 JsonException
        };

        Ok(ParsedConfig {
            inherits_from,
            asset_index_id,
            arguments,
            minecraft_arguments,
        })
    }

    /// 读取版本 JSON 文本（源：File.ReadAllText，路径
    /// Path.Combine(_gameDir, "versions", version, "{version}.json")）
    /// 源 IO 异常（文件不存在等）向上传播 → 启动失败；Rust 以 Error::Params 承载（见日志 p33）
    fn read_version_json(&self, game_dir: &str, version: &str) -> Result<String, Error> {
        let path = Path::new(game_dir)
            .join("versions")
            .join(version)
            .join(format!("{version}.json"));
        std::fs::read_to_string(&path).map_err(|e| Error::Params {
            message: format!("读取版本JSON失败: {}", path.display()),
            source: Some(Box::new(e)),
        })
    }
}

/// 离线 UUID（源：NameToUuid，521-538 行，public static）
/// 算法与 util/platform.rs 的 generate_uuid 完全一致（MD5("OfflinePlayer:{name}")，
/// 改写第 6 字节版本位 0x30 与第 8 字节变体位 0x80，小写 hex），直接复用
pub(crate) fn name_to_uuid(name: &str) -> String {
    generate_uuid(name)
}

/// 参数规范化（源：NormalizeArg，786-793 行，实例方法）：空 → ""；Trim；
/// 含空格且首尾均非双引号 → 加双引号
pub(crate) fn normalize_arg(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let value = value.trim();
    if value.contains(' ') && !value.starts_with('"') && !value.ends_with('"') {
        return format!("\"{value}\"");
    }
    value.to_string()
}

/// 是否支持 quickPlay（源：IsQuickPlaySupported，795-801 行，static）
/// assetIndex.Id 可解析为整数且 >= 4；空 / 不可解析 → false
fn is_quick_play_supported(asset_index_id: Option<&str>) -> bool {
    let Some(index_id) = asset_index_id else {
        return false;
    };
    if index_id.is_empty() {
        return false;
    }
    index_id.parse::<i32>().map(|n| n >= 4).unwrap_or(false)
}

/// 解析服务器地址（源：ParseServerAddress，803-813 行，static）
/// 最后一个 ':' 之后可解析为端口 → (server, port)；否则 (address, 25565)
/// ⚠️ 偏差：.NET int.TryParse 容忍首尾空白，Rust parse::<i32> 不允许（理论场景，见日志 p33）
fn parse_server_address(address: &str) -> (String, i32) {
    if let Some(idx) = address.rfind(':') {
        if idx > 0 {
            let server = &address[..idx];
            let port_str = &address[idx + 1..];
            if let Ok(port) = port_str.parse::<i32>() {
                return (server.to_string(), port);
            }
        }
    }
    (address.to_string(), 25565)
}

/// 源 JsonSerializer.Deserialize<Config> 抛 JsonException → Error::Params("版本Json解析失败")
fn parse_json_failed() -> Error {
    Error::Params {
        message: "版本Json解析失败".to_string(),
        source: None,
    }
}

/// 本地应用数据目录（对应 .NET Environment.SpecialFolder.LocalApplicationData）：
/// Windows → %LOCALAPPDATA%；macOS → ~/.local/share；Linux → $XDG_DATA_HOME 或 ~/.local/share
/// ⚠️ UNMAPPED：std 无 SpecialFolder API；按 .NET 官方映射近似

/// Windows 主版本号（对应 .NET Environment.OSVersion.Version.Major）。
///
/// 读注册表 `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 的
/// `CurrentMajorVersionNumber`（REG_DWORD，Win10/11 = 10，Win7 = 6）。
/// 与 scanner.rs 的 reg query 方案一致（零依赖）；非 Windows 或读取失败返回 None。
fn windows_major_version() -> Option<u32> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "/v",
                "CurrentMajorVersionNumber",
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .find(|l| l.contains("CurrentMajorVersionNumber"))?;
        let value = line.split_whitespace().last()?;
        u32::from_str_radix(value.trim_start_matches("0x"), 16).ok()
    }
    #[cfg(not(windows))]
    {
        None
    }
}
