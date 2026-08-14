//! Forge 安装：Legacy + New（B9，高风险）
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/ForgeInstaller.cs（433 行）
//! 基类：ForgeInstallerBase.cs（P38 并行移植 → forge_base.rs，本文件按契约引用）
//!
//! 双流程要点（逐字保留源 InstallAsync 分支，详见翻译日志 p39-forge.md）：
//! - 构造：sourceId == 1 → BMCLAPI 镜像（BaseUrl + SourceMappings 三组域名重写）；
//!   其余 → 官方多源 `https://maven.minecraftforge.net|https://libraries.minecraft.net`
//!   （'|' 分隔，GetMissForgeLibraries 内逐源 HEAD 探测兜底）；
//! - InstallAsync：javaPath/forgeInstallerPath 为 null → ArgumentNullException；
//!   设 _installerPath/_mainJarPath（`versions/{gameVersion}/{gameVersion}.jar`）；
//!   按 IsLegacyForgeInstaller（install_profile.json 无 processors）分流：
//!   Legacy（1.12 前）/ New（installer.jar 流程）；
//! - New（InstallForge）：zip 解出 version.json + install_profile.json + data/client.lzma
//!   （失败报"读取Forge安装器内容失败"）→ profileName 必须为 "forge"（OrdinalIgnoreCase）
//!   → id/inheritsFrom 覆写 → inheritsFromJson 非空则 MergeVersionJson + clientVersion 补写
//!   → 写 {gameDir}/versions/{versionId}/{versionId}.json → client.lzma 落库
//!   libraries/net/minecraftforge/forge/{gameVersion}-{versionId}/ → 改写
//!   data.BINPATCH.client 为带引号 lzma 路径 → 主 jar（path → maven/{MavenToPath}）落库 →
//!   缺失库下载 → processors 逐处理器执行（ShouldRunProcessor side=client，RunProcessor 经
//!   ForgeInstallerBase，java -jar 逻辑在基类）——任何失败 BackInstall 回滚；
//! - Legacy（InstallLegacyForge）：install_profile.json（version.json 缺省时退回 versionInfo）
//!   → profileName 校验 → 版本 JSON 直接生成（合并逻辑同 New）→ 写版本 JSON → 主 jar
//!   （install.filePath ?? maven/{MavenToPath(path)}）落库 → 缺失库下载（失败回滚）；
//! - GetMissForgeLibraries：libraries ?? versionInfo.libraries → clientreq=="false" 跳过 →
//!   已存在（hash 空或 SHA1 校验通过）跳过 → install.path 匹配库坐标时从 zip 写主 jar →
//!   CheckLibsVerStatic（按 Name 去重，Ordinal 取 Version 最大）→ 缺失项 URL：lib.Url 非空走
//!   ResolveUrl 镜像映射（SourceId != 0）；否则 BaseUrl 含 '|' 逐基地址 HEAD 探测首个可用。
//!
//! ⚠️ 协同契约（P38 并行写入 src/services/installers/forge_base.rs，本文件按其签名引用，
//! 以实际为准；编写时尚未落地，假定签名待校对，详见日志待校对清单）：
//! 1. `ForgeInstallerBase::verify_file_sha1(file_path: &str, expected_hash: &str) -> bool`
//!    （源 `internal static bool VerifyFileSha1`，ForgeInstallerBase.cs）
//! 2. `ForgeInstallerBase::is_file_url_available_async(url: &str, timeout_seconds: u64) -> bool`
//!    （源 `internal static async Task<bool> IsFileUrlAvailableAsync(string url, int timeoutSeconds = 10)`；
//!     HEAD 请求，任何失败返回 false，不抛错）
//! 3. `ForgeInstallerBase::resolve_url(source_mappings: &[(String, String)], original_url: &str) -> String`
//!    （源 `internal string ResolveUrl`：SourceMappings 首个 Original 匹配 → Default ?? 原样）
//! 4. `ForgeInstallerBase::should_run_processor(processor: &Value, side: &str) -> bool`
//!    （源 `internal bool ShouldRunProcessor`：sides 缺失/空 → true；否则包含 side，OrdinalIgnoreCase）
//! 5. `ForgeInstallerBase::run_processor(ip_obj: &Value, processor: &Value, game_dir: &str,
//!    game_version: &str, installer_path: &str, main_jar_path: &str, java_path: &str,
//!    base_url: &str) -> Result<(), Error>`
//!    （源 `internal async Task RunProcessor(JsonObject ipObj, JsonObject processor, string versionId,
//!    string gameDir, string javaPath)` 实例方法 → 状态参数化；源 versionId 参数未使用 → 省略）
//!
//! ⚠️ 错误映射（见日志 UNMAPPED）：ArgumentNullException / 通用 Exception（读取安装器内容失败、
//! 安装器版本不正确、本地 JSON 解析失败）→ Error::Params；文件 IO / 库下载 / 处理器执行 → Error::DownloadFailed。

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::services::installers::forge_base::{ForgeInstallerBase, SourcesList};
use crate::services::installers::installer::MissFileData;
use crate::services::installers::installer::{Installer, InstallerBase};
use crate::util::file_helper::normalize_separators;

/// Forge 安装器（源：`internal class ForgeInstaller : ForgeInstallerBase, IInstaller`）。
///
/// 基类字段（SourceId/BaseUrl/SourceMappings/gameDir/gameVersion/_installerPath/_mainJarPath）
/// 内联为本结构字段；_installerPath/_mainJarPath 仅安装流程内使用且 InstallAsync 内无并发，
/// 不落字段、按参数传递（见日志 D1）。
pub(crate) struct ForgeInstaller {
    /// 下载源标识（源 `SourceId`，实例字段；为 1 → BMCLAPI，GetMissForgeLibraries 据此改写 URL）
    source_id: i32,
    /// 下载源基地址（源 `BaseUrl`，实例字段；官方源为 '|' 分隔的多源）
    base_url: String,
    /// 游戏目录（源 `gameDir`，实例字段）
    game_dir: String,
    /// 原始游戏版本号（源 `gameVersion`，实例字段）
    game_version: String,
    /// Forge 基类（P38）：run_processor/resolve_url 等实例方法承载
    base: ForgeInstallerBase,
}

impl ForgeInstaller {
    /// 创建 Forge 安装器（源：`ForgeInstaller(int sourceId, string gameDir, string gameVersion)`）。
    pub(crate) fn new(source_id: i32, game_dir: String, game_version: String) -> Self {
        // 镜像 int → DownloadMirror 映射（与 FabricInstaller 一致：1 → Bmclapi，其余 → Official）
        let mirror = if source_id == 1 {
            DownloadMirror::Bmclapi
        } else {
            DownloadMirror::Official
        };
        // 镜像选择日志：int → DownloadMirror 映射（源无日志，移植约定补充）
        eprintln!("[ForgeInstaller] 镜像选择: {mirror:?} (sourceId={source_id})");
        let (base_url, source_mappings) = if source_id == 1 {
            let mirror_url = "https://bmclapi2.bangbang93.com/maven";
            (
                mirror_url.to_string(),
                vec![
                    // 源 SourcesList 三组映射，逐字
                    ("https://maven.minecraftforge.net".to_string(), mirror_url.to_string()),
                    ("https://files.minecraftforge.net/maven".to_string(), mirror_url.to_string()),
                    ("https://libraries.minecraft.net".to_string(), mirror_url.to_string()),
                ],
            )
        } else {
            // 源：//BaseUrl = "https://maven.minecraftforge.net";
            // 源：BaseUrl = "https://maven.minecraftforge.net|https://libraries.minecraft.net"（'|' 分隔多源，GetMissForgeLibraries 逐源探测）
            (
                "https://maven.minecraftforge.net|https://libraries.minecraft.net".to_string(),
                Vec::new(),
            )
        };
        Self {
            source_id,
            base_url: base_url.clone(),
            game_dir: game_dir.clone(),
            game_version: game_version.clone(),
            base: ForgeInstallerBase {
                base_url,
                source_id,
                game_dir: game_dir.clone(),
                game_version: game_version.clone(),
                installer_path: String::new(),
                main_jar_path: String::new(),
                source_mappings: source_mappings.iter().map(|(o, d)| SourcesList { original: o.clone(), default: d.clone() }).collect(),
            },
        }
    }

    /// 安装 Forge（New 流程，源：`InstallForge`，private async）。
    ///
    /// 高兼容风险点：版本 JSON（id/inheritsFrom 覆写 + 可选合并）、client.lzma 落库 +
    /// data.BINPATCH.client 改写、主 jar（install_profile.path）落库、缺失库下载、
    /// processors 执行；下载/处理器失败 → BackInstall 回滚。
    async fn install_forge(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        java_path: &str,
        forge_installer_path: &str,
        installer_path: &str,
        main_jar_path: &str,
    ) -> Result<(), Error> {
        // 源：List<string> backFiles = []; List<string> backDirs = [];
        let mut back_files: Vec<String> = Vec::new();
        let mut back_dirs: Vec<String> = Vec::new();

        // 源：try { jsonData = UTF8.GetString(ReadSpecifyFileFromZip(installer, "version.json"));
        //      installProfileData = ...("install_profile.json"); clientLzma = ...("data/client.lzma"); }
        //      catch { throw new Exception("读取Forge安装器内容失败，请检查安装器文件是否正确"); }
        let (mut json_data, install_profile_data, client_lzma) =
            (|| -> Result<(String, String, Vec<u8>), Error> {
                let version_bytes = InstallerBase::read_specify_file_from_zip(forge_installer_path, "version.json")?;
                let profile_bytes = InstallerBase::read_specify_file_from_zip(forge_installer_path, "install_profile.json")?;
                let lzma = InstallerBase::read_specify_file_from_zip(forge_installer_path, "data/client.lzma")?;
                Ok((
                    // 源 Encoding.UTF8.GetString：无效序列替换（有损）
                    String::from_utf8_lossy(&version_bytes).into_owned(),
                    String::from_utf8_lossy(&profile_bytes).into_owned(),
                    lzma,
                ))
            })()
            .map_err(|_| Error::Params {
                message: "读取Forge安装器内容失败，请检查安装器文件是否正确".to_string(),
                source: None,
            })?;

        // 源：var installProfileJson = JsonNode.Parse(installProfileData!)!.AsObject();
        let mut install_profile_json: Value = serde_json::from_str(&install_profile_data).map_err(|e| Error::Params {
            message: format!("install_profile.json 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;

        // 源：profileName 提取（IsNullOrEmpty(profile) ? install?.profileName ?? "" : profile）
        let profile_name = get_profile_name(&install_profile_json);
        eprintln!("[ForgeInstaller] InstallForge 检测到 profileName={profile_name}");
        // 源：if (!string.Equals(profileName, "forge", StringComparison.OrdinalIgnoreCase))
        //      throw new Exception("安装器版本不正确，请检查安装器文件是否正确");
        if !profile_name.eq_ignore_ascii_case("forge") {
            return Err(Error::Params {
                message: "安装器版本不正确，请检查安装器文件是否正确".to_string(),
                source: None,
            });
        }

        // 源：var versionData = JsonNode.Parse(jsonData!)!.AsObject();
        //      versionData["id"] = versionId; versionData["inheritsFrom"] = this.gameVersion;
        let mut version_data: Value = serde_json::from_str(&json_data).map_err(|e| Error::Params {
            message: format!("version.json 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;
        if let Some(obj) = version_data.as_object_mut() {
            obj.insert("id".to_string(), Value::String(version_id.to_string()));
            obj.insert("inheritsFrom".to_string(), Value::String(self.game_version.clone()));
        }
        // 源：jsonData = versionData.ToString();
        json_data = serde_json::to_string(&version_data).map_err(|e| Error::Params {
            message: format!("序列化版本 JSON 失败: {e}"),
            source: None,
        })?;

        // 源：if (!string.IsNullOrEmpty(inheritsFromJson)) { MergeVersionJson + clientVersion 补写 }
        if !inherits_from_json.is_empty() {
            // MergeVersionJson 会删除 inheritsFrom，补写 clientVersion 以保留原版版本信息（源注释）
            json_data = InstallerBase::merge_version_json(inherits_from_json, &json_data, Some(version_id));
            let mut merged_obj: Value = serde_json::from_str(&json_data).map_err(|e| Error::Params {
                message: format!("合并后版本 JSON 解析失败（源 JsonException）: {e}"),
                source: None,
            })?;
            if let Some(obj) = merged_obj.as_object_mut() {
                obj.insert("clientVersion".to_string(), Value::String(self.game_version.clone()));
            }
            json_data = serde_json::to_string(&merged_obj).map_err(|e| Error::Params {
                message: format!("序列化版本 JSON 失败: {e}"),
                source: None,
            })?;
        }

        // 源：var versionDir = Path.Combine(gameDir, "versions", versionId);
        //      if (!Directory.Exists(versionDir)) { CreateDirectory; backDirs.Add; }
        let version_dir = path_combine(&path_combine(&self.game_dir, "versions"), version_id);
        if !Path::new(&version_dir).is_dir() {
            std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建版本目录失败: {version_dir}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(version_dir.clone());
        }
        // 源：File.WriteAllText(Path.Combine(versionDir, $"{versionId}.json"), jsonData);
        let target_json_path = path_combine(&version_dir, &format!("{version_id}.json"));
        std::fs::write(&target_json_path, &json_data).map_err(|e| Error::DownloadFailed {
            message: format!("写入版本 JSON 失败: {target_json_path}"),
            source: Some(Box::new(e)),
        })?;
        back_files.push(target_json_path);

        // 源：lzmaDir = Path.Combine(gameDir, "libraries", "net", "minecraftforge", "forge",
        //      $"{gameVersion}-{versionId}")
        let lzma_dir = path_combine(
            &path_combine(
                &path_combine(&path_combine(&path_combine(&self.game_dir, "libraries"), "net"), "minecraftforge"),
                "forge",
            ),
            &format!("{}-{}", self.game_version, version_id),
        );
        if !Path::new(&lzma_dir).is_dir() {
            std::fs::create_dir_all(&lzma_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建 LZMA 目录失败: {lzma_dir}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(lzma_dir.clone());
        }

        // 源：File.WriteAllBytes(clientLzmaPath, clientLzma);
        //      catch (Exception ex) { BackInstall; throw new Exception($"写出LZMA失败: {ex.Message}"); }
        let client_lzma_path = path_combine(&lzma_dir, "client.lzma");
        back_files.push(client_lzma_path.clone());
        if let Err(e) = std::fs::write(&client_lzma_path, &client_lzma) {
            back_install(&back_files, &back_dirs);
            return Err(Error::DownloadFailed {
                message: format!("写出LZMA失败: {e}"),
                source: None,
            });
        }

        // 源：binPatchPath = $"\"{Path.Combine(gameDir, "libraries", ..., "client.lzma")}\""（带引号）
        //      installProfileJson["data"]!["BINPATCH"]!["client"] = binPatchPath;
        let bin_patch_path = format!("\"{}\"", path_combine(&lzma_dir, "client.lzma"));
        // 源 `!`（null-forgiving）：data/BINPATCH 缺失 → NullReferenceException；非对象 → InvalidOperationException
        let data_obj = install_profile_json.get_mut("data").ok_or_else(|| Error::Params {
            message: "install_profile.json 缺少 data（源 NullReferenceException）".to_string(),
            source: None,
        })?;
        let binpatch = data_obj.get_mut("BINPATCH").ok_or_else(|| Error::Params {
            message: "install_profile.json 缺少 data.BINPATCH（源 NullReferenceException）".to_string(),
            source: None,
        })?;
        let binpatch_obj = binpatch.as_object_mut().ok_or_else(|| Error::Params {
            message: "data.BINPATCH 非对象（源 InvalidOperationException）".to_string(),
            source: None,
        })?;
        binpatch_obj.insert("client".to_string(), Value::String(bin_patch_path));

        // 源：var path = installProfileJson["path"]?.ToString()!; （null → 空串，后续 IsNullOrEmpty 分支）
        let path = install_profile_json.get("path").map(json_node_to_string).unwrap_or_default();
        let mut jar_maven_path = String::new();
        if !path.is_empty() {
            jar_maven_path = InstallerBase::maven_to_path(&path);
        }
        if !jar_maven_path.is_empty() {
            // 源：var forgeJar = ReadSpecifyFileFromZip(installer, $@"maven/{jarMavenPath}");
            let forge_jar = InstallerBase::read_specify_file_from_zip(forge_installer_path, &format!("maven/{jar_maven_path}"))?;
            let jar_full_path = path_combine(&path_combine(&self.game_dir, "libraries"), &jar_maven_path);
            // Windows verbatim 路径（\\?\ 前缀）下 '/' 非分隔符，需换成 '\'
            // （详见 file_helper::normalize_separators），否则 std::fs::write 报 os error 123。
            let jar_full_path = normalize_separators(&jar_full_path);
            // 源：Path.GetDirectoryName → 无父目录时为 null → Directory.CreateDirectory(null) 抛 ArgumentNullException
            let jar_dir = Path::new(&jar_full_path)
                .parent()
                .ok_or_else(|| Error::Params {
                    message: "主 jar 路径无父目录（源 ArgumentNullException）".to_string(),
                    source: None,
                })?
                .to_string_lossy()
                .to_string();
            if !Path::new(&jar_dir).is_dir() {
                std::fs::create_dir_all(&jar_dir).map_err(|e| Error::DownloadFailed {
                    message: format!("创建主 jar 目录失败: {jar_dir}"),
                    source: Some(Box::new(e)),
                })?;
                back_dirs.push(jar_dir);
            }
            back_files.push(jar_full_path.clone());
            std::fs::write(&jar_full_path, forge_jar).map_err(|e| Error::DownloadFailed {
                message: format!("写出主 jar 失败: {jar_full_path}"),
                source: Some(Box::new(e)),
            })?;
        }

        // 源：var libs = GetMissForgeLibraries(forgeInstallerPath, versionId);
        //      foreach: await DownloadFileAsync(CreateHttpClient(), lib.Url, lib.Path);
        //      catch (Exception e) { BackInstall; throw new Exception($"下载缺失的库文件失败: {lib.Path}\n{e.Message}"); }
        let libs = self.get_miss_forge_libraries(forge_installer_path, version_id).await?;
        for lib in &libs {
            if let Err(e) = InstallerBase::download_file_async(&InstallerBase::create_http_client(), &lib.url, &lib.path, 5).await {
                back_install(&back_files, &back_dirs);
                return Err(Error::DownloadFailed {
                    message: format!("下载缺失的库文件失败: {}\n{}", lib.path, e),
                    source: None,
                });
            }
        }

        // 源：var processors = installProfileJson["processors"] as JsonArray;
        //      if (processors != null && processors.Count > 0) { ... }
        //
        // ⚠️ ForgeInstallerBase 未派生 Clone（P38 定案）→ 镜像 neoforge/install.rs 的
        // Neoforge 先例（install_neoforge 内的"手工逐字段复制"）：trait 仅提供 `&self`，
        // 故把本次安装动态状态（installer_path / main_jar_path）写入基类副本后再转交
        // run_processor，否则 run_processor/replace_arguments 读到的基类 game_dir /
        // installer_path / main_jar_path 为空，处理器输出路径会解析成相对 `libraries\…`。
        let base = ForgeInstallerBase {
            base_url: self.base.base_url.clone(),
            source_id: self.base.source_id,
            game_dir: self.base.game_dir.clone(),
            game_version: self.base.game_version.clone(),
            installer_path: installer_path.to_string(),
            main_jar_path: main_jar_path.to_string(),
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
        if let Some(processors) = install_profile_json.get("processors").and_then(|v| v.as_array()) {
            if !processors.is_empty() {
                eprintln!("开始执行Processor后处理，共 {} 个处理器", processors.len());
                for processor in processors {
                    // 源：string processorJar = processorObj["jar"]?.ToString() ?? "未知";
                    let processor_jar = processor.get("jar").map(json_node_to_string).unwrap_or_else(|| "未知".to_string());
                    eprintln!("处理Processor: {processor_jar}");
                    // 源：if (!ShouldRunProcessor(processorObj, "client")) continue;
                    //      Trace.WriteLine("该Processor不适用于当前side=client，跳过执行");
                    //      —— 该日志位于 continue 之后，源为不可达死代码，仅以注释保留
                    if !processor
                        .as_object()
                        .map(|m| ForgeInstallerBase::should_run_processor(m, "client"))
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    // 源：await RunProcessor(installProfileJson, processorObj, versionId, gameDir, javaPath);
                    //      catch (Exception ex) { BackInstall; throw new Exception($"处理器执行失败: ..."); }
                    if let Err(ex) = base
                        .run_processor(
                            install_profile_json.as_object().unwrap_or(&Map::new()),
                            processor.as_object().unwrap_or(&Map::new()),
                            version_id,
                            &base.game_dir,
                            java_path,
                        )
                        .await
                    {
                        back_install(&back_files, &back_dirs);
                        return Err(Error::DownloadFailed {
                            message: format!("处理器执行失败: {processor_jar}\n原因：{ex}"),
                            source: None,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// 安装 Legacy Forge（旧版流程，源：`InstallLegacyForge`，private async）。
    ///
    /// 与 New 流程差异：版本 JSON 优先安装包内 version.json，缺省退回 install_profile.json
    /// 的 versionInfo；无 client.lzma / processors；主 jar 坐标取 install.path ?? path，
    /// zip 内文件名取 install.filePath ?? maven/{MavenToPath(path)}；缺失库下载失败同样回滚。
    async fn install_legacy_forge(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        _java_path: &str,
        forge_installer_path: &str,
    ) -> Result<(), Error> {
        // 源：List<string> backFiles = []; List<string> backDirs = [];
        let mut back_files: Vec<String> = Vec::new();
        let mut back_dirs: Vec<String> = Vec::new();

        // 源：try { installProfileData = ...("install_profile.json");
        //          try { jsonData = ...("version.json"); } catch { } }
        //      catch { throw new Exception("读取Forge安装器内容失败，请检查安装器文件是否正确"); }
        //      —— 内层 version.json 读取失败被吞（置空），仅外层失败报错
        let (install_profile_data, mut json_data) = (|| -> Result<(String, String), Error> {
            let profile_bytes = InstallerBase::read_specify_file_from_zip(forge_installer_path, "install_profile.json")?;
            let version = match InstallerBase::read_specify_file_from_zip(forge_installer_path, "version.json") {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => String::new(),
            };
            Ok((String::from_utf8_lossy(&profile_bytes).into_owned(), version))
        })()
        .map_err(|_| Error::Params {
            message: "读取Forge安装器内容失败，请检查安装器文件是否正确".to_string(),
            source: None,
        })?;

        // 源：var installProfileJson = JsonNode.Parse(installProfileData!)!.AsObject();
        let install_profile_json: Value = serde_json::from_str(&install_profile_data).map_err(|e| Error::Params {
            message: format!("install_profile.json 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;

        // 源：jsonData 为空 → 用 versionInfo（缺失报"无法找到版本Json信息"）；否则用安装包内 version.json
        if json_data.is_empty() {
            eprintln!("[ForgeInstaller] InstallLegacyForge 未找到 version.json，使用 install_profile.json 中的 versionInfo");
            json_data = install_profile_json.get("versionInfo").map(json_node_to_string).ok_or_else(|| Error::Params {
                message: "无法找到版本Json信息".to_string(),
                source: None,
            })?;
        } else {
            eprintln!("[ForgeInstaller] InstallLegacyForge 使用安装包内的 version.json");
        }

        // 源：profileName 校验（同 InstallForge，报错文案一致）
        let profile_name = get_profile_name(&install_profile_json);
        eprintln!("[ForgeInstaller] InstallLegacyForge 检测到 profileName={profile_name}");
        if !profile_name.eq_ignore_ascii_case("forge") {
            return Err(Error::Params {
                message: "安装器版本不正确，请检查安装器文件是否正确".to_string(),
                source: None,
            });
        }

        // 源：版本 JSON 覆写 + 合并（同 InstallForge）
        let mut version_data: Value = serde_json::from_str(&json_data).map_err(|e| Error::Params {
            message: format!("版本 JSON 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;
        if let Some(obj) = version_data.as_object_mut() {
            obj.insert("id".to_string(), Value::String(version_id.to_string()));
            obj.insert("inheritsFrom".to_string(), Value::String(self.game_version.clone()));
        }
        let mut json_data = serde_json::to_string(&version_data).map_err(|e| Error::Params {
            message: format!("序列化版本 JSON 失败: {e}"),
            source: None,
        })?;

        if !inherits_from_json.is_empty() {
            json_data = InstallerBase::merge_version_json(inherits_from_json, &json_data, Some(version_id));
            let mut merged_obj: Value = serde_json::from_str(&json_data).map_err(|e| Error::Params {
                message: format!("合并后版本 JSON 解析失败（源 JsonException）: {e}"),
                source: None,
            })?;
            if let Some(obj) = merged_obj.as_object_mut() {
                obj.insert("clientVersion".to_string(), Value::String(self.game_version.clone()));
            }
            json_data = serde_json::to_string(&merged_obj).map_err(|e| Error::Params {
                message: format!("序列化版本 JSON 失败: {e}"),
                source: None,
            })?;
        }

        // 源：创建版本目录 + 写 {versionId}.json（同 InstallForge）
        let version_dir = path_combine(&path_combine(&self.game_dir, "versions"), version_id);
        if !Path::new(&version_dir).is_dir() {
            std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建版本目录失败: {version_dir}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(version_dir.clone());
        }
        let target_json_path = path_combine(&version_dir, &format!("{version_id}.json"));
        std::fs::write(&target_json_path, &json_data).map_err(|e| Error::DownloadFailed {
            message: format!("写入版本 JSON 失败: {target_json_path}"),
            source: Some(Box::new(e)),
        })?;
        back_files.push(target_json_path);

        // 源：jarMavenPath = MavenToPath(install?.path?.ToString()! ?? path?.ToString()!);
        //      filePath = install?.filePath?.ToString() ?? $@"maven/{MavenToPath(path)}";
        let install = install_profile_json.get("install");
        let jar_maven_path = InstallerBase::maven_to_path(
            &install
                .and_then(|i| i.get("path"))
                .map(json_node_to_string)
                .unwrap_or_else(|| install_profile_json.get("path").map(json_node_to_string).unwrap_or_default()),
        );
        let file_path = install
            .and_then(|i| i.get("filePath"))
            .map(json_node_to_string)
            .unwrap_or_else(|| format!("maven/{}", InstallerBase::maven_to_path(&install_profile_json.get("path").map(json_node_to_string).unwrap_or_default())));
        // 源：var forgeJar = ReadSpecifyFileFromZip(installer, filePath!);
        //      （zip 内文件名与落盘名可能不同：filePath 取自 install.filePath，落盘为 MavenToPath(install.path ?? path)）
        let forge_jar = InstallerBase::read_specify_file_from_zip(forge_installer_path, &file_path)?;
        let jar_full_path = path_combine(&path_combine(&self.game_dir, "libraries"), &jar_maven_path);
        // Windows verbatim 路径（\\?\ 前缀）下 '/' 非分隔符，需换成 '\'
        // （详见 file_helper::normalize_separators），否则 std::fs::write 报 os error 123。
        let jar_full_path = normalize_separators(&jar_full_path);
        let jar_dir = Path::new(&jar_full_path)
            .parent()
            .ok_or_else(|| Error::Params {
                message: "主 jar 路径无父目录（源 ArgumentNullException）".to_string(),
                source: None,
            })?
            .to_string_lossy()
            .to_string();
        if !Path::new(&jar_dir).is_dir() {
            std::fs::create_dir_all(&jar_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建主 jar 目录失败: {jar_dir}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(jar_dir);
        }
        back_files.push(jar_full_path.clone());
        std::fs::write(&jar_full_path, forge_jar).map_err(|e| Error::DownloadFailed {
            message: format!("写出主 jar 失败: {jar_full_path}"),
            source: Some(Box::new(e)),
        })?;

        // 源：缺失库下载（失败回滚，同 InstallForge）
        let libs = self.get_miss_forge_libraries(forge_installer_path, version_id).await?;
        for lib in &libs {
            if let Err(e) = InstallerBase::download_file_async(&InstallerBase::create_http_client(), &lib.url, &lib.path, 5).await {
                back_install(&back_files, &back_dirs);
                return Err(Error::DownloadFailed {
                    message: format!("下载缺失的库文件失败: {}\n{}", lib.path, e),
                    source: None,
                });
            }
        }
        Ok(())
    }

    /// 判定安装器是否旧版 Forge（源：`IsLegacyForgeInstaller`）。
    ///
    /// 读 install_profile.json，profileName 非 "forge"（OrdinalIgnoreCase）→ 报"安装器版本不正确"；
    /// 无 processors 或 processors 为空数组 → Legacy。
    pub(crate) fn is_legacy_forge_installer(&self, forge_installer_path: &str) -> Result<bool, Error> {
        let install_profile_data = String::from_utf8_lossy(
            &InstallerBase::read_specify_file_from_zip(forge_installer_path, "install_profile.json")?,
        )
        .into_owned();
        let install_profile_json: Value = serde_json::from_str(&install_profile_data).map_err(|e| Error::Params {
            message: format!("install_profile.json 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;
        let profile_name = get_profile_name(&install_profile_json);
        eprintln!("[ForgeInstaller] IsLegacyForgeInstaller 检测到 profileName={profile_name}");
        if !profile_name.eq_ignore_ascii_case("forge") {
            return Err(Error::Params {
                message: "安装器版本不正确".to_string(),
                source: None,
            });
        }
        // 源：ContainsKey("processors") && installProfileJson["processors"]!.AsArray().Count > 0
        // ⚠️ 微差：源 processors 存在但非数组时 AsArray() 抛 InvalidOperationException；
        // Rust 侧 as_array() → None 视为无 processors（Legacy）。实际安装器恒为数组。
        let has_processors = install_profile_json
            .get("processors")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| !arr.is_empty());
        Ok(!has_processors)
    }

    /// 计算缺失的 Forge 库文件（源：`GetMissForgeLibraries`）。
    ///
    /// 库来源：install_profile.json 的 `libraries`，缺失时退 `versionInfo.libraries`；逐库：
    /// clientreq == "false" 跳过 → 本地已存在（hash 为空 或 SHA1 校验通过）跳过 →
    /// install.path 与库坐标一致时从 zip 直接写主 jar（失败静默继续）→ 收集 →
    /// CheckLibsVerStatic 按 Name 去重取 Version 最大（Ordinal 字节序比较）→
    /// 未落盘库解析下载 URL：lib.Url 非空（SourceId != 0 → ResolveUrl 镜像映射）；
    /// 否则 BaseUrl 含 '|' 时逐基地址 HEAD 探测首个可用（全部失败取最后基地址），
    /// 否则单基地址拼接。
    ///
    /// ⚠️ 源参数 versionId 在方法体内未使用（保留签名 → `_version_id`）。
    /// ⚠️ 源 LibInfo.Hash/Url 恒为空串（构造时未赋值），对应分支为死代码，仍逐字保留。
    async fn get_miss_forge_libraries(&self, forge_installer_path: &str, _version_id: &str) -> Result<Vec<MissFileData>, Error> {
        // 源：installProfileData = ...("install_profile.json"); try { versionData = ...("version.json"); } catch { }
        //      （versionData 仅解析、方法体内从未使用，照源保留读取动作无意义 → 省略，见日志 D2）
        let install_profile_data = String::from_utf8_lossy(
            &InstallerBase::read_specify_file_from_zip(forge_installer_path, "install_profile.json")?,
        )
        .into_owned();
        let install_profile_json: Value = serde_json::from_str(&install_profile_data).map_err(|e| Error::Params {
            message: format!("install_profile.json 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;

        // 源：var profileLibraries = ContainsKey("libraries") ? libs as JsonArray : versionInfo?.libraries as JsonArray;
        //      foreach (var lib in profileLibraries!) —— null → NullReferenceException
        let profile_libraries = install_profile_json
            .get("libraries")
            .and_then(|v| v.as_array())
            .or_else(|| install_profile_json.get("versionInfo").and_then(|v| v.get("libraries")).and_then(|v| v.as_array()))
            .ok_or_else(|| Error::Params {
                message: "install_profile.json 缺少 libraries（源 NullReferenceException）".to_string(),
                source: None,
            })?;

        let mut libs: Vec<LibInfo> = Vec::new();
        for lib in profile_libraries {
            // 源：var libObj = lib!.AsObject();
            let lib_obj = lib.as_object().ok_or_else(|| Error::Params {
                message: "库条目非对象（源 InvalidOperationException）".to_string(),
                source: None,
            })?;
            // 源：if (libObj.ContainsKey("clientreq") && libObj["clientreq"]?.ToString() == "false") continue;
            if lib_obj.get("clientreq").is_some_and(|v| json_node_to_string(v) == "false") {
                continue;
            }
            // 源：var libInfo = new LibInfo { FullName = libObj["name"]?.ToString() ?? string.Empty };
            let lib_info = LibInfo::new(lib_obj.get("name").map(json_node_to_string).unwrap_or_default());
            // 源：var libPath = Path.Combine(gameDir, "libraries", libInfo.Path);
            let lib_path = normalize_separators(&path_combine(&path_combine(&self.game_dir, "libraries"), &lib_info.path));
            if Path::new(&lib_path).is_file() {
                // 源：if (!string.IsNullOrEmpty(Hash) && VerifyFileSha1(libPath, Hash)) continue;
                //      else { if (string.IsNullOrEmpty(Hash)) continue; }
                if !lib_info.hash.is_empty() {
                    if ForgeInstallerBase::verify_file_sha1(&lib_path, &lib_info.hash) {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            // 源：if (installProfileJson["install"] is not null)
            //      { if (install.path?.ToString() == libInfo.FullName) { 写出主jar; continue; } }
            // ⚠️ 补顶层 path fallback：新版 legacy 安装器（如 1.12.2-14.23.5.2864）
            // 的 install_profile.json 无 install 字段，主 jar 坐标在顶层 path，
            // 与 InstallLegacyForge 的 fallback（install.path ?? 顶层 path）保持一致。
            let main_jar_name = install_profile_json
                .get("install")
                .and_then(|i| i.get("path"))
                .map(json_node_to_string)
                .or_else(|| install_profile_json.get("path").map(json_node_to_string));
            if main_jar_name.as_deref() == Some(lib_info.full_name.as_str()) {
                // 源：try { File.WriteAllBytes(libPath, ReadSpecifyFileFromZip(filePath)); }
                //      catch { continue; } continue;（成功/失败均 continue，即不入缺失列表）
                let main_jar_file = install_profile_json
                    .get("install")
                    .and_then(|i| i.get("filePath"))
                    .map(json_node_to_string)
                    .or_else(|| {
                        install_profile_json
                            .get("path")
                            .map(json_node_to_string)
                            .map(|p| format!("maven/{}", InstallerBase::maven_to_path(&p)))
                    });
                if let Some(file_path) = main_jar_file {
                    if let Ok(bytes) = InstallerBase::read_specify_file_from_zip(forge_installer_path, &file_path) {
                        // 释放前确保父目录存在（源 File.WriteAllBytes 要求目录已存在；
                        // 流水线在 InstallLegacyForge 之前扫描时 libraries 目录可能尚未创建）
                        if let Some(parent) = Path::new(&lib_path).parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&lib_path, bytes);
                    }
                }
                continue;
            }
            libs.push(lib_info);
        }

        // 源：libs = CheckLibsVerStatic(libs);
        let libs = check_libs_ver_static(libs);

        let mut miss_files = Vec::new();
        for lib in libs {
            let lib_path = normalize_separators(&path_combine(&path_combine(&self.game_dir, "libraries"), &lib.path));
            if !Path::new(&lib_path).is_file() {
                // 源：url 解析（见方法 doc）
                let mut url = String::new();
                if !lib.url.is_empty() {
                    // 源：SourceId != 0 ? ResolveUrl(lib.Url) : lib.Url
                    url = if self.source_id != 0 {
                        self.base.resolve_url(&lib.url)
                    } else {
                        lib.url.clone()
                    };
                } else if self.base_url.contains('|') {
                    // 源：foreach (baseUrl in BaseUrl.Split("|")) { url = $"{baseUrl}/{lib.Path}";
                    //      if (IsFileUrlAvailableAsync(url).Result) break; }
                    //      （全部不可用时保留最后基地址拼接结果，照源）
                    let base_urls: Vec<&str> = self.base_url.split('|').collect();
                    for base_url in base_urls {
                        url = format!("{base_url}/{}", lib.path);
                        if ForgeInstallerBase::is_file_url_available_async(&url, 10).await {
                            break;
                        }
                    }
                } else {
                    url = format!("{}/{}", self.base_url, lib.path);
                }
                // 源：new MissFileData($"{lib.Name}-{lib.Version}.jar", libPath, url, lib.Hash)
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

/// 源 InstallAsync 的 trait 实现：para1 = javaPath，para2 = forgeInstallerPath，para3/para4 未使用。
#[async_trait]
impl Installer for ForgeInstaller {
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：if (string.IsNullOrEmpty(javaPath)) throw new ArgumentNullException(nameof(javaPath));
        let java_path = para1.ok_or_else(|| Error::Params {
            message: "javaPath 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        // 源：if (string.IsNullOrEmpty(forgeInstallerPath)) throw new ArgumentNullException(nameof(forgeInstallerPath));
        let forge_installer_path = para2.ok_or_else(|| Error::Params {
            message: "forgeInstallerPath 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        // 源：_installerPath = forgeInstallerPath;
        //      _mainJarPath = Path.Combine("versions", gameVersion, $"{gameVersion}.jar")
        //      （相对路径；ReplaceArguments 内 IsPathRooted 判定非根 → {gameDir}/versions/...）
        let installer_path = forge_installer_path;
        let main_jar_path = path_combine(&path_combine("versions", &self.game_version), &format!("{}.jar", self.game_version));
        // 源：if (IsLegacyForgeInstaller(forgeInstallerPath)) InstallLegacyForge else InstallForge
        if self.is_legacy_forge_installer(forge_installer_path)? {
            self.install_legacy_forge(version_id, inherits_from_json, java_path, forge_installer_path)
                .await
        } else {
            self.install_forge(version_id, inherits_from_json, java_path, forge_installer_path, installer_path, &main_jar_path)
                .await
        }
    }

    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：if (para1 == null) return Task.FromResult(new List<MissFileData>());
        let Some(forge_installer_path) = para1 else {
            return Ok(Vec::new());
        };
        // 源：GetMissForgeLibraries(para1, para2!)（para2 在方法体内未使用；null → 空串占位）
        self.get_miss_forge_libraries(forge_installer_path, para2.unwrap_or("")).await
    }
}

/// 库信息（源：`ForgeInstaller.LibInfo` 嵌套类，ForgeInstaller.cs 末尾）。
///
/// FullName setter 语义：空串 → 各属性保持空；非空按 ':' 分割，≥3 段 →
/// version = [2]、name = [0].[1]、path = MavenToPath(fullName)。Hash/Url 为公开字段，
/// 本流程恒为空串（源从不赋值），保留字段与分支（死代码，见日志 D2）。
struct LibInfo {
    full_name: String,
    name: String,
    path: String,
    version: String,
    hash: String,
    url: String,
}

impl LibInfo {
    fn new(full_name: String) -> Self {
        if full_name.is_empty() {
            return Self {
                full_name,
                name: String::new(),
                path: String::new(),
                version: String::new(),
                hash: String::new(),
                url: String::new(),
            };
        }
        let temp: Vec<&str> = full_name.split(':').collect();
        let (version, name, path) = if temp.len() >= 3 {
            (
                temp[2].to_string(),
                format!("{}.{}", temp[0], temp[1]),
                InstallerBase::maven_to_path(&full_name),
            )
        } else {
            (String::new(), String::new(), String::new())
        };
        Self {
            full_name,
            name,
            path,
            version,
            hash: String::new(),
            url: String::new(),
        }
    }
}

/// 库按 Name 去重取 Version 最大（源：`CheckLibsVerStatic`，internal static）。
///
/// LINQ GroupBy 按各键首次出现顺序输出；组内 `string.Compare(lib.Version, newest.Version,
/// Ordinal) > 0` 严格大于才替换（相等保留先出现者）。Rust `>` 为字节序比较，与 Ordinal 一致。
fn check_libs_ver_static(libs: Vec<LibInfo>) -> Vec<LibInfo> {
    let mut result: Vec<LibInfo> = Vec::new();
    for lib in libs {
        match result.iter_mut().find(|existing| existing.name == lib.name) {
            Some(newest) => {
                if lib.version > newest.version {
                    *newest = lib;
                }
            }
            None => result.push(lib),
        }
    }
    result
}

/// JsonNode.ToString() 语义（源在 profileName / clientreq / install.path / filePath 等多处
/// `.ToString()`）：字符串节点返回原始串（无引号），其余节点返回 JSON 文本。
///
/// ⚠️ 微差：源 JsonNode null 节点 → null；Rust Value::Null → "null"（实际安装器无 null 值节点，
/// 见日志 U3）。
fn json_node_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// profileName 提取（源三处同构逻辑：`IsNullOrEmpty(profile?.ToString()) ?
/// install?.profileName?.ToString() ?? "" : profile?.ToString()!`）。
fn get_profile_name(install_profile_json: &Value) -> String {
    let direct = install_profile_json.get("profile").map(json_node_to_string).unwrap_or_default();
    if direct.is_empty() {
        install_profile_json
            .get("install")
            .and_then(|i| i.get("profileName"))
            .map(json_node_to_string)
            .unwrap_or_default()
    } else {
        direct
    }
}

/// 回滚安装产物（源：`BackInstall`，private static）：删除已写文件（缺失静默）与
/// 递归删除新建目录；任何失败吞掉（源 try/catch 空块）。
fn back_install(files: &[String], dirs: &[String]) {
    // 源：foreach (file) { try { if (File.Exists(file)) File.Delete(file); } catch { } }
    for file in files {
        let _ = std::fs::remove_file(file);
    }
    // 源：foreach (dir) { try { if (Directory.Exists(dir)) Directory.Delete(dir, true); } catch { } }
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 模拟 C# `Path.Combine(a, b)`（与 fabric::install 内部同名辅助语义一致）：
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






