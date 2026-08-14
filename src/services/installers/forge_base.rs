//! Forge 安装器共享基类（B9）
//!
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/ForgeInstallerBase.cs（303 行）
//!
//! 设计决策（详见 b9-logs/p38-forge-base.md）：
//! - `internal abstract class ForgeInstallerBase : InstallerBase` → `pub(crate) struct ForgeInstallerBase`：
//!   实例字段承载状态（源 ForgeInstaller 构造器直接赋值这些 internal 字段，字段全部 pub(crate)）；
//!   "抽象基类"语义不模拟（无虚方法）；Legacy/New 安装分支（源 InstallLegacyForge/InstallForge）
//!   属调用方 forge/install.rs（源 ForgeInstaller.cs，P39 范围），本文件仅承载共享逻辑；
//! - 源实例方法未使用实例状态 → 移植为关联函数（沿用 installer.rs 先例：
//!   `RunInstallProcess` 实例方法 → 静态，见 P35 日志）；
//! - JsonObject/JsonNode → `serde_json::Map<String, Value>` / `Value`；
//! - 源通用 `Exception` → `Error::Params`（校验/数据类错误；下载错误传播
//!   `InstallerBase::download_file_async` 的 `Error::DownloadFailed`，同 fabric/install.rs 错误语义定案）；
//! - InstallerBase 静态工具复用：maven_to_path / download_file_async / get_jar_main_class /
//!   run_install_process / create_http_client；SHA1 计算复用 checksum::sha1_hex（源
//!   `BitConverter.ToString(hash).Replace("-","").ToLower()` 语义）。
//!
//! ⚠️ 任务描述提及的 `ForgeLegacy` 枚举在源中不存在（ForgeInstallerBase.cs 与 ForgeInstaller.cs
//! 均无枚举定义；Legacy/New 判定为 `IsLegacyForgeInstaller` → bool，属 P39 范围），
//! 按"按源"原则未发明枚举。

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Map, Value};

use crate::error::Error;
use crate::services::download::checksum::sha1_hex;
use crate::services::installers::installer::InstallerBase;

/// 源 URL 替换映射条目（源：`internal struct SourcesList { Original; Default; }`）。
pub(crate) struct SourcesList {
    /// 原始地址（源：`Original`）
    pub original: String,
    /// 替换后的默认地址（源：`Default`）
    pub default: String,
}

/// Forge 安装器共享基类（源：`internal abstract class ForgeInstallerBase : InstallerBase`）。
///
/// "抽象"语义由调用方（forge/install.rs）决定；本结构体承载 Forge 安装器共有的
/// 实例状态与共享逻辑（URL 解析、processor 执行、Maven 坐标/路径解析、参数替换等）。
pub(crate) struct ForgeInstallerBase {
    /// 下载源基地址（源：`BaseUrl`，可能含 `|` 分隔的多地址）
    pub base_url: String,
    /// 下载源编号（源：`SourceId`，0=官方，1=BMCLAPI 镜像）
    pub source_id: i32,
    /// Minecraft 根目录（源：`gameDir`）
    pub game_dir: String,
    /// 游戏版本（源：`gameVersion`）
    pub game_version: String,
    /// Forge 安装器文件路径（源：`_installerPath`）
    pub installer_path: String,
    /// 主版本 jar 路径（源：`_mainJarPath`）
    pub main_jar_path: String,
    /// 源 URL 替换映射列表（源：`SourceMappings`）
    pub source_mappings: Vec<SourcesList>,
}

impl ForgeInstallerBase {
    /// 按 SourceMappings 替换下载源 URL（源：`ResolveUrl(string originalUrl)`）。
    ///
    /// 命中首个 `Original == originalUrl` 的映射返回其 `Default`，否则原样返回
    /// （对应 `FirstOrDefault(...)` 后 `mapping.Default ?? originalUrl`）。
    pub(crate) fn resolve_url(&self, original_url: &str) -> String {
        self.source_mappings
            .iter()
            .find(|m| m.original == original_url)
            .map(|m| m.default.clone())
            .unwrap_or_else(|| original_url.to_string())
    }

    /// 执行单个 Forge processor（源：`RunProcessor(JsonObject ipObj, JsonObject processor,
    /// string versionId, string gameDir, string javaPath)`，async）。
    ///
    /// 逻辑要点（逐字保留源）：
    /// 1. 源 `if (processor == null) return;`：Rust `&Map` 不可能为空，省略（见日志 U2）；
    /// 2. 先经 `ReplaceOutputs` 生成输出表：任一输出文件已存在且 SHA1 匹配 → 提前成功返回
    ///    （处理器幂等判定，跳过执行）；
    /// 3. `processor.jar`（Maven 坐标）解析本地库路径，缺失则按
    ///    `{BaseUrl}/{group→/}/{artifact}/{version}/{artifact}-{version}.jar` 下载
    ///    （每次新建 HttpClient，同源 `DownloadFileAsync(CreateHttpClient(), ...)`）；
    /// 4. `classpath` 数组逐项同规则下载并拼接 classpath 串（分隔符：Windows `;`，其余 `:`；
    ///    尾部 `;`/`:` 去除，对应 `TrimEnd(';', ':')`）；
    /// 5. `args` 数组空格拼接（`TrimEnd(' ')`）→ `ReplaceArguments` 占位符/内联坐标替换；
    /// 6. `GetJarMainClass` 取 jar 的 Main-Class（空 → 报错）→
    ///    `RunInstallProcess("-cp \"{cps}{sep}{jar}\" {main} {args}", javaPath)`；
    /// 7. 退出码非 0 → 报错（含命令与退出码）；执行后重新校验输出文件存在 + SHA1 匹配。
    ///
    /// ⚠️ UNMAPPED U1：源 `string processorJar = processor["jar"]?.ToString() ?? "未知Jar";`
    /// 赋值后从未使用（dead code）→ 省略。
    /// ⚠️ `versionId` 参数源方法体内未使用，保留以对齐源签名（`_version_id`）。
    /// 错误映射：校验类 → `Error::Params`；下载失败 → 传播 `Error::DownloadFailed`。
    pub(crate) async fn run_processor(
        &self,
        ip_obj: &Map<String, Value>,
        processor: &Map<String, Value>,
        _version_id: &str,
        game_dir: &str,
        java_path: &str,
    ) -> Result<(), Error> {
        // 源：`string separator = OperatingSystem.IsWindows() ? ";" : ":";`
        let separator = if cfg!(target_os = "windows") { ";" } else { ":" };

        // 源：输出文件已存在且 SHA1 匹配 → 处理器已执行过，提前返回
        let output_paths = self.replace_outputs(ip_obj, processor, game_dir)?;
        for (key, value) in &output_paths {
            let file_path = self.resolve_processor_output_path(key)?;
            let file_sha1 = value.trim_matches('\'');
            if Path::new(&file_path).is_file() && Self::verify_file_sha1(&file_path, file_sha1) {
                return Ok(());
            }
        }

        // 源：`var jar = processor["jar"]?.ToString(); if (string.IsNullOrEmpty(jar)) throw ...`
        let jar = processor.get("jar").map(node_to_string).unwrap_or_default();
        if jar.is_empty() {
            return Err(Error::Params {
                message: "Processor Jar路径未定义".to_string(),
                source: None,
            });
        }
        // 源：`var jarParts = jar.Split(':'); if (jarParts.Length < 3) throw ...`
        let jar_parts: Vec<&str> = jar.split(':').collect();
        if jar_parts.len() < 3 {
            return Err(Error::Params {
                message: format!("Processor Jar格式错误: {jar}"),
                source: None,
            });
        }

        // 源：缺失时下载 `{BaseUrl}/{group→/}/{artifact}/{version}/{artifact}-{version}.jar`
        let jar_path = Self::resolve_library_path(game_dir, &jar)?;
        if !Path::new(&jar_path).is_file() {
            let download_url = format!(
                "{}/{}/{}/{}/{}-{}.jar",
                self.base_url,
                jar_parts[0].replace('.', "/"),
                jar_parts[1],
                jar_parts[2],
                jar_parts[1],
                jar_parts[2]
            );
            let client = InstallerBase::create_http_client();
            InstallerBase::download_file_async(&client, &download_url, &jar_path, 5).await?;
        }

        // 源：classpath 数组逐项解析/下载并拼接（尾部分隔符去除）
        let mut cps = String::new();
        if let Some(classpath_arr) = processor.get("classpath").and_then(|v| v.as_array()) {
            for cp in classpath_arr {
                let cp_str = node_to_string(cp);
                let cp_parts: Vec<&str> = cp_str.split(':').collect();
                if cp_parts.len() < 3 {
                    return Err(Error::Params {
                        message: format!("Classpath格式错误: {cp_str}"),
                        source: None,
                    });
                }
                let cp_jar_path = Self::resolve_library_path(game_dir, &cp_str)?;
                if !Path::new(&cp_jar_path).is_file() {
                    let download_url = format!(
                        "{}/{}/{}/{}/{}-{}.jar",
                        self.base_url,
                        cp_parts[0].replace('.', "/"),
                        cp_parts[1],
                        cp_parts[2],
                        cp_parts[1],
                        cp_parts[2]
                    );
                    let client = InstallerBase::create_http_client();
                    InstallerBase::download_file_async(&client, &download_url, &cp_jar_path, 5)
                        .await?;
                }
                cps.push_str(&cp_jar_path);
                cps.push_str(separator);
            }
            // 源：`cps = cps.TrimEnd(';', ':');`
            cps = cps.trim_end_matches([';', ':']).to_string();
        }

        // 源：args 数组空格拼接 → `TrimEnd(' ')` → ReplaceArguments
        let mut args = String::new();
        if let Some(args_arr) = processor.get("args").and_then(|v| v.as_array()) {
            for arg in args_arr {
                args.push_str(&node_to_string(arg));
                args.push(' ');
            }
            args = args.trim_end_matches(' ').to_string();
            args = self.replace_arguments(ip_obj, &args)?;
        }

        // 源：`var mainClass = GetJarMainClass(jarPath); if (string.IsNullOrEmpty(mainClass)) throw ...`
        let main_class = InstallerBase::get_jar_main_class(&jar_path)?;
        if main_class.is_empty() {
            return Err(Error::Params {
                message: format!("无法获取Jar主类: {jar_path}"),
                source: None,
            });
        }

        // 源：`string command = $"-cp \"{cps}{separator}{jarPath}\" {mainClass} {args}";`
        let command = format!("-cp \"{cps}{separator}{jar_path}\" {main_class} {args}");
        let exit_code = InstallerBase::run_install_process(&command, Some(java_path));
        if exit_code != 0 {
            return Err(Error::Params {
                message: format!("Processor执行失败，命令: {java_path} {command}\nExit code:{exit_code}"),
                source: None,
            });
        }

        // 源：执行后重新校验输出文件存在 + SHA1 匹配
        let output_paths = self.replace_outputs(ip_obj, processor, game_dir)?;
        for (key, value) in &output_paths {
            let file_path = self.resolve_processor_output_path(key)?;
            let file_sha1 = value.trim_matches('\'');
            if !Path::new(&file_path).is_file() {
                return Err(Error::Params {
                    message: format!("Processor执行失败: 输出文件不存在 - {file_path}"),
                    source: None,
                });
            }
            if !Self::verify_file_sha1(&file_path, file_sha1) {
                return Err(Error::Params {
                    message: format!("输出文件SHA1不匹配: {file_path}"),
                    source: None,
                });
            }
        }
        Ok(())
    }

    /// 校验文件 SHA1（源：`internal static bool VerifyFileSha1(string filePath,
    /// string expectedHash)`）。
    ///
    /// 文件不存在 → false（源先判 `File.Exists`）；实际哈希为小写十六进制
    /// （源 `BitConverter.ToString(hash).Replace("-", "").ToLower()`，复用 checksum::sha1_hex），
    /// 与期望值两侧 Trim 后忽略大小写比较（源 `OrdinalIgnoreCase`）。
    /// ⚠️ UNMAPPED U6：源 `File.OpenRead` 失败抛异常；此处遵循 fabric::install::verify_file_sha1
    /// 先例，IO 失败记日志并返回 false（调用方均先经 File.Exists 前置检查，实际不可达）。
    pub(crate) fn verify_file_sha1(file_path: &str, expected_hash: &str) -> bool {
        if !Path::new(file_path).is_file() {
            return false;
        }
        let bytes = match std::fs::read(file_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("SHA1 校验读取文件失败：{e}");
                return false;
            }
        };
        sha1_hex(&bytes)
            .trim()
            .eq_ignore_ascii_case(expected_hash.trim())
    }

    /// HEAD 探测 URL 是否可用（源：`internal static async Task<bool>
    /// IsFileUrlAvailableAsync(string url, int timeoutSeconds = 10)`）。
    ///
    /// - 基础校验：URL 非空白且为绝对 URI（源 `Uri.IsWellFormedUriString(url, UriKind.Absolute)`）
    ///   ⚠️ UNMAPPED U3：以 `reqwest::Url::parse` 成功 + http/https scheme 检查近似；
    /// - 独立 HttpClient 并设置请求超时（源 `client.Timeout = TimeSpan.FromSeconds(...)`）；
    /// - HEAD 请求返回 2xx → true；任何异常（网络/超时/非 2xx 非异常情形）→ false
    ///   （源 catch 全部异常 → false；`IsSuccessStatusCode` 仅 2xx）。
    /// - ⚠️ UNMAPPED U10：源默认参数 `timeoutSeconds = 10` → Rust 显式传参，调用方传 10。
    pub(crate) async fn is_file_url_available_async(url: &str, timeout_seconds: u64) -> bool {
        if url.trim().is_empty() {
            return false;
        }
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return false;
        };
        if !(parsed.scheme() == "http" || parsed.scheme() == "https") {
            return false;
        }
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_seconds))
            .build()
        else {
            return false;
        };
        let Ok(response) = client.head(url).send().await else {
            return false;
        };
        response.status().is_success()
    }

    /// Maven 坐标解析为本地库路径（源：`internal string ResolveLibraryPath(string gameDir,
    /// string mavenCoordinate)`）。
    ///
    /// `MavenToPath`（复用 InstallerBase::maven_to_path）结果为空 → 报错（源 throw
    /// `无效的Maven坐标`）；`/` 替换为平台目录分隔符（源
    /// `relativePath.Replace('/', Path.DirectorySeparatorChar)`）后拼
    /// `{gameDir}/libraries/{path}`（源 `Path.Combine(gameDir, "libraries", ...)`）。
    /// 源为实例方法但未用实例状态 → 移植为关联函数。
    pub(crate) fn resolve_library_path(
        game_dir: &str,
        maven_coordinate: &str,
    ) -> Result<String, Error> {
        let relative_path = InstallerBase::maven_to_path(maven_coordinate);
        if relative_path.trim().is_empty() {
            return Err(Error::Params {
                message: format!("无效的Maven坐标: {maven_coordinate}"),
                source: None,
            });
        }
        let normalized_relative_path = relative_path.replace('/', &std::path::MAIN_SEPARATOR.to_string());
        Ok(join_path(
            &join_path(game_dir, "libraries"),
            &normalized_relative_path,
        ))
    }
    /// 解析 processor 输出键为本地文件路径（源：`internal string
    /// ResolveProcessorOutputPath(string outputKey)`）。
    ///
    /// - 空 → 空字符串（源直接返回 string.Empty）；
    /// - 绝对路径（源 `Path.IsPathRooted` → Rust `Path::has_root()`）：前缀必须与 gameDir
    ///   忽略大小写一致（源 `StartsWith(this.gameDir, OrdinalIgnoreCase)`），否则报错
    ///   `Forge处理器输出路径越界`（⚠️ UNMAPPED U7：`eq_ignore_ascii_case` 近似 OrdinalIgnoreCase）；
    /// - 相对形态：剥除两端 `[`/`]`（源 `TrimEnd(']').TrimStart('[')`）后按 Maven 坐标解析，
    ///   无效坐标原样返回；否则拼 `{gameDir}/libraries/{path}`。
    pub(crate) fn resolve_processor_output_path(&self, output_key: &str) -> Result<String, Error> {
        if output_key.is_empty() {
            return Ok(String::new());
        }
        if Path::new(output_key).has_root() {
            if !output_key
                .get(..self.game_dir.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(&self.game_dir))
            {
                return Err(Error::Params {
                    message: format!("Forge处理器输出路径越界: {output_key}"),
                    source: None,
                });
            }
            return Ok(output_key.to_string());
        }
        let raw_key = output_key.trim_end_matches(']').trim_start_matches('[');
        let lib_maven_path = InstallerBase::maven_to_path(raw_key);
        if lib_maven_path.is_empty() {
            return Ok(output_key.to_string());
        }
        let lib_maven_path = lib_maven_path.replace('/', &std::path::MAIN_SEPARATOR.to_string());
        Ok(join_path(
            &join_path(&self.game_dir, "libraries"),
            &lib_maven_path,
        ))
    }

    /// 生成 processor 输出映射（源：`internal Dictionary<string, string> ReplaceOutputs(
    /// JsonObject ipObj, JsonObject processor, string gameDir)`）。
    ///
    /// `processor.outputs` 缺失 → 空表；每项 `{key}={value}` 先经 `ReplaceArguments`
    /// 整体替换，再 `Split('=')`——恰好 2 段才收录（其余丢弃，源 `if (splitArr.Length != 2)
    /// continue;`）。Rust `str::split` 保留空段，与 C# 默认 Split 行为一致（如 `=x` 收录空键）。
    /// ⚠️ 源 `outputs` 非对象时 `AsObject()` 抛 InvalidOperationException → Rust
    /// `as_object()` None 跳过（UNMAPPED U5，防御性收敛）。
    /// ⚠️ `gameDir` 参数源方法体内未使用，保留以对齐源签名（`_game_dir`）。
    pub(crate) fn replace_outputs(
        &self,
        ip_obj: &Map<String, Value>,
        processor: &Map<String, Value>,
        _game_dir: &str,
    ) -> Result<HashMap<String, String>, Error> {
        let mut outputs = HashMap::new();
        let Some(outputs_obj) = processor.get("outputs").and_then(|v| v.as_object()) else {
            return Ok(outputs);
        };
        for (key, value) in outputs_obj {
            let replaced_str = self.replace_arguments(ip_obj, &format!("{key}={}", node_to_string(value)))?;
            let split_arr: Vec<&str> = replaced_str.split('=').collect();
            if split_arr.len() != 2 {
                continue;
            }
            outputs.insert(split_arr[0].to_string(), split_arr[1].to_string());
        }
        Ok(outputs)
    }

    /// 替换参数中的占位符（源：`internal string ReplaceArguments(JsonObject ipObj,
    /// string args)`）。
    ///
    /// 依次执行三类替换：
    /// 1. `ipObj.data` 各属性：取其 `client` 值（非空时）替换 `{属性名}` 占位符，
    ///    值先经 `NormalizeProcessorValue`（`[maven]` → 本地库路径，`'` 剥离）；
    /// 2. 固定占位符：`{MINECRAFT_VERSION}` → gameVersion；`{MINECRAFT_JAR}` → mainJarPath
    ///    （绝对路径原样，否则 `Path.Combine(gameDir, ...)`）；`{ROOT}` → gameDir；
    ///    `{LIBRARY_DIR}` → `{gameDir}/libraries`（源字面量使用 `/`，保留）；`{INSTALLER}` →
    ///    installerPath；`{SIDE}` → "client"；
    /// 3. `ReplaceInlineMavenCoordinates` 替换 `[...]` 内联坐标。
    ///
    /// `Contains` 判断后替换（源两处均先判断再 Replace，全量替换语义同 `str::replace`）。
    /// ⚠️ 源 `prop.Value?["client"]` 对非对象抛 InvalidOperationException → Rust `.get()`
    /// 返回 None 跳过（UNMAPPED U4）。
    pub(crate) fn replace_arguments(
        &self,
        ip_obj: &Map<String, Value>,
        args: &str,
    ) -> Result<String, Error> {
        let mut args = args.to_string();
        if let Some(data_obj) = ip_obj.get("data").and_then(|v| v.as_object()) {
            for (name, value) in data_obj {
                if let Some(client_value) = value.get("client") {
                    let value_str = node_to_string(client_value);
                    if !value_str.is_empty() {
                        let placeholder = format!("{{{name}}}");
                        if args.contains(&placeholder) {
                            let replacement = self.normalize_processor_value(&value_str)?;
                            args = args.replace(&placeholder, &replacement);
                        }
                    }
                }
            }
        }

        // 源：固定占位符替换字典（保持字典序执行无依赖）
        let minecraft_jar = if Path::new(&self.main_jar_path).has_root() {
            self.main_jar_path.clone()
        } else {
            join_path(&self.game_dir, &self.main_jar_path)
        };
        let replacements = [
            ("{MINECRAFT_VERSION}", self.game_version.clone()),
            ("{MINECRAFT_JAR}", minecraft_jar),
            ("{ROOT}", self.game_dir.clone()),
            ("{LIBRARY_DIR}", format!("{}/libraries", self.game_dir)),
            ("{INSTALLER}", self.installer_path.clone()),
            ("{SIDE}", "client".to_string()),
        ];
        for (key, value) in replacements {
            if args.contains(key) {
                args = args.replace(key, &value);
            }
        }

        args = self.replace_inline_maven_coordinates(&args)?;
        Ok(args)
    }

    /// 规范化 processor 数据值（源：`internal string NormalizeProcessorValue(string value)`）。
    ///
    /// 空白 → 原样返回；`[maven坐标]` 形态（首尾方括号）→ `ResolveLibraryPath(gameDir, 坐标)`
    /// （源 `value[1..^1]` 剥首尾；坐标非法 → 报错传播）；其余 → 剥离全部首尾单引号
    /// （源 `value.Trim('\'')`）。
    pub(crate) fn normalize_processor_value(&self, value: &str) -> Result<String, Error> {
        if value.trim().is_empty() {
            return Ok(value.to_string());
        }
        if value.starts_with('[') && value.ends_with(']') {
            let maven_coordinate = &value[1..value.len() - 1];
            return Self::resolve_library_path(&self.game_dir, maven_coordinate);
        }
        Ok(value.trim_matches('\'').to_string())
    }

    /// 替换字符串中的内联 Maven 坐标（源：`internal string
    /// ReplaceInlineMavenCoordinates(string value)`）。
    ///
    /// 正则 `\[(.+?)\]`（源 `"\\[(.+?)\\]"`，非贪婪）匹配全部内联坐标，逐个替换为
    /// `ResolveLibraryPath(gameDir, 组1)` 结果（源 `Replace(match.Value, replacement, Ordinal)`
    /// 全局替换，Rust `str::replace` 同语义；遍历基于原始 value 的匹配集，替换作用于 result，
    /// 与源一致——替换结果中新增的 `[...]` 不会被再次匹配）。坐标非法 → 报错传播（源 throw）。
    pub(crate) fn replace_inline_maven_coordinates(&self, value: &str) -> Result<String, Error> {
        if value.trim().is_empty() {
            return Ok(value.to_string());
        }
        let mut result = value.to_string();
        let bracket_re = bracket_regex();
        for capture in bracket_re.captures_iter(value) {
            let coordinate = capture
                .get(1)
                .map(|g| g.as_str())
                .unwrap_or_default();
            let replacement = Self::resolve_library_path(&self.game_dir, coordinate)?;
            let matched = capture
                .get(0)
                .map(|g| g.as_str())
                .unwrap_or_default();
            result = result.replace(matched, &replacement);
        }
        Ok(result)
    }

    /// 收集 processors 中 args 的内联 Maven 坐标（源：`internal List<string>
    /// ExtractMavenCoordinatesFromProcessors(JsonObject installProfileJson)`）。
    ///
    /// `processors` 数组逐项（仅 JsonObject，其余跳过，源 `OfType<JsonObject>()`）；
    /// 每项 `args` 数组逐元素正则 `\[(.+?)\]` 提取组 1。源为实例方法但未用实例状态 →
    /// 移植为关联函数。
    pub(crate) fn extract_maven_coordinates_from_processors(
        install_profile_json: &Map<String, Value>,
    ) -> Vec<String> {
        let mut coordinates = Vec::new();
        let Some(processors) = install_profile_json
            .get("processors")
            .and_then(|v| v.as_array())
        else {
            return coordinates;
        };
        let bracket_re = bracket_regex();
        for processor in processors.iter().filter_map(|p| p.as_object()) {
            let Some(args) = processor.get("args").and_then(|v| v.as_array()) else {
                continue;
            };
            for arg in args {
                let text = node_to_string(arg);
                if text.trim().is_empty() {
                    continue;
                }
                for capture in bracket_re.captures_iter(&text) {
                    if let Some(group) = capture.get(1) {
                        coordinates.push(group.as_str().to_string());
                    }
                }
            }
        }
        coordinates
    }

    /// 判断 processor 是否适用于指定 side（源：`internal bool ShouldRunProcessor(
    /// JsonObject processor, string side)`）。
    ///
    /// `sides` 缺失或为空数组 → true（源 `sides == null || sides.Count == 0`）；否则
    /// 任一元素与 side 忽略大小写相等（源 `string.Equals(v, side, OrdinalIgnoreCase)`）→ true。
    /// 源为实例方法但未用实例状态 → 移植为关联函数。
    pub(crate) fn should_run_processor(
        processor: &Map<String, Value>,
        side: &str,
    ) -> bool {
        let Some(sides) = processor.get("sides").and_then(|v| v.as_array()) else {
            return true;
        };
        if sides.is_empty() {
            return true;
        }
        sides.iter().any(|t| node_to_string(t).eq_ignore_ascii_case(side))
    }
}

/// 内联 Maven 坐标正则 `\[(.+?)\]`（源 `Regex.Matches(value, "\\[(.+?)\\]")`）。
fn bracket_regex() -> &'static Regex {
    static BRACKET_RE: OnceLock<Regex> = OnceLock::new();
    BRACKET_RE.get_or_init(|| Regex::new(r"\[(.+?)\]").expect("静态正则编译失败"))
}

/// 模拟 C# `JsonNode.ToString()`：`JsonValue(string).ToString()` 返回不带引号的原始字符串，
/// 其余节点（数字/布尔/对象/数组）返回其 JSON 序列化文本；JSON null 节点映射为空字符串
/// （对应源 `?.ToString()` 的可空传播语义——null 节点各处均被 `IsNullOrEmpty` 跳过）。
fn node_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 模拟 C# `Path.Combine(a, b)` 的文件系统拼接（等效 `PathBuf::join`）。
///
/// 源使用 Path.Combine 处均为本地文件路径（非 URL），join 语义一致；返回 String
/// （本代码库路径一律 String，同 fabric/install.rs 约定）。
fn join_path(a: &str, b: &str) -> String {
    Path::new(a).join(b).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::installers::installer::InstallerBase;

    fn base_with(game_dir: &str) -> ForgeInstallerBase {
        ForgeInstallerBase {
            base_url: "https://maven.minecraftforge.net".to_string(),
            source_id: 0,
            game_dir: game_dir.to_string(),
            game_version: "26.2".to_string(),
            installer_path: "C:/tmp/forge-26.2-65.1.1-installer.jar".to_string(),
            main_jar_path: "versions/26.2/26.2.jar".to_string(),
            source_mappings: Vec::new(),
        }
    }

    /// install_profile.json 的 data 段（对齐 Forge 1.21.x / 26.2-65.1.1）
    fn install_profile_with_data() -> serde_json::Value {
        serde_json::json!({
            "path": "net.minecraftforge:forge:26.2-65.1.1:shim",
            "data": {
                "PATCHED": { "client": "[net.minecraftforge:forge:26.2-65.1.1:client]" },
                "PATCHED_SHA": { "client": "'f9ef709fa7988febfca4c91b87d3d1ad8a438097'" },
                "MC_UNPACKED": { "client": "[net.minecraft:client:26.2]" },
                "MINECRAFT_JAR": { "client": null }
            }
        })
    }

    #[test]
    fn replace_arguments_with_populated_game_dir_resolves_patched_to_absolute_path() {
        // 回归：基类 game_dir 必须被填充，否则 `{PATCHED}`（内联 Maven 坐标
        // `[net.minecraftforge:forge:26.2-65.1.1:client]`）会解析成相对 `libraries\…`，
        // 导致 binarypatcher 的 --output 指向坏路径、processor 退出码 1。
        let base = base_with(r"C:\Games\.minecraft");
        let ip_obj = install_profile_with_data();
        let ip_map = ip_obj.as_object().expect("object");

        let resolved =
            base.replace_arguments(ip_map, &format!("{{PATCHED}}={{PATCHED_SHA}}"))
                .expect("replace_arguments 不应失败");

        let expected_abs = join_path(
            &join_path(r"C:\Games\.minecraft", "libraries"),
            &InstallerBase::maven_to_path("net.minecraftforge:forge:26.2-65.1.1:client")
                .replace('/', &std::path::MAIN_SEPARATOR.to_string()),
        );
        assert!(
            resolved.starts_with(&expected_abs),
            "应解析为绝对库路径，got: {resolved}"
        );
        // 不应是相对 `libraries\…`（修复前空 game_dir 的错误形态）
        assert!(
            !resolved.starts_with("libraries\\net\\"),
            "不应是相对 libraries 路径: {resolved}"
        );
    }

    #[test]
    fn output_key_resolves_to_absolute_with_populated_game_dir_not_reparsed() {
        // 回归核心：`resolve_processor_output_path` 对二进制补丁输出键在此前空 game_dir 下
        // 拿到的是“相对 libraries 路径”，随后把它当 Maven 坐标再喂给 maven_to_path →
        // 日志 `无效的Maven坐标格式：libraries\net\…client.jar，至少需要3个部分`，
        // 且 --output 指向无盘符的坏路径。修复后 game_dir 非空 → 输出键为绝对路径，
        // resolve_processor_output_path 走 rooted 分支、不再触发该报错。
        let processed_key =
            base_with("")
                .replace_arguments(
                    install_profile_with_data().as_object().expect("object"),
                    "{PATCHED}".into(),
                )
                .expect("resolve");
        // 修复前形态：空 game_dir 得到相对 `libraries\net\…`
        assert!(
            processed_key.starts_with("libraries\\net\\minecraftforge\\forge\\"),
            "空 game_dir 应产生相对 libraries 路径：{processed_key}"
        );
        // 修复后形态：game_dir 非空 → 输出键为绝对、rooted，可直接归属 gameDir 前缀
        let populated_key = base_with(r"C:\Games\.minecraft")
            .replace_arguments(
                install_profile_with_data().as_object().expect("object"),
                "{PATCHED}".into(),
            )
            .expect("resolve");
        assert!(Path::new(&populated_key).is_absolute(), "应为绝对路径：{populated_key}");
        assert!(
            populated_key
                .get(..r"C:\Games\.minecraft".len())
                .is_some_and(|h| h.eq_ignore_ascii_case(r"C:\Games\.minecraft")),
            "应以 gameDir 为前缀：{populated_key}"
        );
    }
}

