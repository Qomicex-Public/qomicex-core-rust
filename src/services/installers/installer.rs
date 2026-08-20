//! 安装器基础契约（B9）
//!
//! 对应源文件（Qomicex.Core.AOT/Services/Installers/）：
//! - IInstaller.cs：`IInstaller` 接口 → `Installer` trait
//! - InstallerBase.cs：`InstallerBase` 抽象基类（静态工具）+ `InstallType` 枚举
//! - MissFileData.cs：`MissFileData` record → struct
//!
//! 设计决策（详见 b9-logs/p35-installer-base.md）：
//! - `abstract class`（子类继承静态工具）→ 空结构体 `InstallerBase` + 全静态方法，
//!   安装器实现经 `InstallerBase::xxx()` 调用；
//! - `Trace.WriteLine` → `eprintln!`（与 util/lib_helper.rs、util/file_helper.rs 约定一致）；
//! - `InstallerBase.MavenToPath` 与 `Utils/LibHelper.cs::MavenToPath` 实现重复 →
//!   委托 `util::lib_helper::maven_to_path`（去重，见日志决策 D3）；
//! - 本模块全部 `pub(crate)`：services/ 实现层不对外（见 services/mod.rs）。

use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::error::Error;
use crate::net::NetworkConfig;
use crate::util::lib_helper;

/// 安装器接口（源：`IInstaller` 接口，IInstaller.cs）。
///
/// 各 ModLoader/整合包安装器（Forge/Fabric/Quilt/NeoForge/...）实现此 trait，
/// 提供安装与缺失库查询两个能力。
#[async_trait]
pub trait Installer: Send + Sync {
    /// 执行安装（源：`Task InstallAsync(string versionId, string inheritsFromJson,
    /// string? para1, string? para2, string? para3, string? para4)`）。
    ///
    /// C# 可空 `string?` 参数 → `Option<&str>`；`version_id` / `inherits_from_json`
    /// 为必填（源非可空）。
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
        para4: Option<&str>,
    ) -> Result<(), Error>;

    /// 获取缺失库列表（源：`Task<List<MissFileData>> GetMissLibrariesAsync(
    /// string? para1, string? para2, string? para3)`）。
    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error>;
}

/// 缺失文件信息（源：`MissFileData` record，MissFileData.cs，字段 Name/Path/Url/Sha1）。
///
/// ⚠️ 与 `models::installer::MissFileInfo`（源 MissFileInfo.cs，字段同名同构）重复，
/// 本批按任务要求独立移植；后续批次可评估合并。
#[derive(Debug, Clone, PartialEq)]
pub struct MissFileData {
    /// 文件名（源：`Name`）
    pub name: String,
    /// 相对路径（源：`Path`）
    pub path: String,
    /// 下载地址（源：`Url`）
    pub url: String,
    /// SHA1 校验值（源：`Sha1`）
    pub sha1: String,
}


/// 全局默认 User-Agent（源：`InstallerBase.DefaultUserAgent` 静态属性）。
///
/// C# 为进程级可读写属性（可重复赋值）→ Rust `static OnceLock<String>`：
/// 写入仅生效一次（首次 `set_default_user_agent`），后续调用被忽略（见日志 D2）。
static DEFAULT_USER_AGENT: OnceLock<String> = OnceLock::new();

/// 安装器基础工具（源：`InstallerBase` 抽象基类）。
///
/// C# 侧为抽象基类，子类继承其静态工具；Rust 无继承 → 空结构体 + 全静态方法。
pub(crate) struct InstallerBase;

impl InstallerBase {
    /// 设置全局默认 User-Agent（源：`DefaultUserAgent` setter）。
    ///
    /// 幂等：仅首次调用生效，后续调用忽略（OnceLock 语义；源为可重写属性，
    /// 差异见日志 D2）。
    pub(crate) fn set_default_user_agent(user_agent: impl Into<String>) {
        let _ = DEFAULT_USER_AGENT.set(user_agent.into());
    }

    /// 创建 HTTP 客户端（源：`CreateHttpClient()`）。
    ///
    /// - 已设置全局默认 User-Agent 则携带（源仅在 `DefaultUserAgent` 非空时添加）；
    /// - 重定向策略 `Policy::limited(50)`：对齐 .NET HttpClient 默认
    ///   `MaxAutomaticRedirections = 50`（reqwest 默认为 10）；
    /// - 应用全局 proxy/TLS 配置（`build()` 经 CoreOptions 写入，见 net.rs）；
    /// - 构建失败（源构造不抛异常）→ `expect` 直接中止。
    pub(crate) fn create_http_client() -> reqwest::Client {
        let mut builder = reqwest::Client::builder();
        if let Some(user_agent) = DEFAULT_USER_AGENT.get() {
            builder = builder.user_agent(user_agent);
        }
        builder = NetworkConfig::global().apply(builder);
        builder
            .redirect(reqwest::redirect::Policy::limited(50))
            .build()
            .expect("创建 HTTP 客户端失败（源 HttpClient 构造不抛异常）")
    }

    /// 合并两份版本 JSON（源：`MergeJson(mainVersionJson, mergedVersionJson)`）。
    ///
    /// 深合并语义（源私有 `Merge` 方法）：
    /// - 键冲突且双方均为对象 → 递归合并；
    /// - 键冲突且双方均为数组 → 源数组项追加到目标数组（拼接）；
    /// - 其余键冲突 → 源值覆盖目标值；
    /// - 任一 JSON 解析失败或非对象 → 返回空字符串（源 catch → string.Empty；
    ///   源 `AsObject()` 对非对象抛 InvalidOperationException，同样落入 catch）。
    ///
    /// ⚠️ 源私有 `Merge` 的 `comparison = OrdinalIgnoreCase` 参数在方法体内未被使用
    /// （`JsonObject.TryGetPropertyValue` 按字典默认比较器精确匹配键名），移植忽略该参数。
    pub(crate) fn merge_json(main_version_json: &str, merged_version_json: &str) -> String {
        let Ok(main_json) = serde_json::from_str::<Value>(main_version_json) else {
            return String::new();
        };
        let Ok(merged_json) = serde_json::from_str::<Value>(merged_version_json) else {
            return String::new();
        };
        if !main_json.is_object() || !merged_json.is_object() {
            return String::new();
        }
        let result = merge_value(main_json, merged_json);
        serde_json::to_string(&result).unwrap_or_default()
    }

    /// 将 merged 版本目录整体合并进 main 版本目录（源：`MergeDirectories`）。
    ///
    /// 语义：源目录不存在 → 直接成功返回；自动创建目标目录；文件覆盖复制
    /// （对应 `File.Copy(file, dest, true)`，`std::fs::copy` 截断覆盖）；
    /// 子目录递归合并。任何 IO 失败向上返回（源无 try/catch，异常上抛，

    /// 合并版本 JSON 并重写 id/inheritsFrom（源：`MergeVersionJson`）。
    ///
    /// 先 `MergeJson` 合并，再移除 `inheritsFrom` 键；`default_version_id` 非空时
    /// 覆盖 `id` 键。⚠️ 源在 MergeJson 失败（返回空串）后 `JsonNode.Parse(...)!`
    /// 抛异常，Rust 侧收敛为返回空字符串（差异见日志 D6）。
    pub(crate) fn merge_version_json(
        main_version_json: &str,
        merged_version_json: &str,
        default_version_id: Option<&str>,
    ) -> String {
        let json_data = Self::merge_json(main_version_json, merged_version_json);
        let Ok(mut json) = serde_json::from_str::<Value>(&json_data) else {
            return String::new();
        };
        if let Some(obj) = json.as_object_mut() {
            obj.remove("inheritsFrom");
            if let Some(id) = default_version_id {
                obj.insert("id".to_string(), Value::String(id.to_string()));
            }
        }
        serde_json::to_string(&json).unwrap_or_default()
    }

    /// 将 mergedVersion 的 JSON 与目录合并进 mainVersion（源：`MergeVersion`）。
    ///
    /// 语义：main 版本 JSON 不存在 → 记日志并返回 false；merged JSON 不存在 →
    /// false；合并 JSON 后重写 `id = mainVersion`、移除 `inheritsFrom`；
    /// 再将 merged 版本目录整体合并进 main 目录；最后写回 main 版本 JSON。

    /// 下载文件到本地（源：`DownloadFileAsync(client, url, destinationPath,
    /// maxRedirects = 5)`）。
    ///
    /// 语义（与源书写逻辑一致）：
    /// - `max_redirects <= 0` → 报错「超过最大重定向次数（{n}次）」；
    /// - 手动跟随重定向（301/302/303/307；.NET 中 302 的 Found/Redirect 为同一值，
    ///   源重复列出），每次递减计数，递归重试；
    /// - 重定向地址可为相对 URL（基于原 URL 解析，对应 `Uri.TryCreate`）；
    /// - 自动创建目标目录（`Path.GetDirectoryName` 非空时）；按响应头开始流式
    ///   写入（对应 `HttpCompletionOption.ResponseHeadersRead`）；
    /// - 非 2xx 状态码报错（对应 `EnsureSuccessStatusCode`）；
    /// - 所有失败包装为「下载文件失败（{url}）：{原因}」。
    ///
    /// ⚠️ 重定向行为说明：reqwest 默认客户端自动跟随重定向（`create_http_client`
    /// 为 limited(50)），此时本方法的手动重定向分支与源默认客户端行为一致地不可达
    /// （.NET 默认 HttpClient 亦自动跟随）；仅当调用方传入
    /// `Policy::none()` 客户端时手动逻辑生效（同源）。
    /// ⚠️ 源递归失败会嵌套包装错误前缀，Rust 侧单层包装（见日志 D7）。
    pub(crate) async fn download_file_async(
        client: &reqwest::Client,
        url: &str,
        destination_path: &str,
        max_redirects: i32,
    ) -> Result<bool, Error> {
        if max_redirects <= 0 {
            return Err(Error::DownloadFailed {
                message: format!("超过最大重定向次数（{max_redirects}次）"),
                source: None,
            });
        }

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("下载文件失败（{url}）：{e}"),
                source: Some(Box::new(e)),
            })?;

        if is_redirect_status(response.status()) {
            let redirect_url = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let Some(redirect_url) = redirect_url else {
                return Err(Error::DownloadFailed {
                    message: format!("下载文件失败（{url}）：重定向失败：未返回Location"),
                    source: None,
                });
            };

            let resolved = match reqwest::Url::parse(&redirect_url) {
                Ok(abs) => abs.to_string(),
                Err(_) => match reqwest::Url::parse(url).and_then(|base| base.join(&redirect_url)) {
                    Ok(joined) => joined.to_string(),
                    Err(_) => {
                        return Err(Error::DownloadFailed {
                            message: format!("下载文件失败（{url}）：重定向地址无效"),
                            source: None,
                        });
                    }
                },
            };

            return Box::pin(Self::download_file_async(client, &resolved, destination_path, max_redirects - 1)).await;
        }

        if !response.status().is_success() {
            return Err(Error::DownloadFailed {
                message: format!("下载文件失败（{url}）：响应状态码非成功：{}", response.status()),
                source: None,
            });
        }

        // Windows verbatim（\\?\ 前缀）路径下 '/' 不是分隔符，安装器经 path_combine
        // 拼接的 Maven 库路径（如 `libraries\net/...jar`）会令写文件报
        // ERROR_INVALID_NAME (os error 123)；统一换成 '\'（规避同 download_batch）。
        let destination_path = crate::util::file_helper::normalize_separators(destination_path);
        let path = Path::new(&destination_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.is_dir() {
                std::fs::create_dir_all(parent).map_err(|e| Error::DownloadFailed {
                    message: format!("下载文件失败（{url}）：创建目录失败：{e}"),
                    source: Some(Box::new(e)),
                })?;
            }
        }

        let mut file = tokio::fs::File::create(destination_path)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("下载文件失败（{url}）：创建文件失败：{e}"),
                source: Some(Box::new(e)),
            })?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::DownloadFailed {
                message: format!("下载文件失败（{url}）：{e}"),
                source: Some(Box::new(e)),
            })?;
            file.write_all(&chunk)
                .await
                .map_err(|e| Error::DownloadFailed {
                    message: format!("下载文件失败（{url}）：写入文件失败：{e}"),
                    source: Some(Box::new(e)),
                })?;
        }
        Ok(true)
    }

    /// Maven 坐标转文件路径（源：`MavenToPath`，InstallerBase.cs）。
    ///
    /// ⚠️ 源 `Services/Installers/InstallerBase.cs` 与 `Utils/LibHelper.cs` 各有一份
    /// 相同实现（utils 版对 group/artifact 多一步 `RemoveOptionalSuffix` 的 @ 后缀
    /// 剥离；Maven 坐标中 @ 仅出现在 version/classifier 段，实际无差异），
    /// util 版已于早前批次移植 → 此处直接委托（见日志 D3）。
    pub(crate) fn maven_to_path(maven: &str) -> String {
        lib_helper::maven_to_path(maven)
    }

    /// 从 ZIP 中读取指定文件内容（源：`ReadSpecifyFileFromZip(path, fileName)`）。
    ///
    /// 条目名忽略大小写匹配（源 `OrdinalIgnoreCase` → `eq_ignore_ascii_case`）；
    /// 未找到 → Err「未找到指定文件 {fileName}」（源 FileNotFoundException）。
    /// ⚠️ UNMAPPED：源 FileNotFoundException 无对应 Error 变体，暂用 `Error::Params`
    /// （见日志 U1）。zip crate 读 API（B6 已裁剪 deflate-miniz/deflate64，无需新特性）。
    pub(crate) fn read_specify_file_from_zip(path: &str, file_name: &str) -> Result<Vec<u8>, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Params {
            message: format!("读取ZIP失败（{path}）：{e}"),
            source: Some(Box::new(e)),
        })?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| Error::Params {
            message: format!("读取ZIP失败（{path}）：{e}"),
            source: Some(Box::new(e)),
        })?;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| Error::Params {
                message: format!("读取ZIP失败（{path}）：{e}"),
                source: Some(Box::new(e)),
            })?;
            if entry.name().eq_ignore_ascii_case(file_name) {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut buf).map_err(|e| Error::Params {
                    message: format!("读取ZIP失败（{path}）：{e}"),
                    source: Some(Box::new(e)),
                })?;
                return Ok(buf);
            }
        }
        Err(Error::Params {
            message: format!("未找到指定文件 {file_name}"),
            source: None,
        })
    }

    /// 读取 JAR 的 Main-Class（源：`GetJarMainClass(jarPath)`）。
    ///
    /// 从 `META-INF/MANIFEST.MF` 中查找以 `Main-Class: ` 开头的行（忽略大小写，
    /// 源 `StartsWith(OrdinalIgnoreCase)`），返回该值并去除首尾空白；无匹配返回
    /// 空字符串。行按 `\r`/`\n` 分割并丢弃空行（对应源 `Split(['\r','\n'],
    /// RemoveEmptyEntries)`）；MANIFEST 缺失 → 错误上抛（源 FileNotFoundException
    /// 传播）。UTF-8 解码为有损（对应 .NET `Encoding.UTF8.GetString` 替换无效序列）。
    pub(crate) fn get_jar_main_class(jar_path: &str) -> Result<String, Error> {
        let manifest_bytes = Self::read_specify_file_from_zip(jar_path, "META-INF/MANIFEST.MF")?;
        let manifest = String::from_utf8_lossy(&manifest_bytes);

        const MAIN_CLASS_PREFIX: &str = "Main-Class: ";
        for line in manifest.split(['\r', '\n']).filter(|l| !l.is_empty()) {
            if line
                .get(..MAIN_CLASS_PREFIX.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(MAIN_CLASS_PREFIX))
            {
                return Ok(line[MAIN_CLASS_PREFIX.len()..].trim().to_string());
            }
        }
        Ok(String::new())
    }

    /// 运行安装进程并等待退出（源：`RunInstallProcess(arguments, program)` 实例方法，
    /// 未使用实例状态 → 移植为静态方法）。
    ///
    /// 平台分支（B8 定案：android 归 linux）：
    /// - Windows：`program` 为空 → `cmd.exe`；program 为 cmd.exe 时参数加
    ///   `/c ` 前缀，否则原样传递；
    /// - 非 Windows：`program` 为空 → `/bin/bash`（存在时）否则 `/bin/sh`；
    ///   program 为 `/bin/bash` 时参数以 `-c "{arguments}"` 前缀，否则原样传递；
    /// - stdout/stderr 重定向（对应 `RedirectStandardOutput/Error = true`），
    ///   输出不消费即丢弃（源 `BeginOutputReadLine` 事件未处理）；
    /// - 返回子进程退出码；启动失败（源 `Process.Start()` 抛异常）→ 记日志返回 -1。
    ///   ⚠️ 差异：源 stdin 继承父进程，Rust `output()` 关闭 stdin；源
    ///   `CreateNoWindow`（Windows 无控制台窗口）无 std 等价物（见日志 D8）。
    ///
    /// ⚠️ 关键修复（对照 C# `ProcessStartInfo.Arguments`）：C# 是把 `arguments` 当
    /// **命令行字符串**交给 Win32 `CreateProcess` 解析（按 Windows 命令行规则切词），
    /// 因此 java.exe 拿到的是拆分后的独立参数。而 Rust `Command::arg(arguments)` 会把
    /// 整个字符串当作**单个字面参数**（不做命令行切词）传给 java → java 把
    /// `-cp "…classpath…" main …` 判为 `Unrecognized option` → `Could not create the
    /// Java Virtual Machine` → 退出码 1（binarypatcher/installertools 等 java 处理器全失败）。
    /// 修复：Windows 下对非 cmd 程序（java 等）复用 `launch::process::split_command_line`
    /// （CommandLineToArgvW 规则，launch 路径已用）切词，逐参数 `.arg()`。
    pub(crate) fn run_install_process(arguments: &str, program: Option<&str>) -> i32 {
        let program = match program {
            Some(p) => p.to_string(),
            None if cfg!(target_os = "windows") => "cmd.exe".to_string(),
            None if Path::new("/bin/bash").exists() => "/bin/bash".to_string(),
            None => "/bin/sh".to_string(),
        };

        let mut _command = std::process::Command::new(&program);
        #[cfg(windows)]
        {
            if program == "cmd.exe" {
                // cmd 仍需原始命令串，交由 cmd 自身的解析器处理
                _command.arg("/c").arg(arguments);
            } else {
                // java 等非 cmd 程序：按 Windows 命令行规则切词、逐参数传递（对齐 C#）
                for token in crate::services::launch::process::split_command_line(arguments) {
                    _command.arg(token);
                }
            }
        }
        #[cfg(not(windows))]
        {
            if program == "/bin/bash" {
                _command.arg("-c").arg(format!("\"{arguments}\""));
            } else {
                _command.arg(arguments);
            }
        }
        _command.stdout(std::process::Stdio::piped());
        _command.stderr(std::process::Stdio::piped());

        match _command.output() {
            Ok(output) => output.status.code().unwrap_or(-1),
            Err(e) => {
                eprintln!("启动安装进程失败（{program}）：{e}");
                -1
            }
        }
    }
}

/// JSON 深合并（源私有 `Merge(JsonObject, JsonObject)` 方法）。
///
/// - 双方均为对象 → 递归合并（目标已有同键对象时，源缺失键保留）；
/// - 双方均为数组 → 源数组项追加到目标数组末尾（拼接，非覆盖）；
/// - 其余情况（含对象 vs 数组、对象 vs 标量、标量 vs 数组等）→ 源值覆盖。
fn merge_value(target: Value, source: Value) -> Value {
    match (target, source) {
        (Value::Object(mut target_obj), Value::Object(source_obj)) => {
            for (key, value) in source_obj {
                match target_obj.remove(&key) {
                    Some(existing) => {
                        let merged = if value.is_object() && existing.is_object() {
                            merge_value(existing, value)
                        } else if value.is_array() && existing.is_array() {
                            let mut target_arr = existing.as_array().expect("已判定为数组").clone();
                            if let Value::Array(items) = value {
                                target_arr.extend(items);
                            }
                            Value::Array(target_arr)
                        } else {
                            value
                        };
                        target_obj.insert(key, merged);
                    }
                    None => {
                        target_obj.insert(key, value);
                    }
                }
            }
            Value::Object(target_obj)
        }
        (_, source) => source,
    }
}

/// 是否为源手动处理的重定向状态码（301/302/303/307）。
///
/// 源列表：MovedPermanently(301)、Found(302)、Redirect(302，.NET 中与 Found 同值
/// 的过时别名)、SeeOther(303)、TemporaryRedirect(307)；不含 PermanentRedirect(308)
/// —— 308 在源中落入 `EnsureSuccessStatusCode` 报错分支，此处保持一致
/// （不采用 `StatusCode::is_redirection()`，其覆盖全部 3xx）。
fn is_redirect_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::MOVED_PERMANENTLY
            | reqwest::StatusCode::FOUND
            | reqwest::StatusCode::SEE_OTHER
            | reqwest::StatusCode::TEMPORARY_REDIRECT
    )
}



