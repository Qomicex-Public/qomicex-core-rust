//! LiteLoader 安装器（B9）
//! 对应源：Services/Installers/LiteloaderInstaller.cs（310 行）
//!
//! ⚠️ 结构说明：任务要求新建 flat `liteloader.rs`；实际脚手架（并行任务）已建
//! `liteloader/` 目录（mod.rs 含 `pub mod install;` + 本 stub），installers/mod.rs
//! 已声明 `pub mod liteloader;` —— 再建 liteloader.rs 将与目录并存触发 E0761，
//! 故实现写入本文件（同 fabric/quilt 结构先例），mod.rs 无需修改。
//!
//! ⚠️ 协同契约（P35 并行写入 src/services/installers/installer.rs，已按实际签名核对）：
//! 1. `crate::services::installers::installer::Installer` trait（#[async_trait]，源 IInstaller）：
//!    - `install(&self, version_id: &str, inherits_from_json: &str, para1: Option<&str>,
//!      para2: Option<&str>, para3: Option<&str>, para4: Option<&str>) -> Result<(), Error>`
//!      （源 `Task InstallAsync(string versionId, string inheritsFromJson,
//!      string? modLoaderVersion, string? gameVersion, string? para3, string? para4)`；
//!      ⚠️ 源忽略 inheritsFromJson 参数，基础版本 JSON 从磁盘读取）
//!    - `get_miss_libraries(&self, para1: Option<&str>, para2: Option<&str>,
//!      para3: Option<&str>) -> Result<Vec<MissFileData>, Error>`
//!      （源 `Task<List<MissFileData>> GetMissLibrariesAsync(string? para1..para3)`）
//! 2. `InstallerBase` 静态工具（pub(crate)）：
//!    - `create_http_client() -> reqwest::Client`（源 CreateHttpClient，可选 DefaultUserAgent）
//!    - `merge_version_json(main_version_json: &str, merged_version_json: &str,
//!      default_version_id: Option<&str>) -> String`（源 MergeVersionJson：合并 +
//!      移除 inheritsFrom + 回写 id；失败返回空串）
//!    - `download_file_async(client: &reqwest::Client, url: &str, destination_path: &str,
//!      max_redirects: i32) -> Result<bool, Error>`（源 DownloadFileAsync，默认 5 次重定向）
//! 3. MavenToPath 复用 util::lib_helper::maven_to_path（MAPPING_TABLE utils 既定映射）。
//!
//! 流程要点（逐字保留，详见翻译日志 p41）：
//! - meta API：官方 `https://dl.liteloader.com/versions/versions.json` /
//!   BMCLAPI `https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/versions.json`
//!   （sourceId == 0 → 官方，其余 → BMCLAPI）
//! - 版本查找链：versions[ mcVersion ] → snapshots|artefacts（按序，命中即停）
//!   → "com.mumfrey:liteloader" → 逐属性匹配 liteVersion（version 字段，Ordinal）
//! - 版本 JSON：id=versionId / inheritsFrom=base.GameVersion / type=release /
//!   arguments.game=[--tweakClass, tweakClass] /
//!   mainClass=net.minecraft.launchwrapper.Launch / libraries / logging={}，
//!   经 InstallerBase::merge_version_json 与基础版本 JSON 深合并
//! - 库 URL 回退：lib.url 缺失或空且 sourceId==0 → https://repo.spongepowered.org/maven
//! - 核心库：com.mumfrey:liteloader:{liteVersion}，下载 URL 取 Urls[0]，
//!   失败回退 `{baseRepoUrl.TrimEnd('/')}/{mavenPath}`
//!
//! 错误语义（同 fabric/install.rs 定案）：
//! - 传输层（请求/状态码/响应读取/JSON 解析）→ Error::Http
//! - 文件 IO / 下载 → Error::DownloadFailed
//! - 参数缺失（源 ArgumentNullException）/ 版本信息获取失败、基础版本缺失、核心库构建
//!   异常、安装失败（源普通 Exception）→ Error::Params

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::services::installers::installer::MissFileData;
use crate::services::installers::installer::{Installer, InstallerBase};
use crate::util::lib_helper::maven_to_path;

/// LiteLoader 安装器（源：`internal class LiteloaderInstaller : InstallerBase, IInstaller`）
pub(crate) struct LiteloaderInstaller {
    /// 下载源根地址（源 `_baseRepoUrl`）：sourceId == 1 → BMCLAPI Maven
    /// （`https://bmclapi2.bangbang93.com/maven/`），其余 → 官方
    /// （`https://dl.liteloader.com/versions`）。镜像语义映射 DownloadMirror
    /// （1 → Bmclapi，0 → Official）。
    base_repo_url: String,
    /// 下载源编号（源 `_sourceId`，readonly；0 → 官方 / 其余 → BMCLAPI，
    /// 决定 meta JSON 端点与库 URL 回退）
    source_id: i32,
    /// 游戏根目录（源 `_gameDir`，readonly）
    game_dir: String,
}

impl LiteloaderInstaller {
    /// 创建 LiteLoader 安装器（源：`LiteloaderInstaller(int sourceId, string gameDir,
    /// string gameVersion)`）
    pub(crate) fn new(source_id: i32, game_dir: String, _game_version: String) -> Self {
        // 源：`_baseRepoUrl = sourceId == 1 ? "https://bmclapi2.bangbang93.com/maven/" :
        //   "https://dl.liteloader.com/versions";`（其余值一律按官方处理）
        let mirror = if source_id == 1 {
            DownloadMirror::Bmclapi
        } else {
            DownloadMirror::Official
        };
        // 镜像选择日志：int → DownloadMirror 映射（源无日志，移植约定补充）
        eprintln!("[LiteloaderInstaller] 镜像选择: {mirror:?} (sourceId={source_id})");
        let base_repo_url = if source_id == 1 {
            "https://bmclapi2.bangbang93.com/maven/".to_string()
        } else {
            "https://dl.liteloader.com/versions".to_string()
        };
        Self {
            base_repo_url,
            source_id,
            game_dir,
        }
    }

    /// 安装 LiteLoader 核心（源：InstallLiteLoaderCoreAsync，private）
    async fn install_liteloader_core(
        &self,
        version_id: &str,
        mc_version: &str,
        lite_version: &str,
    ) -> Result<bool, Error> {
        // 源：`var remoteVersion = await GetRemoteVersionByVersionsAsync(...);
        //      if (remoteVersion == null) throw new Exception(...)`
        let remote_version = self
            .get_remote_version_by_versions(mc_version, lite_version)
            .await?;
        let Some(remote_version) = remote_version else {
            return Err(Error::Params {
                message: format!("无法获取LiteLoader {lite_version}（对应MC {mc_version}）的版本信息"),
                source: None,
            });
        };

        // 源：`var baseVersion = GetBaseMcVersion(mcVersion);
        //      if (baseVersion == null) throw new Exception(...)`
        let base_version = self.get_base_mc_version(mc_version);
        let Some(base_version) = base_version else {
            return Err(Error::Params {
                message: format!("未找到MC {mc_version}的基础版本配置"),
                source: None,
            });
        };

        // 源：`var coreLibrary = CreateCoreLibrary(remoteVersion);
        //      if (coreLibrary.DownloadInfo == null || string.IsNullOrEmpty(coreLibrary.DownloadInfo.Path))
        //          throw new Exception("核心库信息构建异常");`
        let core_library = self.create_core_library(&remote_version);
        let core_library_ok = core_library
            .download_info
            .as_ref()
            .is_some_and(|info| info.path.as_deref().is_some_and(|p| !p.is_empty()));
        if !core_library_ok {
            return Err(Error::Params {
                message: "核心库信息构建异常".to_string(),
                source: None,
            });
        }

        let merged_libraries = Self::merge_libraries(&remote_version.libraries, core_library);

        // 源：`foreach (var lib in mergedLibraries) { if (lib.DownloadInfo?.Path == null ||
        //      lib.DownloadInfo.Url == null || lib.Artifact == null) continue; ... }`
        for lib in &merged_libraries {
            let Some(download_info) = &lib.download_info else {
                continue;
            };
            let Some(path) = &download_info.path else {
                continue;
            };
            let Some(url) = &download_info.url else {
                continue;
            };
            if lib.artifact.is_none() {
                continue;
            }

            // 源：`string localPath = Path.Combine(_gameDir, "libraries", lib.DownloadInfo.Path);
            //      if (File.Exists(localPath)) continue;`
            let local_path = path_combine(&path_combine(&self.game_dir, "libraries"), path);
            if Path::new(&local_path).is_file() {
                continue;
            }

            // 源：`string directory = Path.GetDirectoryName(localPath)!;
            //      if (!Directory.Exists(directory)) Directory.CreateDirectory(directory);`
            if let Some(directory) = Path::new(&local_path).parent() {
                if !directory.is_dir() {
                    std::fs::create_dir_all(directory).map_err(|e| Error::DownloadFailed {
                        message: format!("创建库目录失败: {local_path}"),
                        source: Some(Box::new(e)),
                    })?;
                }
            }

            // 源：`await DownloadFileAsync(CreateHttpClient(), lib.DownloadInfo.Url, localPath);`
            // （每次迭代新建客户端为源逐字行为；返回值被源忽略，失败抛异常传播）
            let client = InstallerBase::create_http_client();
            InstallerBase::download_file_async(&client, url, &local_path, 5).await?;
        }

        // 源：`var versionJson = BuildVersionJson(...);
        //      if (string.IsNullOrEmpty(versionJson)) throw new Exception("构建版本配置失败");`
        let version_json = self.build_version_json(version_id, &base_version, &remote_version, &merged_libraries);
        if version_json.is_empty() {
            return Err(Error::DownloadFailed {
                message: "构建版本配置失败".to_string(),
                source: None,
            });
        }

        // 源：`SaveVersionJson(versionId, _gameDir, versionJson); return true;`
        Self::save_version_json(version_id, &self.game_dir, &version_json)?;
        Ok(true)
    }

    /// 获取远端 LiteLoader 版本信息（源：GetRemoteVersionByVersionsAsync，private）。
    ///
    /// ⚠️ 源 HttpClient 为 `new HttpClient { Timeout = TimeSpan.FromSeconds(5) }`
    /// （独立客户端，不带 DefaultUserAgent）→ 直接构建 reqwest Client（5 秒超时），
    /// 不经 InstallerBase::create_http_client。
    async fn get_remote_version_by_versions(
        &self,
        mc_version: &str,
        lite_version: &str,
    ) -> Result<Option<LiteLoaderRemoteVersion>, Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("创建 HTTP 客户端失败（源 HttpClient 构造不抛异常）");

        // 源：`if (_sourceId == 0) jsonUrl = "https://dl.liteloader.com/versions/versions.json";
        //      else jsonUrl = "https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/versions.json";`
        let json_url = if self.source_id == 0 {
            "https://dl.liteloader.com/versions/versions.json"
        } else {
            "https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/versions.json"
        };

        // 源：`try { jsonContent = await client.GetStringAsync(jsonUrl); } catch { return null; }`
        // （发送失败 / 非 2xx / 读取失败均落入 catch → None；GetStringAsync 内部
        //  EnsureSuccessStatusCode，非 2xx 亦抛异常）
        let json_content = match client.get(json_url).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    return Ok(None);
                }
                match response.text().await {
                    Ok(text) => text,
                    Err(_) => return Ok(None),
                }
            }
            Err(_) => return Ok(None),
        };
        if json_content.is_empty() {
            return Ok(None);
        }

        // 源：`var root = JsonNode.Parse(jsonContent)!.AsObject();`（在 catch 之外，
        // Parse 失败 → JsonException / 非对象 → InvalidOperationException 向上传播）
        let root = serde_json::from_str::<Value>(&json_content).map_err(|e| Error::Http {
            message: format!("LiteLoader meta JSON 解析失败: {e}"),
            source: Some(Box::new(e)),
        })?;
        let root_obj = root.as_object().ok_or_else(|| Error::Http {
            message: "LiteLoader meta 顶层非 JSON 对象".to_string(),
            source: None,
        })?;

        // 源：`if (root["versions"] is not JsonObject versionsObj) return null;`
        let Some(versions_obj) = root_obj.get("versions").and_then(|v| v.as_object()) else {
            return Ok(None);
        };
        // 源：`if (!versionsObj.TryGetPropertyValue(mcVersion, out var mcVersionNode) ||
        //      mcVersionNode is not JsonObject mcObj) return null;`
        let Some(mc_obj) = versions_obj.get(mc_version).and_then(|v| v.as_object()) else {
            return Ok(None);
        };

        // 源：`foreach (string nodeName in new[] { "snapshots", "artefacts" })`：
        // mcObj[nodeName] 为对象且其 ["com.mumfrey:liteloader"] 为对象 → 命中即 break
        let mut lite_loader_versions: Option<&serde_json::Map<String, Value>> = None;
        for node_name in ["snapshots", "artefacts"] {
            if let Some(node_obj) = mc_obj.get(node_name).and_then(|v| v.as_object()) {
                if node_obj.get("com.mumfrey:liteloader").is_some_and(|v| v.is_object()) {
                    lite_loader_versions = node_obj
                        .get("com.mumfrey:liteloader")
                        .and_then(|v| v.as_object());
                    break;
                }
            }
        }
        let Some(lite_loader_versions) = lite_loader_versions else {
            return Ok(None);
        };

        // 源：`foreach (var prop in liteLoaderVersions) { if (prop.Value is not JsonObject
        //      versionObj) continue; ... string.Equals(version, liteVersion, Ordinal) }`
        let mut target_version: Option<&serde_json::Map<String, Value>> = None;
        for (_, version_value) in lite_loader_versions {
            let Some(version_obj) = version_value.as_object() else {
                continue;
            };
            let Some(version) = version_obj.get("version").and_then(|v| v.as_str()) else {
                continue;
            };
            if version == lite_version {
                target_version = Some(version_obj);
                break;
            }
        }
        let Some(target_version) = target_version else {
            return Ok(None);
        };

        // 源：`string? fileName = targetVersion["file"]?.GetValue<string>();
        //      if (string.IsNullOrEmpty(fileName)) return null;`
        let Some(file_name) = target_version.get("file").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        if file_name.is_empty() {
            return Ok(None);
        }

        // 源：`string mainUrl = $"{_baseRepoUrl.TrimEnd('/')}/com/mumfrey/liteloader/{liteVersion}/{fileName}";`
        let main_url = format!(
            "{}/com/mumfrey/liteloader/{lite_version}/{file_name}",
            self.base_repo_url.trim_end_matches('/')
        );

        // 源：`targetVersion["libraries"] is JsonArray` → 逐项构建 Library
        let mut libraries = Vec::new();
        if let Some(libs_array) = target_version.get("libraries").and_then(|v| v.as_array()) {
            for lib_item in libs_array {
                // 源：`if (libItem is not JsonObject libObj) continue;
                //      string? libName = libObj["name"]?.GetValue<string>();
                //      if (string.IsNullOrEmpty(libName)) continue;`
                let Some(lib_obj) = lib_item.as_object() else {
                    continue;
                };
                let Some(lib_name) = lib_obj.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                if lib_name.is_empty() {
                    continue;
                }

                // 源：`string mavenPath = MavenToPath(libName); if (string.IsNullOrEmpty(mavenPath)) continue;`
                let maven_path = maven_to_path(lib_name);
                if maven_path.is_empty() {
                    continue;
                }

                // 源：`string libUrl = libObj["url"]?.GetValue<string>() ?? _baseRepoUrl;
                //      if (_sourceId == 0 && string.IsNullOrEmpty(libObj["url"]?.GetValue<string>()))
                //          libUrl = "https://repo.spongepowered.org/maven";`
                // ⚠️ 源对 url 读取两次：`??` 回退 baseRepoUrl 后，仍以原始 url 值判空
                //（缺失或空 → 命中回退；url 非空 → 取原始 url）
                let lib_url_value = lib_obj.get("url").and_then(|v| v.as_str());
                let mut lib_url = lib_url_value.unwrap_or(&self.base_repo_url).to_string();
                if self.source_id == 0 && lib_url_value.map_or(true, |u| u.is_empty()) {
                    lib_url = "https://repo.spongepowered.org/maven".to_string();
                }

                libraries.push(Library {
                    // 源：`Artifact = ParseMavenCoordinate(libName)`
                    artifact: parse_maven_coordinate(lib_name),
                    // 源：`Url = libUrl`
                    url: Some(lib_url.clone()),
                    // 源：`DownloadInfo = new LibraryDownloadInfo { Path = mavenPath,
                    //      Url = $"{libUrl.TrimEnd('/')}/{mavenPath}" }`
                    download_info: Some(LibraryDownloadInfo {
                        path: Some(maven_path.clone()),
                        url: Some(format!("{}/{}", lib_url.trim_end_matches('/'), maven_path)),
                    }),
                });
            }
        }

        // 源：`string tweakClass = targetVersion["tweakClass"]?.GetValue<string>() ??
        //      "com.mumfrey.liteloader.launch.LiteLoaderTweaker";`
        let tweak_class = target_version
            .get("tweakClass")
            .and_then(|v| v.as_str())
            .unwrap_or("com.mumfrey.liteloader.launch.LiteLoaderTweaker")
            .to_string();

        Ok(Some(LiteLoaderRemoteVersion {
            // 源：`GameVersion = mcVersion, SelfVersion = liteVersion, Urls = [mainUrl], ...`
            game_version: mc_version.to_string(),
            self_version: lite_version.to_string(),
            urls: vec![main_url],
            tweak_class,
            libraries,
            json: None,
        }))
    }

    /// 读取本地基础 MC 版本配置（源：GetBaseMcVersion，private）。
    ///
    /// 文件不存在或读取失败（源 try/catch → null，静默无日志）→ None。
    fn get_base_mc_version(&self, mc_version: &str) -> Option<LiteLoaderRemoteVersion> {
        // 源：`var localVersionPath = Path.Combine(_gameDir, "versions", mcVersion,
        //      $"{mcVersion}.json"); if (!File.Exists(localVersionPath)) return null;`
        let local_version_path = path_combine(
            &path_combine(&path_combine(&self.game_dir, "versions"), mc_version),
            &format!("{mc_version}.json"),
        );
        if !Path::new(&local_version_path).is_file() {
            return None;
        }
        let json_content = match std::fs::read_to_string(&local_version_path) {
            Ok(content) => content,
            Err(_) => return None,
        };
        // 源：`return new LiteLoaderRemoteVersion { GameVersion = mcVersion, Json = jsonContent };`
        Some(LiteLoaderRemoteVersion {
            game_version: mc_version.to_string(),
            self_version: String::new(),
            urls: Vec::new(),
            tweak_class: String::new(),
            libraries: Vec::new(),
            json: Some(json_content),
        })
    }

    /// 构建核心库（源：CreateCoreLibrary，private）。
    ///
    /// 坐标 `com.mumfrey:liteloader:{liteVersion}`；MavenToPath 失败回退
    /// `com/mumfrey/liteloader/{liteVersion}/liteloader-{liteVersion}.jar`；
    /// 下载 URL 取 `remoteVersion.Urls.FirstOrDefault()`，缺省回退
    /// `{_baseRepoUrl.TrimEnd('/')}/{mavenPath}`。
    fn create_core_library(&self, remote_version: &LiteLoaderRemoteVersion) -> Library {
        let lite_version = &remote_version.self_version;
        let maven_coord = format!("com.mumfrey:liteloader:{lite_version}");
        let mut maven_path = maven_to_path(&maven_coord);
        if maven_path.is_empty() {
            maven_path = format!("com/mumfrey/liteloader/{lite_version}/liteloader-{lite_version}.jar");
        }

        let download_url = remote_version
            .urls
            .first()
            .cloned()
            .unwrap_or_else(|| {
                format!("{}/{maven_path}", self.base_repo_url.trim_end_matches('/'))
            });

        Library {
            // 源：`Artifact = new Artifact { GroupId = "com.mumfrey", ArtifactId = "liteloader",
            //      Version = liteVersion }`
            artifact: Some(Artifact {
                group_id: "com.mumfrey".to_string(),
                artifact_id: "liteloader".to_string(),
                version: lite_version.clone(),
            }),
            // 源：`Url = _baseRepoUrl`
            url: Some(self.base_repo_url.clone()),
            // 源：`DownloadInfo = new LibraryDownloadInfo { Path = mavenPath, Url = downloadUrl }`
            download_info: Some(LibraryDownloadInfo {
                path: Some(maven_path),
                url: Some(download_url),
            }),
        }
    }

    /// 合并库列表：核心库坐标已存在于基础库 → 不重复添加（源：MergeLibraries，private static）。
    ///
    /// ⚠️ 源 `coreLibrary.Artifact == null` → 直接 `[.. baseLibraries, coreLibrary]`
    /// （跳过去重检查），Rust 侧逐字保留。
    fn merge_libraries(base_libraries: &[Library], core_library: Library) -> Vec<Library> {
        let Some(core_artifact) = &core_library.artifact else {
            let mut result = base_libraries.to_vec();
            result.push(core_library);
            return result;
        };
        let core_lib_coord = core_artifact.to_string();
        // 源：`baseLibraries.Any(lib => lib.Artifact?.ToString() == coreLibCoord)`
        // （lib.Artifact 为 null → null != coord → 视为不存在；闭包经 is_some_and 比较，
        // 避免 move 捕获 core_lib_coord —— FnMut 限制）
        let core_lib_exists = base_libraries
            .iter()
            .any(|lib| lib.artifact.as_ref().is_some_and(|a| a.to_string() == core_lib_coord));
        let mut result = base_libraries.to_vec();
        if !core_lib_exists {
            result.push(core_library);
        }
        result
    }

    /// 构建 LiteLoader 版本 JSON 并与基础版本 JSON 合并（源：BuildVersionJson，private）。
    ///
    /// 中间 liteJson：id / inheritsFrom=base.GameVersion / type=release /
    /// arguments.game=["--tweakClass", tweakClass] /
    /// mainClass=net.minecraft.launchwrapper.Launch / libraries / logging={}，
    /// `WriteIndented=true` → to_string_pretty；随后
    /// `MergeVersionJson(baseVersion.Json ?? "{}", liteJsonStr, versionId)`
    /// （合并后移除 inheritsFrom、回写 id，由 InstallerBase::merge_version_json 实现）。
    fn build_version_json(
        &self,
        version_id: &str,
        base_version: &LiteLoaderRemoteVersion,
        remote_version: &LiteLoaderRemoteVersion,
        libraries: &[Library],
    ) -> String {
        let libraries_json: Vec<Value> = libraries.iter().map(library_to_json).collect();

        let lite_json = serde_json::json!({
            // 源：`["id"] = versionId`
            "id": version_id,
            // 源：`["inheritsFrom"] = baseVersion.GameVersion`
            "inheritsFrom": &base_version.game_version,
            // 源：`["type"] = "release"`
            "type": "release",
            // 源：`["arguments"] = new JsonObject { ["game"] = new JsonArray("--tweakClass",
            //      remoteVersion.TweakClass ?? "com.mumfrey.liteloader.launch.LiteLoaderTweaker") }`
            // （TweakClass 在构造处已应用 ?? 默认值，字段恒非空）
            "arguments": { "game": ["--tweakClass", &remote_version.tweak_class] },
            // 源：`["mainClass"] = "net.minecraft.launchwrapper.Launch"`
            "mainClass": "net.minecraft.launchwrapper.Launch",
            // 源：`["libraries"] = new JsonArray(libraries.Select(...).ToArray())`
            "libraries": libraries_json,
            // 源：`["logging"] = new JsonObject()`
            "logging": {},
        });

        // 源：`liteJson.ToJsonString(new JsonSerializerOptions { WriteIndented = true })`
        let lite_json_str = serde_json::to_string_pretty(&lite_json).unwrap_or_default();
        // 源：`MergeVersionJson(baseVersion.Json ?? "{}", liteJsonStr, versionId)`
        let base_json = base_version.json.as_deref().unwrap_or("{}");
        InstallerBase::merge_version_json(base_json, &lite_json_str, Some(version_id))
    }

    /// 保存版本 JSON（源：SaveVersionJson，private static）。
    fn save_version_json(version_id: &str, game_dir: &str, json_content: &str) -> Result<(), Error> {
        // 源：`var versionDir = Path.Combine(gameDir, "versions", versionId);
        //      if (!Directory.Exists(versionDir)) Directory.CreateDirectory(versionDir);`
        let version_dir = path_combine(&path_combine(game_dir, "versions"), version_id);
        std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
            message: format!("创建版本目录失败: {version_dir}"),
            source: Some(Box::new(e)),
        })?;
        // 源：`var jsonPath = Path.Combine(versionDir, $"{versionId}.json");
        //      File.WriteAllText(jsonPath, jsonContent);`
        let json_path = path_combine(&version_dir, &format!("{version_id}.json"));
        std::fs::write(&json_path, json_content).map_err(|e| Error::DownloadFailed {
            message: format!("写入版本JSON失败: {json_path}"),
            source: Some(Box::new(e)),
        })?;
        Ok(())
    }
}

/// 源 InstallAsync 的 trait 实现（para1 = modLoaderVersion，para2 = gameVersion，
/// para3/para4 未使用；inheritsFromJson 参数源亦未使用——基础版本 JSON 从磁盘读取）。
/// 签名按 P35 契约推断，installer.rs 交付后按实际校对。
#[async_trait]
impl Installer for LiteloaderInstaller {
    async fn install(
        &self,
        version_id: &str,
        _inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：`if (string.IsNullOrEmpty(modLoaderVersion))
        //      throw new ArgumentNullException(nameof(modLoaderVersion));`
        let mod_loader_version = para1.filter(|v| !v.is_empty()).ok_or_else(|| Error::Params {
            message: "modLoaderVersion 为 null 或空（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        // 源：`if (string.IsNullOrEmpty(gameVersion))
        //      throw new ArgumentNullException(nameof(gameVersion));`
        let game_version = para2.filter(|v| !v.is_empty()).ok_or_else(|| Error::Params {
            message: "gameVersion 为 null 或空（源 ArgumentNullException）".to_string(),
            source: None,
        })?;

        // 源：`bool installResult = await InstallLiteLoaderCoreAsync(versionId, gameVersion,
        //      modLoaderVersion); if (!installResult) throw new Exception(...);`
        let install_result = self
            .install_liteloader_core(version_id, game_version, mod_loader_version)
            .await?;
        if !install_result {
            return Err(Error::Params {
                message: format!("LiteLoader安装失败 - 版本ID: {version_id}"),
                source: None,
            });
        }
        Ok(())
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

/// 库条目（源：`LiteloaderInstaller.Library` 嵌套类；Artifact/Url/DownloadInfo 均可空）。
#[derive(Debug, Clone)]
pub(crate) struct Library {
    /// Maven 坐标（源：`Artifact? Artifact`）
    pub artifact: Option<Artifact>,
    /// 库 URL 域名（源：`string? Url`）
    pub url: Option<String>,
    /// 下载信息（源：`LibraryDownloadInfo? DownloadInfo`）
    pub download_info: Option<LibraryDownloadInfo>,
}

/// Maven 坐标（源：`LiteloaderInstaller.Artifact` 嵌套类；字段默认 string.Empty，
/// `ToString() => $"{GroupId}:{ArtifactId}:{Version}"`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Artifact {
    /// 组 ID（源：`GroupId`）
    pub group_id: String,
    /// 构件 ID（源：`ArtifactId`）
    pub artifact_id: String,
    /// 版本（源：`Version`）
    pub version: String,
}

impl std::fmt::Display for Artifact {
    /// Maven 坐标串（源：`Artifact.ToString()`）
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.group_id, self.artifact_id, self.version)
    }
}

/// 库下载信息（源：`LiteloaderInstaller.LibraryDownloadInfo` 嵌套类；Path/Url 可空）。
#[derive(Debug, Clone)]
pub(crate) struct LibraryDownloadInfo {
    /// 相对路径（源：`string? Path`）
    pub path: Option<String>,
    /// 下载地址（源：`string? Url`）
    pub url: Option<String>,
}

/// LiteLoader 远端版本信息（源：`LiteloaderInstaller.LiteLoaderRemoteVersion` 嵌套类）。
#[derive(Debug, Clone)]
pub(crate) struct LiteLoaderRemoteVersion {
    /// 对应 MC 版本（源：`GameVersion`，默认 string.Empty）
    pub game_version: String,
    /// LiteLoader 自身版本（源：`SelfVersion`，默认 string.Empty）
    pub self_version: String,
    /// 下载地址列表（源：`List<string> Urls`，默认空列表）
    pub urls: Vec<String>,
    /// Tweak 类名（源：`TweakClass`，默认 string.Empty；构造处已应用 ?? 默认值，恒非空）
    pub tweak_class: String,
    /// 库列表（源：`List<Library> Libraries`，默认空列表）
    pub libraries: Vec<Library>,
    /// 基础版本 JSON 原文（源：`string? Json`）
    pub json: Option<String>,
}

/// 解析 Maven 坐标（源：ParseMavenCoordinate，private static）。
///
/// 空白 / 少于 3 段 / 任一段 trim 后为空 → None；仅取前 3 段（group:artifact:version），
/// 多余段忽略（源行为）。
fn parse_maven_coordinate(maven: &str) -> Option<Artifact> {
    if maven.trim().is_empty() {
        return None;
    }
    let parts: Vec<&str> = maven.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let group = parts[0].trim();
    let artifact = parts[1].trim();
    let version = parts[2].trim();
    if group.is_empty() || artifact.is_empty() || version.is_empty() {
        return None;
    }
    Some(Artifact {
        group_id: group.to_string(),
        artifact_id: artifact.to_string(),
        version: version.to_string(),
    })
}

/// 库条目 → 版本 JSON 的 libraries 数组元素（源：BuildVersionJson 内 Select lambda）。
///
/// name = Artifact.ToString()（Artifact 为 null → null）；url 非空时写入；
/// DownloadInfo 非 null 时写入 downloads.artifact{path, url}（path/url 可 null，源
/// `new JsonObject { ["path"] = ... }` 对 null 属性同样写入 null 值）。
fn library_to_json(lib: &Library) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "name".to_string(),
        lib.artifact
            .as_ref()
            .map(|a| Value::String(a.to_string()))
            .unwrap_or(Value::Null),
    );
    // 源：`if (!string.IsNullOrEmpty(lib.Url)) obj["url"] = lib.Url;`
    if let Some(url) = lib.url.as_deref().filter(|u| !u.is_empty()) {
        obj.insert("url".to_string(), Value::String(url.to_string()));
    }
    // 源：`if (lib.DownloadInfo != null) { obj["downloads"] = ... }`
    if let Some(download_info) = &lib.download_info {
        let mut artifact_obj = serde_json::Map::new();
        artifact_obj.insert(
            "path".to_string(),
            download_info
                .path
                .as_ref()
                .map(|p| Value::String(p.clone()))
                .unwrap_or(Value::Null),
        );
        artifact_obj.insert(
            "url".to_string(),
            download_info
                .url
                .as_ref()
                .map(|u| Value::String(u.clone()))
                .unwrap_or(Value::Null),
        );
        obj.insert("downloads".to_string(), serde_json::json!({ "artifact": Value::Object(artifact_obj) }));
    }
    Value::Object(obj)
}

/// 模拟 C# `Path.Combine(a, b)` 的拼接语义（源在 InstallLiteLoaderCoreAsync /
/// GetBaseMcVersion / SaveVersionJson 中使用）：a 以分隔符（/ 或 \）结尾 → 直接拼接；
/// 否则插入平台主分隔符（Windows: \，其余: /）。C# Combine 仅字符串拼接 + 分隔符判断，
/// 不规范化路径。
fn path_combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if a.ends_with(['/', '\\']) {
        format!("{a}{b}")
    } else {
        format!("{a}{}{b}", std::path::MAIN_SEPARATOR)
    }
}

