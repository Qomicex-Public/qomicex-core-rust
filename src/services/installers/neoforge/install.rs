//! NeoForge 安装器（B9）
//! 对应源文件：Services/Installers/NeoForgeInstaller.cs（256 行）
//!
//! 流程要点（逐字保留）：
//! - 安装器 ZIP 内读取 version.json / install_profile.json / data/client.lzma；
//! - profile 名校验（小写后须为 "neoforge"；1.20.1 特例放行 "forge"）；
//! - 版本 JSON：id = versionId、inheritsFrom = gameVersion；inheritsFromJson 非空时
//!   MergeVersionJson 合并并补写 clientVersion（保留原版版本信息）；
//! - 写出 `versions/{versionId}/{versionId}.json` 与
//!   `libraries/net/neoforged/neoforge/{versionId}/client.lzma`，回写
//!   install_profile.json 的 data.BINPATCH.client 指向 client.lzma 绝对路径；
//! - 缺失库下载（GetMissNeoForgeLibraries）+ processors 按序执行（side=client）；
//! - 任何写出/下载/处理器失败 → BackInstall 回滚（删除已写文件 + 空目录）。
//!
//! ⚠️ 协同契约（src/services/installers/forge_base.rs，P38 已交付，以下为已校对的实际签名）：
//! - `ForgeInstallerBase`：pub(crate) 字段 base_url / source_id / game_dir / game_version /
//!   installer_path / main_jar_path / source_mappings（对应源 internal 字段 BaseUrl /
//!   SourceId / gameDir / gameVersion / _installerPath / _mainJarPath / SourceMappings；
//!   ⚠️ 未派生 Clone → 本文件在 install_neoforge 内手工逐字段复制以写动态状态）；
//! - `SourcesList { original: String, default: String }`（源 SourcesList 结构体）；
//! - 实例方法：`resolve_url(&self, &str) -> String`（源 ResolveUrl）、
//!   `async run_processor(&self, &Map<String, Value>, &Map<String, Value>, &str, &str,
//!   &str) -> Result<(), Error>`（源 RunProcessor）；
//! - 关联函数（源实例方法未用实例状态 → 静态化，P38 定案）：
//!   `should_run_processor(&Map<String, Value>, &str) -> bool`（源同名）、
//!   `extract_maven_coordinates_from_processors(&Map<String, Value>) -> Vec<String>`（源同名）；
//! - 静态方法：`async is_file_url_available_async(url: &str, timeout_seconds: u64) -> bool`
//!   （源 `static async Task<bool> IsFileUrlAvailableAsync(string url, int timeoutSeconds = 10)`）。
//!
//! ⚠️ 设计说明：源 InstallAsync 对实例字段 `_installerPath`/`_mainJarPath` 赋值（每次
//! 安装的动态状态），而 Installer trait 仅提供 `&self` → 手工复制 base 并在副本上写入
//! 这两个值后再进入安装主体（ForgeInstallerBase 未派生 Clone，见日志 p40）。
//!
//! 错误语义（沿用安装器域既有定案）：ArgumentNullException / 参数校验 → Error::Params；
//! JSON 解析（源 JsonException）→ Error::Http；文件 IO / 下载 → Error::DownloadFailed。

use std::cmp::Ordering;
use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Error;
use crate::services::installers::forge_base::{
    ForgeInstallerBase, SourcesList, main_jar_relative_path,
};
use crate::services::installers::installer::{Installer, InstallerBase, MissFileData};

/// NeoForge 安装器（源：`internal class NeoForgeInstaller : ForgeInstallerBase, IInstaller`）。
///
/// C# 继承 → Rust 组合：持有 `ForgeInstallerBase`（源基类实例状态与解析辅助）。
pub(crate) struct NeoForgeInstaller {
    base: ForgeInstallerBase,
}

impl NeoForgeInstaller {
    /// 创建 NeoForge 安装器（源：`NeoForgeInstaller(int sourceId, string gameDir,
    /// string gameVersion)`）。
    ///
    /// - `sourceId == 1` → BMCLAPI 镜像：BaseUrl = `https://bmclapi2.bangbang93.com/maven`，
    ///   并注册两条源映射（maven.neoforged.net 的 forge / neoforge 仓库路径 → 镜像路径）；
    /// - 其余值一律按官方处理：BaseUrl =
    ///   `https://maven.neoforged.net/releases|https://libraries.minecraft.net`
    ///   （`|` 分隔的多备选源，见 get_miss_neoforge_libraries）。
    pub(crate) fn new(source_id: i32, game_dir: String, game_version: String) -> Self {
        let (base_url, source_mappings) = if source_id == 1 {
            let base_url = "https://bmclapi2.bangbang93.com/maven".to_string();
            let source_mappings = vec![
                SourcesList {
                    original: "https://maven.neoforged.net/releases/net/neoforged/forge"
                        .to_string(),
                    default: format!("{base_url}/net/neoforged/forge"),
                },
                SourcesList {
                    original: "https://maven.neoforged.net/releases/net/neoforged/neoforge"
                        .to_string(),
                    default: format!("{base_url}/net/neoforged/neoforge"),
                },
            ];
            (base_url, source_mappings)
        } else {
            (
                "https://maven.neoforged.net/releases|https://libraries.minecraft.net".to_string(),
                Vec::new(),
            )
        };
        Self {
            base: ForgeInstallerBase {
                base_url,
                source_id,
                source_mappings,
                game_dir,
                game_version,
                installer_path: String::new(),
                main_jar_path: String::new(),
            },
        }
    }

    /// 执行 NeoForge 安装主体（源：`InstallNeoForge(string versionId, string
    /// inheritsFromJson, string javaPath, string neoForgeInstallerPath)`，private）。
    ///
    /// 逐字流程：
    /// 1. 读取安装器 ZIP 内 version.json / install_profile.json / data/client.lzma，
    ///    失败 → 「读取NeoForge安装器内容失败，请检查安装器文件是否正确」；
    /// 2. profile 名校验：小写后须为 "neoforge"；特例 `gameVersion == "1.20.1"` 且
    ///    profile 为 "forge" 亦放行（NeoForge 1.20.1 安装器沿用 forge profile），
    ///    否则「安装器版本不正确，请检查安装器文件是否正确」；
    /// 3. version.json 重写 id = versionId、inheritsFrom = gameVersion；inheritsFromJson
    ///    非空时 MergeVersionJson 合并，并补写 clientVersion = gameVersion
    ///    （源注释：MergeVersionJson 会删除 inheritsFrom，补写 clientVersion 以保留
    ///    原版版本信息）；
    /// 4. 写出 `versions/{versionId}/{versionId}.json`（失败 → 回滚 +
    ///    「写出NeoForge版本Json失败: {原因}」）；
    /// 5. 写出 `libraries/net/neoforged/neoforge/{versionId}/client.lzma`（失败 → 回滚 +
    ///    「写出NeoForge LZMA失败: {原因}」）；
    /// 6. install_profile.json 的 data.BINPATCH.client 回写为 `"{clientLzmaPath}"`；
    /// 7. 下载缺失库（get_miss_neoforge_libraries；失败 → 回滚 +
    ///    「下载NeoForge缺失库失败: {path}\n{原因}」）；
    /// 8. 按序执行 processors（side = "client"，should_run_processor 过滤；失败 → 回滚 +
    ///    「处理NeoForge处理器失败: {jar}\n{原因}」）。
    ///
    /// ⚠️ 源 InstallAsync 先将 `_installerPath`/`_mainJarPath` 实例字段赋值再进入本方法；
    /// trait 仅提供 `&self` → 手工复制 base 并在副本上写入本次安装动态状态。
    /// 主 jar 指向**版本隔离目录** `versions/{versionId}/{versionId}.jar`（源为共享的
    /// `versions/{gameVersion}`；见 `forge_base::main_jar_relative_path` 原因说明）。
    async fn install_neoforge(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        java_path: &str,
        neo_forge_installer_path: &str,
    ) -> Result<(), Error> {
        // ⚠️ ForgeInstallerBase 未派生 Clone（P38 定案）→ 手工逐字段复制
        let mut base = ForgeInstallerBase {
            base_url: self.base.base_url.clone(),
            source_id: self.base.source_id,
            game_dir: self.base.game_dir.clone(),
            game_version: self.base.game_version.clone(),
            installer_path: self.base.installer_path.clone(),
            main_jar_path: self.base.main_jar_path.clone(),
            source_mappings: self
                .base
                .source_mappings
                .iter()
                .map(|m| SourcesList {
                    original: m.original.clone(),
                    default: m.default.clone(),
                })
                .collect(),
        };
        base.installer_path = neo_forge_installer_path.to_string();
        base.main_jar_path = main_jar_relative_path(version_id);

        // 源：try { ... } catch (Exception ex) {
        //      throw new Exception("读取NeoForge安装器内容失败，请检查安装器文件是否正确", ex); }
        let read_zip = || -> Result<(String, String, Vec<u8>), Error> {
            Ok((
                String::from_utf8_lossy(&InstallerBase::read_specify_file_from_zip(
                    neo_forge_installer_path,
                    "version.json",
                )?)
                .into_owned(),
                String::from_utf8_lossy(&InstallerBase::read_specify_file_from_zip(
                    neo_forge_installer_path,
                    "install_profile.json",
                )?)
                .into_owned(),
                InstallerBase::read_specify_file_from_zip(
                    neo_forge_installer_path,
                    "data/client.lzma",
                )?,
            ))
        };
        let (mut json_data, install_profile_data, client_lzma) =
            read_zip().map_err(|e| Error::DownloadFailed {
                message: "读取NeoForge安装器内容失败，请检查安装器文件是否正确".to_string(),
                source: Some(Box::new(e)),
            })?;

        // 源：`var installProfileJson = JsonNode.Parse(installProfileData!)!.AsObject();`
        let mut profile_value: Value =
            serde_json::from_str(&install_profile_data).map_err(|e| Error::Http {
                message: format!("install_profile.json 解析失败: {e}"),
                status: None,
                source: Some(Box::new(e)),
            })?;

        // 源：`string profileName = installProfileJson["profile"]?.ToString().ToLower()
        //      ?? string.Empty;`
        let profile_name = profile_value
            .get("profile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        // 源：`if (profileName != "neoforge" && !(gameVersion == "1.20.1" && profileName == "forge"))
        //      throw new Exception("安装器版本不正确，请检查安装器文件是否正确");`
        if profile_name != "neoforge" && !(base.game_version == "1.20.1" && profile_name == "forge")
        {
            return Err(Error::Params {
                message: "安装器版本不正确，请检查安装器文件是否正确".to_string(),
                source: None,
            });
        }

        // 源：`versionData["id"] = versionId; versionData["inheritsFrom"] = this.gameVersion;
        //      jsonData = versionData.ToJsonString();`
        let mut version_value: Value =
            serde_json::from_str(&json_data).map_err(|e| Error::Http {
                message: format!("version.json 解析失败: {e}"),
                status: None,
                source: Some(Box::new(e)),
            })?;
        let version_obj = version_value.as_object_mut().ok_or_else(|| Error::Http {
            message: "version.json 顶层非 JSON 对象（源 InvalidOperationException）".to_string(),
            status: None,
            source: None,
        })?;
        version_obj.insert("id".to_string(), Value::String(version_id.to_string()));
        version_obj.insert(
            "inheritsFrom".to_string(),
            Value::String(base.game_version.clone()),
        );
        json_data = version_value.to_string();

        if !inherits_from_json.is_empty() {
            // 源：`jsonData = MergeVersionJson(inheritsFromJson, jsonData, versionId);`
            let merged =
                InstallerBase::merge_version_json(inherits_from_json, &json_data, Some(version_id));
            // 源：`var mergedObj = JsonNode.Parse(jsonData)!.AsObject();`
            //（MergeVersionJson 失败返回空串 → 源此处解析抛异常，见 installer.rs D6）
            let mut merged_obj: Value =
                serde_json::from_str(&merged).map_err(|_| Error::DownloadFailed {
                    message: "版本JSON合并失败".to_string(),
                    source: None,
                })?;
            if !merged_obj.is_object() {
                return Err(Error::DownloadFailed {
                    message: "版本JSON合并失败".to_string(),
                    source: None,
                });
            }
            // 源注释：MergeVersionJson 会删除 inheritsFrom，补写 clientVersion 以保留原版版本信息
            merged_obj["clientVersion"] = Value::String(base.game_version.clone());
            json_data = merged_obj.to_string();
        }

        let mut back_files: Vec<String> = Vec::new();
        let mut back_dirs: Vec<String> = Vec::new();

        // 源：`var versionDir = Path.Combine(gameDir, "versions", versionId);
        //      if (!Directory.Exists(versionDir)) { Directory.CreateDirectory(versionDir);
        //      backDirs.Add(versionDir); }`（CreateDirectory 无 try/catch，异常直接上抛不回滚）
        let version_dir = path_combine(&path_combine(&base.game_dir, "versions"), version_id);
        if !Path::new(&version_dir).is_dir() {
            std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建版本目录失败: {version_dir}: {e}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(version_dir.clone());
        }
        let target_json_path = path_combine(&version_dir, &format!("{version_id}.json"));
        // 源：try { File.WriteAllText } catch { BackInstall;
        //      throw "写出NeoForge版本Json失败: {ex.Message}" }
        if let Err(e) = std::fs::write(&target_json_path, &json_data) {
            back_install(&back_files, &back_dirs);
            return Err(Error::DownloadFailed {
                message: format!("写出NeoForge版本Json失败: {e}"),
                source: Some(Box::new(e)),
            });
        }
        back_files.push(target_json_path);

        // 源：`var lzmaDir = Path.Combine(gameDir, "libraries", "net", "neoforged",
        //      "neoforge", versionId);`（目录创建同 versionDir，无 try/catch）
        let lzma_dir = path_combine(
            &path_combine(
                &path_combine(&path_combine(&base.game_dir, "libraries"), "net"),
                "neoforged",
            ),
            "neoforge",
        );
        let lzma_dir = path_combine(&lzma_dir, version_id);
        if !Path::new(&lzma_dir).is_dir() {
            std::fs::create_dir_all(&lzma_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建LZMA目录失败: {lzma_dir}: {e}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(lzma_dir.clone());
        }
        let client_lzma_path = path_combine(&lzma_dir, "client.lzma");
        // 源：try { File.WriteAllBytes } catch { BackInstall;
        //      throw "写出NeoForge LZMA失败: {ex.Message}" }
        if let Err(e) = std::fs::write(&client_lzma_path, &client_lzma) {
            back_install(&back_files, &back_dirs);
            return Err(Error::DownloadFailed {
                message: format!("写出NeoForge LZMA失败: {e}"),
                source: Some(Box::new(e)),
            });
        }
        back_files.push(client_lzma_path.clone());

        // 源：`installProfileJson["data"]!["BINPATCH"]!["client"] = $"\"{clientLzmaPath}\"";`
        //（data / BINPATCH / client 任一缺失 → 源 NullReferenceException）
        let binpatch_client = profile_value
            .get_mut("data")
            .and_then(|d| d.as_object_mut())
            .and_then(|d| d.get_mut("BINPATCH"))
            .and_then(|b| b.as_object_mut())
            .and_then(|b| b.get_mut("client"))
            .ok_or_else(|| Error::Params {
                message:
                    "install_profile.json 缺少 data.BINPATCH.client（源 NullReferenceException）"
                        .to_string(),
                source: None,
            })?;
        *binpatch_client = Value::String(format!("\"{client_lzma_path}\""));

        // 源：`var libs = GetMissNeoForgeLibraries(neoForgeInstallerPath, versionId);`
        let libs = self
            .get_miss_neoforge_libraries(neo_forge_installer_path, version_id)
            .await?;
        let client = InstallerBase::create_http_client();
        for lib in libs {
            // 源：try { await DownloadFileAsync(CreateHttpClient(), lib.Url, lib.Path); }
            //     catch { BackInstall;
            //     throw "下载NeoForge缺失库失败: {lib.Path}\n{ex.Message}" }
            if let Err(e) =
                InstallerBase::download_file_async(&client, &lib.url, &lib.path, 5).await
            {
                back_install(&back_files, &back_dirs);
                return Err(Error::DownloadFailed {
                    message: format!("下载NeoForge缺失库失败: {}\n{}", lib.path, e),
                    source: Some(Box::new(e)),
                });
            }
        }

        // 源：`var processors = installProfileJson["processors"] as JsonArray;
        //      if (processors != null && processors.Count > 0)`
        // 源 `installProfileJson = JsonNode.Parse(...)!.AsObject()` → 顶层恒为对象；
        // 此处取 &Map 供 P38 的 run_processor / should_run_processor（签名见文件头）
        let profile_obj = profile_value.as_object().ok_or_else(|| Error::Params {
            message: "install_profile.json 顶层非 JSON 对象（源 InvalidOperationException）"
                .to_string(),
            source: None,
        })?;
        if let Some(processors) = profile_value.get("processors").and_then(|p| p.as_array()) {
            if !processors.is_empty() {
                for processor in processors {
                    // 源：`var processorObject = processor!.AsObject();`（条目非对象 →
                    //      InvalidOperationException）
                    let processor_obj = processor.as_object().ok_or_else(|| Error::Params {
                        message: "processor 条目非 JSON 对象（源 InvalidOperationException）"
                            .to_string(),
                        source: None,
                    })?;
                    // 源：`if (!ShouldRunProcessor(processorObject, "client")) continue;`
                    if !ForgeInstallerBase::should_run_processor(processor_obj, "client") {
                        continue;
                    }
                    // 源：try { await RunProcessor(installProfileJson, processorObject,
                    //      versionId, gameDir, javaPath); } catch { BackInstall;
                    //      throw "处理NeoForge处理器失败: {processorObject["jar"]}\n{ex.Message}" }
                    if let Err(e) = base
                        .run_processor(
                            profile_obj,
                            processor_obj,
                            version_id,
                            &base.game_dir,
                            java_path,
                        )
                        .await
                    {
                        back_install(&back_files, &back_dirs);
                        let jar = processor.get("jar").and_then(|j| j.as_str()).unwrap_or("");
                        return Err(Error::DownloadFailed {
                            message: format!("处理NeoForge处理器失败: {jar}\n{}", e),
                            source: Some(Box::new(e)),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// 获取 NeoForge 缺失库列表（源：`GetMissNeoForgeLibraries(string
    /// neoForgeInstallerPath, string versionId)`）。
    ///
    /// 逐字流程：
    /// 1. 重读安装器 ZIP 内 version.json / install_profile.json（失败 →
    ///    「读取NeoForge安装器内容失败，请检查安装器文件是否正确」）；
    /// 2. 库来源三合一：install_profile.json 的 libraries + version.json 的 libraries
    ///    （get_libraries_from_json）+ processors args 内 `[Maven坐标]` 片段
    ///    （extract_maven_coordinates_from_processors）；再 check_libs_ver_static
    ///    按库名去重取组内最高版本；
    /// 3. 逐个库：`{gameDir}/libraries/{path}` 已存在 → 跳过（源不做 SHA1 校验，
    ///    与 Forge 版不同）；否则解析下载 URL：
    ///    - 库自带 Url 非空 → `SourceId != 0 ? ResolveUrl(Url) : Url`（镜像源映射）；
    ///    - 否则 BaseUrl 含 `|` → 逐备选源拼接 `{base}/{path}`，首个 HEAD 可达
    ///      （is_file_url_available_async，超时 10s）即采用，全部失败则保留最后一次 URL；
    ///    - 否则 `{BaseUrl}/{path}`；
    /// 4. 产出 MissFileData（name = "{Name}-{Version}.jar"、path = 库绝对路径、
    ///    url、sha1 = lib.Hash）。
    ///
    /// ⚠️ 源为同步方法（内部 `IsFileUrlAvailableAsync(url).Result` 同步阻塞等待）；
    /// Rust 侧整体异步化，await 等待（差异见日志 p40）。
    async fn get_miss_neoforge_libraries(
        &self,
        neo_forge_installer_path: &str,
        _version_id: &str,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：try { versionData/installProfileData = ReadSpecifyFileFromZip(...) }
        //     catch { throw "读取NeoForge安装器内容失败，请检查安装器文件是否正确" }
        let read_zip = || -> Result<(String, String), Error> {
            Ok((
                String::from_utf8_lossy(&InstallerBase::read_specify_file_from_zip(
                    neo_forge_installer_path,
                    "version.json",
                )?)
                .into_owned(),
                String::from_utf8_lossy(&InstallerBase::read_specify_file_from_zip(
                    neo_forge_installer_path,
                    "install_profile.json",
                )?)
                .into_owned(),
            ))
        };
        let (version_data, install_profile_data) =
            read_zip().map_err(|e| Error::DownloadFailed {
                message: "读取NeoForge安装器内容失败，请检查安装器文件是否正确".to_string(),
                source: Some(Box::new(e)),
            })?;

        // 源：`var libs = GetLibrariesFromJson(installProfileData!);
        //       libs.AddRange(GetLibrariesFromJson(versionData!));`
        let mut libs = get_libraries_from_json(&install_profile_data)?;
        libs.extend(get_libraries_from_json(&version_data)?);

        // 源：`foreach (var coordinate in ExtractMavenCoordinatesFromProcessors(
        //      JsonNode.Parse(installProfileData!)!.AsObject()))
        //      libs.Add(new ForgeInstaller.LibInfo { FullName = coordinate });`
        let profile_value: Value =
            serde_json::from_str(&install_profile_data).map_err(|e| Error::Http {
                message: format!("install_profile.json 解析失败: {e}"),
                status: None,
                source: Some(Box::new(e)),
            })?;
        // 源 `AsObject()`（顶层非对象 → InvalidOperationException）；P38 关联函数接收 &Map
        let profile_obj = profile_value.as_object().ok_or_else(|| Error::Http {
            message: "install_profile.json 顶层非 JSON 对象（源 InvalidOperationException）"
                .to_string(),
            status: None,
            source: None,
        })?;
        for coordinate in ForgeInstallerBase::extract_maven_coordinates_from_processors(profile_obj)
        {
            libs.push(LibInfo::new(coordinate));
        }

        // 源：`libs = ForgeInstaller.CheckLibsVerStatic(libs);`
        libs = check_libs_ver_static(libs);

        let mut miss_files = Vec::new();
        for lib in libs {
            // 源：`var libPath = Path.Combine(gameDir, "libraries", lib.Path);`
            let lib_path = path_combine(&path_combine(&self.base.game_dir, "libraries"), &lib.path);
            // 源：`if (!File.Exists(libPath))`（已存在直接跳过，不做 SHA1 校验）
            if !Path::new(&lib_path).is_file() {
                let mut url = String::new();
                // 源：`if (!string.IsNullOrEmpty(lib.Url))
                //      url = SourceId != 0 ? ResolveUrl(lib.Url) : lib.Url;`
                if !lib.url.is_empty() {
                    url = if self.base.source_id != 0 {
                        self.base.resolve_url(&lib.url)
                    } else {
                        lib.url.clone()
                    };
                } else if self.base.base_url.contains('|') {
                    // 源：`var baseUrls = BaseUrl.Split("|");
                    //      foreach { url = $"{baseUrl}/{lib.Path}";
                    //      if (IsFileUrlAvailableAsync(url).Result) break; }`
                    for base_url in self.base.base_url.split('|') {
                        url = format!("{base_url}/{}", lib.path);
                        if ForgeInstallerBase::is_file_url_available_async(&url, 10).await {
                            break;
                        }
                    }
                } else {
                    // 源：`url = $"{BaseUrl}/{lib.Path}";`
                    url = format!("{}/{}", self.base.base_url, lib.path);
                }

                // 源：`missFiles.Add(new MissFileData($"{lib.Name}-{lib.Version}.jar",
                //      libPath, url, lib.Hash));`
                miss_files.push(MissFileData {
                    name: format!("{}-{}.jar", lib.name, lib.version),
                    path: lib_path,
                    url,
                    sha1: lib.hash,
                });
            }
        }
        Ok(miss_files)
    }
}

/// 源 InstallAsync 的 trait 实现（para1 = javaPath，para2 = neoForgeInstallerPath，
/// para3/para4 源未使用）。签名按 P35 契约（installer.rs，已交付）核对。
#[async_trait]
impl Installer for NeoForgeInstaller {
    /// 执行 NeoForge 安装（源：`InstallAsync(string versionId, string inheritsFromJson,
    /// string? javaPath, string? neoForgeInstallerPath, string? para3, string? para4)`）。
    ///
    /// 源用 `string.IsNullOrEmpty` 校验：javaPath / neoForgeInstallerPath 为 null 或
    /// 空串 → ArgumentNullException → Error::Params；随后写入 `_installerPath` /
    /// `_mainJarPath` 并委托 install_neoforge。
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        let java_path = para1
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::Params {
                message: "javaPath 为 null 或空（源 ArgumentNullException）".to_string(),
                source: None,
            })?;
        let neo_forge_installer_path =
            para2
                .filter(|p| !p.is_empty())
                .ok_or_else(|| Error::Params {
                    message: "neoForgeInstallerPath 为 null 或空（源 ArgumentNullException）"
                        .to_string(),
                    source: None,
                })?;
        self.install_neoforge(
            version_id,
            inherits_from_json,
            java_path,
            neo_forge_installer_path,
        )
        .await
    }

    /// 获取缺失库列表（源：`GetMissLibrariesAsync(string? para1, string? para2,
    /// string? para3)`）。
    ///
    /// para1 = neoForgeInstallerPath 为 null → 空列表（源 Task.FromResult(空列表)）；
    /// para2 = versionId 源以 `para2!` 强制解引用（null → NullReferenceException）→
    /// Error::Params；para3 源未使用。
    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        let Some(neo_forge_installer_path) = para1 else {
            return Ok(Vec::new());
        };
        let version_id = para2.ok_or_else(|| Error::Params {
            message: "versionId 为 null（源 NullReferenceException，para2! 强制解引用）"
                .to_string(),
            source: None,
        })?;
        self.get_miss_neoforge_libraries(neo_forge_installer_path, version_id)
            .await
    }
}

/// 回滚已写入文件与新建目录（源：`BackInstall(List<string> files, List<string> dirs)`，static）。
///
/// - 文件逐个删除，删除失败静默忽略（源 catch 空体）；
/// - 目录：Distinct() 去重 + 按路径长度降序（OrderByDescending，先删子目录再删父目录），
///   仅删除存在且为空（EnumerateFileSystemEntries 无条目）的目录，失败静默忽略。
fn back_install(files: &[String], dirs: &[String]) {
    for file in files {
        // 源：`try { if (File.Exists(file)) File.Delete(file); } catch { }`
        let _ = std::fs::remove_file(file);
    }
    // 源：`dirs.Distinct().OrderByDescending(d => d.Length).ToList()`
    let mut dir_list = dirs.to_vec();
    dir_list.sort_by(|a, b| b.len().cmp(&a.len()));
    dir_list.dedup();
    for dir in dir_list {
        // 源：`if (Directory.Exists(dir) && !Directory.EnumerateFileSystemEntries(dir).Any())
        //      Directory.Delete(dir, false);`（false → 仅删空目录；异常 catch 忽略）
        let empty = Path::new(&dir).is_dir()
            && std::fs::read_dir(&dir)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
        if empty {
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

/// 库信息（源：`ForgeInstaller.LibInfo`，ForgeInstaller.cs 嵌套类；NeoForgeInstaller
/// 跨类使用其 `FullName`/`Name`/`Version`/`Path`/`Hash`/`Url`）。
///
/// ⚠️ UNMAPPED/待去重：源定义在 ForgeInstaller 类内（ForgeInstaller.cs），P39 并行写
/// forge/install.rs 时可能重复定义 —— 交付后若 P39 暴露共享版本应合并去重。
/// P42 Cleanroom 安装器（源 CleanroomInstaller.cs 复用 ForgeInstaller.LibInfo /
/// CheckLibsVerStatic）选用本文件为共享版，字段加宽 pub(crate)（见日志 p42）。
/// 派生规则（源 FullName setter）：坐标按 ':' 分割，不足 3 段 → name/path/version
/// 保持空；version = 第 3 段、name = "{group}.{artifact}"、
/// path = InstallerBase.MavenToPath(FullName)。
#[derive(Debug, Clone)]
pub(crate) struct LibInfo {
    /// 完整 Maven 坐标（源：`FullName`）
    pub(crate) full_name: String,
    /// 库名 "group.artifact"（源：`Name` 派生属性）
    pub(crate) name: String,
    /// 库相对路径（源：`Path` 派生属性）
    pub(crate) path: String,
    /// 版本号（源：`Version` 派生属性）
    pub(crate) version: String,
    /// SHA1 校验值（源：`Hash` 公开字段，缺省空串）
    pub(crate) hash: String,
    /// 下载地址（源：`Url` 公开字段，缺省空串）
    pub(crate) url: String,
}

impl LibInfo {
    /// 以完整坐标构造（源：`FullName` setter）。
    ///
    /// 空坐标 → 各派生字段保持空（源 setter 提前 return）；坐标少于 3 段 →
    /// 不派生 name/path/version（源仅 `temp.Length >= 3` 时赋值）。
    pub(crate) fn new(full_name: String) -> Self {
        let mut info = Self {
            full_name,
            name: String::new(),
            path: String::new(),
            version: String::new(),
            hash: String::new(),
            url: String::new(),
        };
        if info.full_name.is_empty() {
            return info;
        }
        let parts: Vec<&str> = info.full_name.split(':').collect();
        if parts.len() >= 3 {
            info.version = parts[2].to_string();
            info.name = format!("{}.{}", parts[0], parts[1]);
            info.path = InstallerBase::maven_to_path(&info.full_name);
        }
        info
    }
}

/// 按库名去重并取组内版本最高者（源：`ForgeInstaller.CheckLibsVerStatic`，static）。
///
/// 分组键 = Name（"group.artifact"）；组内比较用 `string.Compare(lib.Version,
/// newest.Version, Ordinal) > 0` 取更高版本（Rust String 按字节序比较，版本号均为
/// ASCII，语义等价，见 util/lib_helper.rs 先例）；保留各组首次出现顺序（同 GroupBy）。
pub(crate) fn check_libs_ver_static(libs: Vec<LibInfo>) -> Vec<LibInfo> {
    let mut best: Vec<(String, LibInfo)> = Vec::new();
    for lib in libs {
        match best.iter_mut().find(|(key, _)| key == &lib.name) {
            Some((_, newest)) => {
                if lib.version.cmp(&newest.version) == Ordering::Greater {
                    *newest = lib;
                }
            }
            None => best.push((lib.name.clone(), lib)),
        }
    }
    best.into_iter().map(|(_, lib)| lib).collect()
}

/// 从版本/安装配置 JSON 提取库列表（源：`NeoForgeInstaller.GetLibrariesFromJson`，static）。
///
/// 逐字语义：libraries 字段缺失或非数组 → 「libraries字段不存在或格式错误」；
/// 库条目无 name 或 name 为空 → 跳过；downloads.artifact 存在时取
/// sha1（缺省空串）/ url（缺省空串）。⚠️ 防御性差异：源对 name 为 JSON null
/// （`libObj["name"]!` → NullReferenceException）或 artifact 非对象
/// （InvalidOperationException）会抛异常，Rust 侧视为缺失/空跳过。
pub(crate) fn get_libraries_from_json(json_data: &str) -> Result<Vec<LibInfo>, Error> {
    let data: Value = serde_json::from_str(json_data).map_err(|e| Error::Http {
        message: format!("JSON 解析失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let Some(libraries) = data.get("libraries").and_then(|l| l.as_array()) else {
        return Err(Error::Params {
            message: "libraries字段不存在或格式错误".to_string(),
            source: None,
        });
    };

    let mut libs = Vec::new();
    for item in libraries {
        // 源：`var libObj = item!.AsObject();`（条目非对象 → InvalidOperationException）
        let Some(lib_obj) = item.as_object() else {
            return Err(Error::Params {
                message: "库条目非 JSON 对象（源 InvalidOperationException）".to_string(),
                source: None,
            });
        };
        // 源：`if (libObj.ContainsKey("name")) { var name = libObj["name"]!.ToString();
        //      if (!string.IsNullOrEmpty(name)) { ... } }`
        if let Some(name) = lib_obj.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                let mut info = LibInfo::new(name.to_string());
                if lib_obj.contains_key("downloads") {
                    // 源：`var artifact = libObj["downloads"]?["artifact"];
                    //      info.Hash = artifact?["sha1"]?.ToString() ?? string.Empty;
                    //      info.Url = artifact?["url"]?.ToString() ?? string.Empty;`
                    let artifact = lib_obj.get("downloads").and_then(|d| d.get("artifact"));
                    info.hash = artifact
                        .and_then(|a| a.get("sha1"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    info.url = artifact
                        .and_then(|a| a.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                }
                libs.push(info);
            }
        }
    }
    Ok(libs)
}

/// 模拟 C# `Path.Combine(a, b)` 的拼接语义（源大量用于版本目录 / 库路径拼接）：
/// a 以分隔符（/ 或 \）结尾 → 直接拼接；否则插入平台主分隔符（Windows: \，其余: /）。
/// C# Combine 仅字符串拼接 + 分隔符判断，不规范化路径。
fn path_combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if a.ends_with(['/', '\\']) {
        format!("{a}{b}")
    } else {
        format!("{a}{}{b}", std::path::MAIN_SEPARATOR)
    }
}
