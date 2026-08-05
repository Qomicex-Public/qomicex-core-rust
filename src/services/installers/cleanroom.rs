//! Cleanroom 安装器（B9）
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/CleanroomInstaller.cs（199 行）
//!
//! 流程要点（逐字保留，详见翻译日志 p42-optifine-cleanroom.md）：
//! - InstallAsync：仅 installerPath 校验（ArgumentNullException）；javaPath 等其余参数
//!   源声明但未使用（逐字保留签名）；
//! - InstallCleanroom：zip 内读取 version.json + install_profile.json（失败 →
//!   「读取 Cleanroom 安装器内容失败，请检查安装器文件是否正确」）→ version.json 的
//!   id 覆写为 versionId → 写 `{gameDir}/versions/{versionId}/{versionId}.json`
//!   （紧凑序列化，源 `ToJsonString()` 无缩进；失败回滚 + 「写出 Cleanroom 版本 Json
//!   失败」）→ install_profile.json 的 `path` 坐标经 MavenToPath → zip 内
//!   `maven/{路径}` 提取核心 jar 落 `{gameDir}/libraries/{路径}`（失败回滚 +
//!   「提取 Cleanroom 核心 Jar 失败」）→ 缺失库下载（失败回滚 + 「下载 Cleanroom 缺失
//!   库失败」）；
//! - BackInstall（Cleanroom 版，与 Forge 版不同，不跨文件复用）：删除已写文件（缺失
//!   静默）；目录先 Distinct 去重再按长度降序，仅删除空目录（非递归，
//!   `Directory.Delete(dir, false)` + 空检查）；
//! - GetMissCleanroomLibraries：install_profile 与 version 两处 libraries 合并 →
//!   CheckLibsVerStatic（按 Name 去重取 Version 最大）→ 已存在跳过 → URL：lib.Url
//!   非空用原值，否则 sourceId == 1 → BMCLAPI maven 镜像
//!   （`https://bmclapi2.bangbang93.com/maven/{路径}`），其余 → Maven 中央仓库
//!   （`https://repo.maven.apache.org/maven2/{路径}`）；MissFileData 文件名
//!   `{Name}-{Version}.jar`；
//! - 复用 Forge 系共享工具：源 `ForgeInstaller.LibInfo` / `ForgeInstaller.CheckLibsVerStatic`
//!   （定义于 ForgeInstaller.cs，非 ForgeInstallerBase.cs）→ 复用 neoforge/install.rs 的
//!   pub(crate) 共享版（本文件已加宽字段可见性，见日志 p42）。
//!
//! 错误映射（沿用安装器域定案）：ArgumentNullException / 校验 / JSON 解析（源
//! JsonException）/ zip 读取失败 → Error::Params；文件 IO / 下载 → Error::DownloadFailed。

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Error;
use crate::services::installers::installer::{Installer, InstallerBase, MissFileData};
use crate::services::installers::neoforge::install::{check_libs_ver_static, LibInfo};

/// Cleanroom 安装器（源：`internal class CleanroomInstaller : InstallerBase, IInstaller`）。
///
/// 源继承 InstallerBase 仅为复用其静态工具（无实例基类状态）→ 直接持有两字段
/// （源 readonly 字段）。
pub(crate) struct CleanroomInstaller {
    /// 下载源编号（源 `_sourceId`）：1 → BMCLAPI maven 镜像，其余 → Maven 中央仓库
    /// （仅缺失库 URL 兜底使用）
    source_id: i32,
    /// 游戏根目录（源 `_gameDir`，readonly）
    game_dir: String,
}

impl CleanroomInstaller {
    /// 创建 Cleanroom 安装器（源：`CleanroomInstaller(int sourceId, string gameDir)`）。
    pub(crate) fn new(source_id: i32, game_dir: String) -> Self {
        Self { source_id, game_dir }
    }

    /// 安装主体（源：`InstallCleanroom`，private async）。
    ///
    /// 失败回滚语义：版本 JSON 写出 / 核心 jar 提取 / 缺失库下载三类失败均先
    /// BackInstall 再抛错（回滚清单仅含已成功写入的文件与新建目录）。
    async fn install_cleanroom(&self, version_id: &str, installer_path: &str) -> Result<(), Error> {
        // 源：`List<string> backFiles = []; List<string> backDirs = [];`
        let mut back_files: Vec<String> = Vec::new();
        let mut back_dirs: Vec<String> = Vec::new();

        // 源：`try { versionJsonData = UTF8.GetString(ReadSpecifyFileFromZip(installer, "version.json"));
        //      installProfileData = ...("install_profile.json"); }
        //      catch (Exception ex) { throw new Exception("读取 Cleanroom 安装器内容失败，请检查安装器文件是否正确", ex); }`
        let (version_json_data, install_profile_data) =
            (|| -> Result<(String, String), Error> {
                let version_bytes =
                    InstallerBase::read_specify_file_from_zip(installer_path, "version.json")?;
                let profile_bytes = InstallerBase::read_specify_file_from_zip(
                    installer_path,
                    "install_profile.json",
                )?;
                Ok((
                    // 源 Encoding.UTF8.GetString：无效序列替换（有损）
                    String::from_utf8_lossy(&version_bytes).into_owned(),
                    String::from_utf8_lossy(&profile_bytes).into_owned(),
                ))
            })()
            .map_err(|e| Error::Params {
                message: "读取 Cleanroom 安装器内容失败，请检查安装器文件是否正确".to_string(),
                source: Some(Box::new(e)),
            })?;

        // 源：`var versionJson = JsonNode.Parse(versionJsonData)!.AsObject();`
        let mut version_json: Value =
            serde_json::from_str(&version_json_data).map_err(|e| Error::Params {
                message: format!("version.json 解析失败（源 JsonException）: {e}"),
                source: None,
            })?;
        // 源：`var installProfileJson = JsonNode.Parse(installProfileData)!.AsObject();`
        let install_profile_json: Value =
            serde_json::from_str(&install_profile_data).map_err(|e| Error::Params {
                message: format!("install_profile.json 解析失败（源 JsonException）: {e}"),
                source: None,
            })?;

        // 源：`versionJson["id"] = versionId;`
        if let Some(obj) = version_json.as_object_mut() {
            obj.insert("id".to_string(), Value::String(version_id.to_string()));
        }

        // 源：`var versionDir = Path.Combine(_gameDir, "versions", versionId);
        //      if (!Directory.Exists(versionDir)) { Directory.CreateDirectory(versionDir); backDirs.Add(versionDir); }`
        let version_dir = path_combine(&path_combine(&self.game_dir, "versions"), version_id);
        if !Path::new(&version_dir).is_dir() {
            std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建版本目录失败: {version_dir}"),
                source: Some(Box::new(e)),
            })?;
            back_dirs.push(version_dir.clone());
        }

        // 源：`File.WriteAllText(targetJsonPath, versionJson.ToJsonString());`（紧凑序列化，
        //      无 WriteIndented）；失败 → BackInstall + 「写出 Cleanroom 版本 Json 失败: {原因}」
        let target_json_path = path_combine(&version_dir, &format!("{version_id}.json"));
        let json_content = serde_json::to_string(&version_json).map_err(|e| Error::Params {
            message: format!("序列化版本 JSON 失败: {e}"),
            source: None,
        })?;
        if let Err(e) = std::fs::write(&target_json_path, &json_content) {
            back_install(&back_files, &back_dirs);
            return Err(Error::DownloadFailed {
                message: format!("写出 Cleanroom 版本 Json 失败: {e}"),
                source: Some(Box::new(e)),
            });
        }
        back_files.push(target_json_path);

        // 源：`var cleanroomMavenCoord = installProfileJson["path"]?.ToString();`
        let cleanroom_maven_coord = install_profile_json
            .get("path")
            .map(json_node_to_string)
            .unwrap_or_default();
        if !cleanroom_maven_coord.is_empty() {
            // 源：`var jarRelPath = MavenToPath(cleanroomMavenCoord);`
            let jar_rel_path = InstallerBase::maven_to_path(&cleanroom_maven_coord);
            if !jar_rel_path.is_empty() {
                // 源：`var jarEntryPath = $"maven/{jarRelPath.Replace('\\', '/')}";`
                let jar_entry_path = format!("maven/{}", jar_rel_path.replace('\\', "/"));
                // 源：`try { 读 zip → 建目录 → WriteAllBytes } catch { BackInstall;
                //      throw new Exception($"提取 Cleanroom 核心 Jar 失败: {ex.Message}"); }`
                let extract_result: Result<(), Error> = (|| {
                    let jar_bytes =
                        InstallerBase::read_specify_file_from_zip(installer_path, &jar_entry_path)?;
                    let jar_full_path =
                        path_combine(&path_combine(&self.game_dir, "libraries"), &jar_rel_path);
                    // 源：`var jarDir = Path.GetDirectoryName(jarFullPath);
                    //      if (!string.IsNullOrEmpty(jarDir) && !Directory.Exists(jarDir)) { ...; backDirs.Add(jarDir); }`
                    if let Some(jar_dir) = Path::new(&jar_full_path).parent() {
                        let jar_dir = jar_dir.to_string_lossy().to_string();
                        if !jar_dir.is_empty() && !Path::new(&jar_dir).is_dir() {
                            std::fs::create_dir_all(&jar_dir).map_err(|e| Error::DownloadFailed {
                                message: format!("创建核心 Jar 目录失败: {jar_dir}"),
                                source: Some(Box::new(e)),
                            })?;
                            back_dirs.push(jar_dir);
                        }
                    }
                    std::fs::write(&jar_full_path, &jar_bytes).map_err(|e| Error::DownloadFailed {
                        message: format!("写出核心 Jar 失败: {jar_full_path}"),
                        source: Some(Box::new(e)),
                    })?;
                    back_files.push(jar_full_path);
                    Ok(())
                })();
                if let Err(e) = extract_result {
                    back_install(&back_files, &back_dirs);
                    return Err(Error::DownloadFailed {
                        message: format!("提取 Cleanroom 核心 Jar 失败: {e}"),
                        source: Some(Box::new(e)),
                    });
                }
            }
        }

        // 源：`var libs = GetMissCleanroomLibraries(installerPath, versionId);
        //      foreach (var lib in libs) { try { await DownloadFileAsync(CreateHttpClient(), lib.Url, lib.Path); }
        //      catch (Exception ex) { BackInstall; throw new Exception($"下载 Cleanroom 缺失库失败: {lib.Path}\n{ex.Message}"); } }`
        let libs = self.get_miss_cleanroom_libraries(installer_path, version_id)?;
        for lib in &libs {
            if let Err(e) = InstallerBase::download_file_async(
                &InstallerBase::create_http_client(),
                &lib.url,
                &lib.path,
                5,
            )
            .await
            {
                back_install(&back_files, &back_dirs);
                return Err(Error::DownloadFailed {
                    message: format!("下载 Cleanroom 缺失库失败: {}\n{}", lib.path, e),
                    source: Some(Box::new(e)),
                });
            }
        }
        Ok(())
    }

    /// 计算缺失的 Cleanroom 库文件（源：`GetMissCleanroomLibraries`，public）。
    ///
    /// 库来源：install_profile.json 与 version.json 的 `libraries` 合并 →
    /// CheckLibsVerStatic（按 Name 去重取 Version 最大）；本地已存在 → 跳过；URL 取
    /// lib.Url，为空时按 sourceId 兜底（1 → BMCLAPI maven 镜像，其余 → Maven 中央仓库）。
    ///
    /// ⚠️ 源参数 versionId 在方法体内未使用（保留签名 → `_version_id`）。
    pub(crate) fn get_miss_cleanroom_libraries(
        &self,
        installer_path: &str,
        _version_id: &str,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：zip 读取（同 InstallCleanroom，失败 → 同一错误文案；此处无内层包装）
        let (version_json_data, install_profile_data) =
            (|| -> Result<(String, String), Error> {
                let version_bytes =
                    InstallerBase::read_specify_file_from_zip(installer_path, "version.json")?;
                let profile_bytes = InstallerBase::read_specify_file_from_zip(
                    installer_path,
                    "install_profile.json",
                )?;
                Ok((
                    String::from_utf8_lossy(&version_bytes).into_owned(),
                    String::from_utf8_lossy(&profile_bytes).into_owned(),
                ))
            })()
            .map_err(|_| Error::Params {
                message: "读取 Cleanroom 安装器内容失败，请检查安装器文件是否正确".to_string(),
                source: None,
            })?;

        // 源：`allLibs = GetLibInfosFromJson(installProfileData) + GetLibInfosFromJson(versionJsonData);
        //      allLibs = ForgeInstaller.CheckLibsVerStatic(allLibs);`
        let mut all_libs = Vec::new();
        all_libs.extend(Self::get_lib_infos_from_json(&install_profile_data)?);
        all_libs.extend(Self::get_lib_infos_from_json(&version_json_data)?);
        let all_libs = check_libs_ver_static(all_libs);

        let mut miss_files = Vec::new();
        for lib in all_libs {
            // 源：`var libPath = Path.Combine(_gameDir, "libraries", lib.Path);`
            let lib_path = path_combine(&path_combine(&self.game_dir, "libraries"), &lib.path);
            // 源：`if (File.Exists(libPath)) continue;`
            if Path::new(&lib_path).is_file() {
                continue;
            }
            // 源：`var url = lib.Url; if (string.IsNullOrEmpty(url)) { sourceId == 1 ?
            //      $"https://bmclapi2.bangbang93.com/maven/{lib.Path}" :
            //      $"https://repo.maven.apache.org/maven2/{lib.Path}"; }`
            let mut url = lib.url.clone();
            if url.is_empty() {
                url = if self.source_id == 1 {
                    format!("https://bmclapi2.bangbang93.com/maven/{}", lib.path)
                } else {
                    format!("https://repo.maven.apache.org/maven2/{}", lib.path)
                };
            }
            // 源：`new MissFileData($"{lib.Name}-{lib.Version}.jar", libPath, url, lib.Hash)`
            miss_files.push(MissFileData {
                name: format!("{}-{}.jar", lib.name, lib.version),
                path: lib_path,
                url,
                sha1: lib.hash,
            });
        }
        Ok(miss_files)
    }

    /// 从 JSON 提取库列表（源：`internal static List<ForgeInstaller.LibInfo>
    /// GetLibInfosFromJson(string jsonData)`，internal → pub(crate)）。
    ///
    /// 逐字语义：`libraries` 字段缺失或非数组 → 返回空列表（源提前 return libs）；
    /// 库条目无 name / name 为空 → 跳过；`downloads.artifact` 存在时取
    /// sha1（缺省空串）/ url（缺省空串）。
    /// ⚠️ UNMAPPED（见日志 p42 U3）：源 `item!.AsObject()` 对非对象条目抛
    /// InvalidOperationException → Rust `as_object()` None 防御性跳过（同 forge_base.rs
    /// U5 先例）。
    pub(crate) fn get_lib_infos_from_json(json_data: &str) -> Result<Vec<LibInfo>, Error> {
        // 源：`var data = JsonNode.Parse(jsonData)!.AsObject();`（非对象 → InvalidOperationException）
        let data: Value = serde_json::from_str(json_data).map_err(|e| Error::Params {
            message: format!("JSON 解析失败（源 JsonException）: {e}"),
            source: None,
        })?;
        let data_obj = data.as_object().ok_or_else(|| Error::Params {
            message: "JSON 顶层非对象（源 InvalidOperationException）".to_string(),
            source: None,
        })?;
        // 源：`if (!data.TryGetPropertyValue("libraries", out var librariesToken) ||
        //      librariesToken is not JsonArray libraries) return libs;`
        let Some(libraries) = data_obj.get("libraries").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };

        let mut libs = Vec::new();
        for item in libraries {
            // 源：`var libObj = item!.AsObject();`（非对象 → 防御性跳过，见 doc）
            let Some(lib_obj) = item.as_object() else {
                continue;
            };
            // 源：`var name = libObj["name"]?.ToString(); if (string.IsNullOrEmpty(name)) continue;`
            let name = lib_obj
                .get("name")
                .map(json_node_to_string)
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            // 源：`var info = new ForgeInstaller.LibInfo { FullName = name };`
            let mut info = LibInfo::new(name);
            // 源：`if (libObj.TryGetPropertyValue("downloads", out var downloadsToken) &&
            //      downloadsToken is JsonObject downloads)`（downloads 非对象 → 跳过）
            if let Some(downloads) = lib_obj.get("downloads").and_then(|v| v.as_object()) {
                // 源：`var artifact = downloads["artifact"] as JsonObject;`（`as` 转换，
                //      非对象时为 null，不抛异常）
                if let Some(artifact) = downloads.get("artifact").and_then(|v| v.as_object()) {
                    // 源：`info.Hash = artifact["sha1"]?.ToString() ?? string.Empty;`
                    info.hash = artifact
                        .get("sha1")
                        .map(json_node_to_string)
                        .unwrap_or_default();
                    // 源：`info.Url = artifact["url"]?.ToString() ?? string.Empty;`
                    info.url = artifact
                        .get("url")
                        .map(json_node_to_string)
                        .unwrap_or_default();
                }
            }
            libs.push(info);
        }
        Ok(libs)
    }
}

/// 源 InstallAsync 的 trait 实现：para1 = javaPath（源声明但未使用），
/// para2 = installerPath，para3/para4 未使用；inheritsFromJson 参数源未使用。
#[async_trait]
impl Installer for CleanroomInstaller {
    async fn install(
        &self,
        version_id: &str,
        _inherits_from_json: &str,
        _para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：`if (string.IsNullOrEmpty(installerPath)) throw new ArgumentNullException(nameof(installerPath));`
        let installer_path = para2.ok_or_else(|| Error::Params {
            message: "installerPath 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        // 源：`await InstallCleanroom(versionId, installerPath);`
        self.install_cleanroom(version_id, installer_path).await
    }

    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：`if (para1 == null) return Task.FromResult(new List<MissFileData>());`
        let Some(installer_path) = para1 else {
            return Ok(Vec::new());
        };
        // 源：`return Task.FromResult(GetMissCleanroomLibraries(para1, para2!));`
        //      （para2 在方法体内未使用；null → 空串占位，同 forge/install.rs）
        self.get_miss_cleanroom_libraries(installer_path, para2.unwrap_or(""))
    }
}

/// 回滚安装产物（源：`BackInstall`，private static，Cleanroom 版）。
///
/// 与 Forge 版差异（逐字保留，不跨文件复用）：文件直接删除（缺失静默，源 try/catch
/// 空块）；目录先 Distinct 去重（`dirs.Distinct()`）再按长度降序
/// （`OrderByDescending(d => d.Length)`），且仅当目录为空
/// （`!Directory.EnumerateFileSystemEntries(dir).Any()`）时非递归删除
/// （`Directory.Delete(dir, false)`）；任何失败吞掉。
fn back_install(files: &[String], dirs: &[String]) {
    // 源：`foreach (var file in files) { try { if (File.Exists(file)) File.Delete(file); } catch { } }`
    for file in files {
        let _ = std::fs::remove_file(file);
    }
    // 源：`var dirList = dirs.Distinct().OrderByDescending(d => d.Length).ToList();`
    let mut dir_list: Vec<&String> = Vec::new();
    for dir in dirs {
        if !dir_list.contains(&dir) {
            dir_list.push(dir);
        }
    }
    dir_list.sort_by(|a, b| b.len().cmp(&a.len()));
    // 源：`foreach (var dir in dirList) { try { if (Directory.Exists(dir) &&
    //      !Directory.EnumerateFileSystemEntries(dir).Any()) Directory.Delete(dir, false); } catch { } }`
    for dir in dir_list {
        let empty = Path::new(dir).is_dir()
            && std::fs::read_dir(dir)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
        if empty {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// 模拟 C# `JsonNode.ToString()`：`JsonValue(string).ToString()` 返回不带引号的原始
/// 字符串，其余节点（数字/布尔/对象/数组）返回其 JSON 序列化文本（与 forge/install.rs
/// 同语义）。
fn json_node_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
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
