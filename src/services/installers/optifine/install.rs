//! OptiFine 安装器（B9）
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/OptiFineInstaller.cs（204 行）
//!
//! 流程要点（逐字保留，详见翻译日志 p42-optifine-cleanroom.md）：
//! - 构造：`sourceId == 0` → 官方源 `https://optifine.net/download`，其余值一律 →
//!   BMCLAPI 镜像 `https://bmclapi2.bangbang93.com/optifine`（注意判定条件为 `== 0`，
//!   与 Fabric 的 `== 1` 不同，按源逐字保留）；
//! - InstallAsync：modLoaderVersion（"Type-Patch"）/ installerFilePath / javaPath 空 →
//!   ArgumentNullException；modLoaderVersion 按 '-' 分割非 2 段 → ArgumentException；
//!   构造 `OptiFineVersionInfo`（models/installer.rs）→ InstallCoreAsync：下载安装包
//!   （本地已存在跳过）→ 创建版本目录 → 生成版本 JSON → java 执行 optifine.Patcher →
//!   finally 删除临时安装包；
//! - GetAvailableVersionsAsync：`{源}/{gameVersion}` 查询版本列表（独立 HttpClient，
//!   5 秒超时，源无 User-Agent/重定向定制）→ 按 Patch Ordinal 降序排序；
//! - 版本 JSON：id/inheritsFrom/type/time/releaseTime/mainClass=
//!   `net.minecraft.launchwrapper.Launch`/minecraftArguments=`--tweakClass
//!   optifine.OptiFineTweaker`；libraries = 基础版本 libraries 拷贝 +
//!   `optifine:OptiFine:{mc}_{Type}_{Patch}` + `net.minecraft:launchwrapper:1.12`；
//!   inheritsFromJson 非空时 MergeVersionJson 合并后重新解析；缩进写出（源
//!   WriteIndented=true）；基础 jar 复制到版本目录（File.Copy overwrite）；
//! - RunInstallerAsync：`java -cp "{安装包}" optifine.Patcher "{基础jar}" "{安装包}"
//!   "{库jar}"`，WorkingDirectory = gameDir，stdout/stderr 逐行日志（源
//!   Trace.WriteLine → eprintln! 约定），退出码 0 才成功；
//! - GetMissLibrariesAsync：恒返回空列表（源逐字）。
//!
//! 设计决策（见日志 p42）：
//! - 源 RunInstallerAsync 的 Process 行为（WorkingDirectory / 输出行日志）超出
//!   `InstallerBase::run_install_process` 能力（该工具无 WorkingDirectory、输出直接丢弃，
//!   见 installer.rs D8）→ 本文件内联 `tokio::process::Command` 实现：参数数组直传
//!   （C# 整串经 CommandLineToArgvW 解析后 argv 与之等价，引号仅为壳层语法）；
//!   Windows `CreateNoWindow` → `creation_flags(CREATE_NO_WINDOW)`（launch/process.rs 先例）；
//! - 源 `DateTime.UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ssZ")` → 私有 helper（无 chrono，
//!   B2 定案；civil_from_days 算法同 launch/process.rs，格式逐字：无小数秒、'Z' 后缀）；
//! - 错误映射（沿用安装器域定案）：ArgumentNullException/ArgumentException/
//!   FileNotFoundException（本地文件缺失）→ Error::Params；HTTP/JSON 解析 →
//!   Error::Http；文件 IO / 下载 → Error::DownloadFailed；
//! - ⚠️ UNMAPPED U1：源 RunInstallerAsync 的 `outputJarPath` 计算后从未使用（dead code）
//!   → 省略（同 forge_base.rs U1 先例）。

use std::path::Path;
use tokio::io::AsyncBufReadExt;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::error::Error;
use crate::models::installer::OptiFineVersionInfo;
use crate::services::installers::installer::{Installer, InstallerBase, MissFileData};

/// OptiFine 安装器（源：`internal class OptiFineInstaller : InstallerBase, IInstaller`）。
///
/// 源继承 InstallerBase 仅为复用其静态工具（无实例基类状态）→ 不组合
/// `ForgeInstallerBase`，直接持有三字段（源 readonly 字段）。
pub(crate) struct OptiFineInstaller {
    /// 下载源根地址（源 `_downloadSource`）：sourceId == 0 → 官方，
    /// 其余 → BMCLAPI 镜像（版本列表查询与安装包下载共用）
    download_source: String,
    /// 游戏根目录（源 `_gameDir`，readonly）
    game_dir: String,
    /// 原版游戏版本号（源 `_gameVersion`，readonly）
    game_version: String,
}

impl OptiFineInstaller {
    /// 创建 OptiFine 安装器（源：`OptiFineInstaller(int sourceId, string gameDir,
    /// string gameVersion)`）。
    pub(crate) fn new(source_id: i32, game_dir: String, game_version: String) -> Self {
        // 源：`_downloadSource = sourceId == 0 ? "https://optifine.net/download"
        //      : "https://bmclapi2.bangbang93.com/optifine";`（逐字，`== 0` 判定）
        let download_source = if source_id == 0 {
            "https://optifine.net/download".to_string()
        } else {
            "https://bmclapi2.bangbang93.com/optifine".to_string()
        };
        Self {
            download_source,
            game_dir,
            game_version,
        }
    }

    /// 安装主体（源：`InstallCoreAsync`，private async）。
    ///
    /// 流程：下载安装包（`installerFile.Exists` 前置校验，失败 →
    /// FileNotFoundException「OptiFine安装包下载失败」）→ 创建版本目录 → 版本 JSON
    /// 生成（false → 「版本配置文件创建失败」）→ java 执行 Patcher（false →
    /// 「OptiFine安装程序执行失败」）→ finally 清理临时安装包。
    async fn install_core(
        &self,
        version_id: &str,
        version: &OptiFineVersionInfo,
        java_path: &str,
        inherits_from_json: &str,
    ) -> Result<(), Error> {
        // 源：`var installerFile = await DownloadInstallerAsync(version);`
        let installer_file = self.download_installer(version).await?;
        // 源：`if (!installerFile.Exists) throw new FileNotFoundException("OptiFine安装包下载失败", ...);`
        if !Path::new(&installer_file).is_file() {
            return Err(Error::Params {
                message: format!("OptiFine安装包下载失败: {installer_file}"),
                source: None,
            });
        }

        // 源：`try { ... } finally { CleanupTempFiles(installerFile.FullName); }`
        let result = async {
            // 源：`var versionDir = Path.Combine(_gameDir, "versions", optiVersionId);
            //      if (!Directory.Exists(versionDir)) Directory.CreateDirectory(versionDir);`
            let version_dir = path_combine(&path_combine(&self.game_dir, "versions"), version_id);
            if !Path::new(&version_dir).is_dir() {
                std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
                    message: format!("创建版本目录失败: {version_dir}"),
                    source: Some(Box::new(e)),
                })?;
            }

            // 源：`bool jsonCreated = await CreateVersionJsonAsync(...);
            //      if (!jsonCreated) throw new Exception("版本配置文件创建失败");`
            let json_created = self
                .create_version_json(version, version_id, &version_dir, inherits_from_json)
                .await?;
            if !json_created {
                return Err(Error::Params {
                    message: "版本配置文件创建失败".to_string(),
                    source: None,
                });
            }

            // 源：`bool installSuccess = await RunInstallerAsync(...);
            //      if (!installSuccess) throw new Exception("OptiFine安装程序执行失败");`
            let install_success = self
                .run_installer(&installer_file, java_path, &version_dir, version_id)
                .await?;
            if !install_success {
                return Err(Error::Params {
                    message: "OptiFine安装程序执行失败".to_string(),
                    source: None,
                });
            }
            Ok(())
        }
        .await;
        Self::cleanup_temp_files(&installer_file);
        result
    }

    /// 查询 OptiFine 可用版本列表（源：`GetAvailableVersionsAsync`，public async）。
    ///
    /// - 独立 HttpClient，超时 5 秒（源 `new HttpClient { Timeout = TimeSpan.FromSeconds(5) }`，
    ///   不设 User-Agent/重定向策略 → 不复用 create_http_client）；
    /// - URL = `{_downloadSource}/{_gameVersion}`（源逐字）；响应为空串 → 空列表；
    /// - 反序列化为 `Vec<OptiFineVersionInfo>`（源 AOT context ListOptiFineVersionInfo）；
    /// - 按 Patch 降序排序：源 `string.Compare(b.Patch, a.Patch, Ordinal)` → Rust
    ///   `Option<String>` 字节序比较（None < Some，与 C# null < 任意字符串一致）。

    /// 下载（或复用）OptiFine 安装包（源：`DownloadInstallerAsync`，private async）。
    ///
    /// 优先级：`version.FileName` 非空且本地存在 → 直接使用；否则下载
    /// `{_downloadSource}/{_gameVersion}/{Type}/{Patch}` 到
    /// `{gameDir}/temp/{gameVersion}_{Type}_{Patch}.jar`（已存在跳过）。
    async fn download_installer(&self, version: &OptiFineVersionInfo) -> Result<String, Error> {
        // 源：`if (!string.IsNullOrEmpty(version.FileName) && File.Exists(version.FileName))
        //      return new FileInfo(version.FileName);`
        if let Some(file_name) = &version.file_name {
            if !file_name.is_empty() && Path::new(file_name).is_file() {
                return Ok(file_name.clone());
            }
        }

        // 源：`var url = $"{_downloadSource}/{_gameVersion}/{version.Type}/{version.Patch}";`
        let url = format!(
            "{}/{}/{}/{}",
            self.download_source,
            self.game_version,
            version.r#type.as_deref().unwrap_or_default(),
            version.patch.as_deref().unwrap_or_default(),
        );
        // 源：`var fileName = $"{_gameVersion}_{version.Type}_{version.Patch}.jar";`
        let file_name = format!(
            "{}_{}_{}.jar",
            self.game_version,
            version.r#type.as_deref().unwrap_or_default(),
            version.patch.as_deref().unwrap_or_default(),
        );
        // 源：`var savePath = Path.Combine(_gameDir, "temp", fileName);`
        let save_path = path_combine(&path_combine(&self.game_dir, "temp"), &file_name);

        // 源：`var tempDir = Path.GetDirectoryName(savePath)!;`（null → 后续
        //      Directory.CreateDirectory(null) 抛 ArgumentNullException）
        let temp_dir = Path::new(&save_path)
            .parent()
            .ok_or_else(|| Error::Params {
                message: "安装包保存路径无父目录（源 ArgumentNullException）".to_string(),
                source: None,
            })?
            .to_string_lossy()
            .to_string();
        if !Path::new(&temp_dir).is_dir() {
            std::fs::create_dir_all(&temp_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建临时目录失败: {temp_dir}"),
                source: Some(Box::new(e)),
            })?;
        }

        // 源：`if (File.Exists(savePath)) return new FileInfo(savePath);`
        if Path::new(&save_path).is_file() {
            return Ok(save_path);
        }

        // 源：`await DownloadFileAsync(CreateHttpClient(), url, savePath);`
        InstallerBase::download_file_async(&InstallerBase::create_http_client(), &url, &save_path, 5)
            .await?;
        Ok(save_path)
    }

    /// 生成 OptiFine 版本 JSON 并复制基础 jar（源：`CreateVersionJsonAsync`，private async）。
    ///
    /// 逐字要点：基础版本 JSON 必须存在（否则「基础Minecraft客户端JAR文件不存在」）；
    /// 新 JSON 固定字段（id/inheritsFrom/type=release/time/releaseTime/mainClass/
    /// minecraftArguments）+ libraries（基础拷贝 + optifine 库 + launchwrapper 1.12）；
    /// inheritsFromJson 非空 → `MergeVersionJson(inheritsFromJson, 新JSON, versionId)`
    /// 合并并重新解析；`WriteIndented = true` → `to_string_pretty`；
    /// 基础 jar 复制到版本目录（覆盖）。
    async fn create_version_json(
        &self,
        opti_version: &OptiFineVersionInfo,
        version_id: &str,
        version_dir: &str,
        inherits_from_json: &str,
    ) -> Result<bool, Error> {
        // 源：`var baseJsonPath = Path.Combine(_gameDir, "versions", _gameVersion, $"{_gameVersion}.json");
        //      if (!File.Exists(baseJsonPath)) throw new FileNotFoundException(...);`
        let base_json_path = path_combine(
            &path_combine(&path_combine(&self.game_dir, "versions"), &self.game_version),
            &format!("{}.json", self.game_version),
        );
        if !Path::new(&base_json_path).is_file() {
            return Err(Error::Params {
                message: format!("基础Minecraft客户端JAR文件不存在: {base_json_path}"),
                source: None,
            });
        }

        // 源：`var baseJsonContent = await File.ReadAllTextAsync(baseJsonPath);`
        let base_json_content = std::fs::read_to_string(&base_json_path).map_err(|e| Error::Params {
            message: format!("读取基础版本 JSON 失败: {e}"),
            source: Some(Box::new(e)),
        })?;
        // 源：`var baseJson = JsonNode.Parse(baseJsonContent)!.AsObject();`
        let base_json: Value = serde_json::from_str(&base_json_content).map_err(|e| Error::Params {
            message: format!("基础版本 JSON 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;
        let base_libraries = base_json.get("libraries").and_then(|v| v.as_array());

        // 源：`["time"] = DateTime.UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ssZ")`（time/releaseTime 同值）
        let utc_now = utc_now_z_string();

        // 源：`["libraries"] = new JsonArray()` → 先拷基础 libraries（JsonNode 引用共享，
        //      Rust 侧 Value clone 深拷贝，内容等价），再追加 optifine 库与 launchwrapper 库
        let mut libraries = Vec::new();
        if let Some(base_libraries) = base_libraries {
            for lib in base_libraries {
                libraries.push(lib.clone());
            }
        }
        libraries.push(Value::Object(Map::from_iter([(
            "name".to_string(),
            Value::String(format!(
                "optifine:OptiFine:{}_{}_{}",
                self.game_version,
                opti_version.r#type.as_deref().unwrap_or_default(),
                opti_version.patch.as_deref().unwrap_or_default(),
            )),
        )])));
        libraries.push(Value::Object(Map::from_iter([(
            "name".to_string(),
            Value::String("net.minecraft:launchwrapper:1.12".to_string()),
        )])));

        let mut new_json = Map::new();
        new_json.insert("id".to_string(), Value::String(version_id.to_string()));
        new_json.insert("inheritsFrom".to_string(), Value::String(self.game_version.clone()));
        new_json.insert("type".to_string(), Value::String("release".to_string()));
        new_json.insert("time".to_string(), Value::String(utc_now.clone()));
        new_json.insert("releaseTime".to_string(), Value::String(utc_now));
        new_json.insert(
            "mainClass".to_string(),
            Value::String("net.minecraft.launchwrapper.Launch".to_string()),
        );
        new_json.insert(
            "minecraftArguments".to_string(),
            Value::String("--tweakClass optifine.OptiFineTweaker".to_string()),
        );
        new_json.insert("libraries".to_string(), Value::Array(libraries));

        // 源：`if (!string.IsNullOrEmpty(inheritsFromJson))
        //      newJson = JsonNode.Parse(MergeVersionJson(inheritsFromJson, newJson.ToString(), versionId))!.AsObject();`
        let new_json: Value = if !inherits_from_json.is_empty() {
            let merged = InstallerBase::merge_version_json(
                inherits_from_json,
                &serde_json::to_string(&Value::Object(new_json)).map_err(|e| Error::Params {
                    message: format!("序列化版本 JSON 失败: {e}"),
                    source: None,
                })?,
                Some(version_id),
            );
            // 源 `JsonNode.Parse(...)!`：合并结果非空（merge_version_json 失败返回空串时
            // 源 Parse("") 抛 JsonException，语义一致）
            serde_json::from_str(&merged).map_err(|e| Error::Params {
                message: format!("合并后版本 JSON 解析失败（源 JsonException）: {e}"),
                source: None,
            })?
        } else {
            Value::Object(new_json)
        };

        // 源：`await File.WriteAllTextAsync(jsonPath, newJson.ToJsonString(new JsonSerializerOptions { WriteIndented = true }));`
        let json_path = path_combine(version_dir, &format!("{version_id}.json"));
        let content = serde_json::to_string_pretty(&new_json).map_err(|e| Error::Params {
            message: format!("序列化版本 JSON 失败: {e}"),
            source: None,
        })?;
        std::fs::write(&json_path, &content).map_err(|e| Error::DownloadFailed {
            message: format!("写出版本 JSON 失败: {json_path}"),
            source: Some(Box::new(e)),
        })?;

        // 源：`var sourceJar = ...; var targetJar = ...; if (!File.Exists(sourceJar)) throw ...;
        //      File.Copy(sourceJar, targetJar, true);`
        let source_jar = path_combine(
            &path_combine(&path_combine(&self.game_dir, "versions"), &self.game_version),
            &format!("{}.jar", self.game_version),
        );
        if !Path::new(&source_jar).is_file() {
            return Err(Error::Params {
                message: format!("基础Minecraft客户端JAR文件不存在: {source_jar}"),
                source: None,
            });
        }
        let target_jar = path_combine(version_dir, &format!("{version_id}.jar"));
        std::fs::copy(&source_jar, &target_jar).map_err(|e| Error::DownloadFailed {
            message: format!("复制基础 JAR 失败: {e}"),
            source: Some(Box::new(e)),
        })?;

        Ok(true)
    }

    /// 执行 OptiFine Patcher（源：`RunInstallerAsync`，private async）。
    ///
    /// java 启动 `-cp "{安装包}" optifine.Patcher "{基础jar}" "{安装包}" "{库jar}"`，
    /// WorkingDirectory = gameDir；stdout/stderr 重定向逐行日志（源 Trace.WriteLine →
    /// eprintln! 约定）；返回退出码 == 0。
    ///
    /// ⚠️ 差异说明（见日志 p42）：源 `ProcessStartInfo.Arguments` 整串由 Windows
    /// CommandLineToArgvW / Unix CRT 解析为 argv，双引号仅作分组语法 → Rust 直接传参数
    /// 数组，argv 与源等价。`CreateNoWindow` → Windows `creation_flags(CREATE_NO_WINDOW)`
    /// （launch/process.rs 先例）。
    async fn run_installer(
        &self,
        installer_path: &str,
        java_path: &str,
        _version_dir: &str,
        version_id: &str,
    ) -> Result<bool, Error> {
        // 源：`var clientJarPath = Path.Combine(_gameDir, "versions", _gameVersion, $"{_gameVersion}.jar");`
        let client_jar_path = path_combine(
            &path_combine(&path_combine(&self.game_dir, "versions"), &self.game_version),
            &format!("{}.jar", self.game_version),
        );
        // ⚠️ U1：源 `var outputJarPath = ...` 计算后从未使用（dead code）→ 省略

        // 源：`var parts = versionId.Split('_');`（长度不足时源抛 IndexOutOfRangeException，
        //      Rust 侧显式校验 → Error::Params，见日志 p42 U2）
        let parts: Vec<&str> = version_id.split('_').collect();
        if parts.len() < 3 {
            return Err(Error::Params {
                message: "版本ID格式错误，缺少 '_' 分段（源 IndexOutOfRangeException）".to_string(),
                source: None,
            });
        }
        // 源：`var libPath = Path.Combine(_gameDir, "libraries", "optifine", "OptiFine",
        //      $"{_gameVersion}_{parts[1]}_{parts[2]}",
        //      $"OptiFine-{_gameVersion}_{parts[1]}_{parts[2]}.jar");`
        let lib_version_part = format!("{}_{}", parts[1], parts[2]);
        let lib_path = path_combine(
            &path_combine(
                &path_combine(
                    &path_combine(&path_combine(&self.game_dir, "libraries"), "optifine"),
                    "OptiFine",
                ),
                &format!("{}_{}", self.game_version, lib_version_part),
            ),
            &format!("OptiFine-{}_{}.jar", self.game_version, lib_version_part),
        );

        // 源：`var libDir = Path.GetDirectoryName(libPath)!; if (!Directory.Exists(libDir)) Directory.CreateDirectory(libDir);`
        let lib_dir = Path::new(&lib_path)
            .parent()
            .ok_or_else(|| Error::Params {
                message: "库路径无父目录（源 ArgumentNullException）".to_string(),
                source: None,
            })?
            .to_string_lossy()
            .to_string();
        if !Path::new(&lib_dir).is_dir() {
            std::fs::create_dir_all(&lib_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建 OptiFine 库目录失败: {lib_dir}"),
                source: Some(Box::new(e)),
            })?;
        }

        // 源：`var arguments = $"-cp \"{installerPath}\" optifine.Patcher \"{clientJarPath}\"
        //      \"{installerPath}\" \"{libPath}\"";` → argv 数组（引号仅为命令行分组语法）
        let mut cmd = tokio::process::Command::new(java_path);
        cmd.args([
            "-cp",
            installer_path,
            "optifine.Patcher",
            &client_jar_path,
            installer_path,
            &lib_path,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(&self.game_dir);
        // 源 `CreateNoWindow = true`（Windows 不创建控制台窗口；launch/process.rs 先例）
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        // 源 `process.Start()` 失败抛异常（Win32Exception）→ 传播
        let mut child = cmd.spawn().map_err(|e| Error::Params {
            message: format!("启动 OptiFine 安装进程失败（源 Process.Start 异常）: {e}"),
            source: Some(Box::new(e)),
        })?;

        // 源：`OutputDataReceived/ErrorDataReceived` 事件 → Trace.WriteLine（→ eprintln! 约定）；
        //      空行丢弃（`if (!string.IsNullOrEmpty(e.Data))`）。必须读管道，否则子进程输出
        //      缓冲满会阻塞（同 launch/process.rs forward_pipe 语义）
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        if let Some(pipe) = stdout_pipe {
            let _ = tokio::spawn(forward_pipe_lines(pipe, true));
        }
        if let Some(pipe) = stderr_pipe {
            let _ = tokio::spawn(forward_pipe_lines(pipe, false));
        }

        // 源：`await process.WaitForExitAsync(); return process.ExitCode == 0;`
        let status = child.wait().await.map_err(|e| Error::Params {
            message: format!("等待 OptiFine 安装进程失败: {e}"),
            source: Some(Box::new(e)),
        })?;
        Ok(status.code() == Some(0))
    }

    /// 清理临时安装包（源：`CleanupTempFiles`，private static）。
    ///
    /// 删除文件失败静默（源 try/catch 空块；`File.Exists` 前置检查由
    /// `std::fs::remove_file` 对不存在的处理等价覆盖）。
    fn cleanup_temp_files(installer_path: &str) {
        let _ = std::fs::remove_file(installer_path);
    }
}

/// 源 InstallAsync 的 trait 实现：para1 = modLoaderVersion（"Type-Patch"），
/// para2 = installerFilePath，para3 = javaPath，para4 未使用。
#[async_trait]
impl Installer for OptiFineInstaller {
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：`if (string.IsNullOrEmpty(modLoaderVersion)) throw new ArgumentNullException(...);`
        let mod_loader_version = para1.ok_or_else(|| Error::Params {
            message: "modLoaderVersion 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        // 源：`if (string.IsNullOrEmpty(installerFilePath)) throw new ArgumentNullException(...);`
        let installer_file_path = para2.ok_or_else(|| Error::Params {
            message: "installerFilePath 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        // 源：`if (string.IsNullOrEmpty(javaPath)) throw new ArgumentNullException(...);`
        let java_path = para3.ok_or_else(|| Error::Params {
            message: "javaPath 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;

        // 源：`var ofInfoParts = modLoaderVersion.Split('-');
        //      if (ofInfoParts.Length != 2) throw new ArgumentException("modLoaderVersion格式错误，需为\"Type-Patch\"", ...);`
        let of_info_parts: Vec<&str> = mod_loader_version.split('-').collect();
        if of_info_parts.len() != 2 {
            return Err(Error::Params {
                message: "modLoaderVersion格式错误，需为\"Type-Patch\"".to_string(),
                source: None,
            });
        }

        // 源：`var version = new OptiFineVersionInfo { McVersion = _gameVersion, Type = ...,
        //      Patch = ..., FileName = installerFilePath };`
        let version = OptiFineVersionInfo {
            mc_version: Some(self.game_version.clone()),
            r#type: Some(of_info_parts[0].to_string()),
            patch: Some(of_info_parts[1].to_string()),
            file_name: Some(installer_file_path.to_string()),
        };

        // 源：`await InstallCoreAsync(versionId, version, javaPath, inheritsFromJson);`
        self.install_core(version_id, &version, java_path, inherits_from_json)
            .await
    }

    async fn get_miss_libraries(
        &self,
        _para1: Option<&str>,
        _para2: Option<&str>,
        _para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：`Task.FromResult(new List<MissFileData>())`（恒空列表）
        Ok(Vec::new())
    }
}

/// 逐行读取子进程管道并打印日志（源 `OutputDataReceived`/`ErrorDataReceived` 事件 +
/// Trace.WriteLine → eprintln! 约定；空行丢弃，读错误/EOF 结束，不向外传播）。
async fn forward_pipe_lines<R>(mut pipe: R, is_stdout: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = tokio::io::BufReader::new(&mut pipe);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if !trimmed.is_empty() {
                    if is_stdout {
                        eprintln!("OptiFine安装输出: {trimmed}");
                    } else {
                        eprintln!("OptiFine安装错误: {trimmed}");
                    }
                }
            }
        }
    }
}

/// 源 `DateTime.UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ssZ")` 的 UTC 时间串。
///
/// 格式逐字：无小数秒、字面 'Z' 后缀（区别于 launch/process.rs 的毫秒版与
/// microsoft.rs 的 +00:00 版）；无 chrono 依赖（B2 定案），民用日期换算采用
/// Howard Hinnant civil_from_days 算法（与 launch/process.rs 同源）。
fn utc_now_z_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
    )
}

/// 天数（自 1970-01-01）→ 民用日期 (年, 月, 日)（Howard Hinnant civil_from_days）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month as u32, day as u32)
}

/// 模拟 C# `Path.Combine(a, b)`（与 forge/fabric 安装器内部同名辅助一致）：
/// a 为空 → b；a 以分隔符（/ 或 \）结尾 → 直接拼接；否则插入平台主分隔符
/// （Windows: \，其余: /）。仅字符串拼接，不规范化路径。
fn path_combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if a.ends_with(['/', '\\']) {
        format!("{a}{b}")
    } else {
        format!("{a}{}{b}", std::path::MAIN_SEPARATOR)
    }
}



