//! LegacyFabric 安装器（B9）
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/LegacyFabricInstaller.cs（134 行）
//!
//! 流程要点（逐字保留源逻辑）：
//! - Meta API：`https://meta.legacyfabric.net/v2/versions/loader/{gameVersion}/{lfVersion}/profile/json`
//!   （构造参数 downloadSource 被源忽略，_downloadSource 恒为 meta.legacyfabric.net）；
//! - 安装流程：拉取 loader profile JSON → 遍历 libraries 逐一下载
//!   （下载 URL = urlDomain + MavenToPath，库自身的 "url" 字段非空时作为 urlDomain 覆盖）→
//!   meta["id"] = versionId → 与主版本 JSON 合并（MergeVersionJson，去 inheritsFrom）→ 写
//!   {gameDir}/versions/{versionId}/{versionId}.json；
//! - 缺失库检查：SHA1 非空且本地文件 SHA1 匹配则跳过，否则计入 MissFileData。
//!
//! ⚠️ UNMAPPED：契约模块 `crate::services::installers::installer`（Installer trait +
//! InstallerBase 静态工具）由 P35 并行写入，本文件编写时尚未存在；所有引用为假定签名，
//! 详见翻译日志 p37。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::services::installers::fabric::install::verify_file_sha1;
use crate::services::installers::installer::MissFileData;
use crate::services::installers::installer::{Installer, InstallerBase};
use crate::util::lib_helper::maven_to_path;
use crate::util::platform::{get_current_arch, get_current_os_name};

/// 源 `_downloadSource` 固定值：`https://meta.legacyfabric.net/`（构造参数被忽略）
const LEGACY_FABRIC_META_BASE: &str = "https://meta.legacyfabric.net/";

/// LegacyFabric 安装器（源：`internal class LegacyFabricInstaller : InstallerBase, IInstaller`）。
pub(crate) struct LegacyFabricInstaller {
    /// 源 `_downloadSource`：恒为 meta.legacyfabric.net（镜像切换不生效）
    download_source: String,
    /// 源 `_gameDir`：Minecraft 根目录
    game_dir: PathBuf,
}

impl LegacyFabricInstaller {
    /// 对应源构造器 `LegacyFabricInstaller(int downloadSource, string gameDir)`。
    /// 源语义：downloadSource 参数完全被忽略（内部恒用 meta.legacyfabric.net），
    /// 按映射表 `downloadSource int → DownloadMirror` 收参。
    pub(crate) fn new(_download_source: DownloadMirror, game_dir: impl Into<PathBuf>) -> Self {
        Self {
            download_source: LEGACY_FABRIC_META_BASE.to_string(),
            game_dir: game_dir.into(),
        }
    }

    /// 对应源 `InstallLegacyFabricAsync`：
    /// 构建 JSON → 创建版本目录 → 与主版本 JSON 合并 → 写入 `{gameDir}/versions/{versionId}/{versionId}.json`。
    pub(crate) async fn install_legacy_fabric_async(
        &self,
        version_id: &str,
        lf_version: &str,
        game_version: &str,
        inherits_from_json: Option<&str>,
    ) -> Result<bool, Error> {
        // 源：var jsonData = await BuildJson(...); if (string.IsNullOrEmpty(jsonData)) throw new Exception("构建JSON数据失败");
        let json_data = self
            .build_json(version_id, lf_version, game_version, &self.game_dir)
            .await?;
        if json_data.is_empty() {
            // ⚠️ UNMAPPED：源为通用 Exception，此处映射为 Error::DownloadFailed
            return Err(Error::DownloadFailed {
                message: "构建JSON数据失败".to_string(),
                source: None,
            });
        }

        // 源：var versionDir = $"{_gameDir}/versions/{versionId}"; if (!Directory.Exists(versionDir)) Directory.CreateDirectory(versionDir);
        let version_dir = self.game_dir.join("versions").join(version_id);
        std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
            message: format!("创建版本目录失败: {}", version_dir.display()),
            source: Some(Box::new(e)),
        })?;

        // 源：if (!string.IsNullOrEmpty(inheritsFromJson)) jsonData = MergeVersionJson(inheritsFromJson, jsonData, versionId); else throw new Exception("主版本JSON文件不存在");
        let json_data = match inherits_from_json {
            Some(main_json) if !main_json.is_empty() => {
                InstallerBase::merge_version_json(main_json, &json_data, Some(version_id))
            }
            _ => {
                // ⚠️ UNMAPPED：源为通用 Exception，此处映射为 Error::VersionNotFound
                return Err(Error::VersionNotFound {
                    message: "主版本JSON文件不存在".to_string(),
                    source: None,
                });
            }
        };

        // 源：await File.WriteAllTextAsync(Path.Combine(_gameDir, "versions", versionId, $"{versionId}.json"), jsonData);
        let json_path = version_dir.join(format!("{version_id}.json"));
        tokio::fs::write(&json_path, json_data)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("写入版本 JSON 失败: {}", json_path.display()),
                source: Some(Box::new(e)),
            })?;
        Ok(true)
    }

    /// 对应源私有 `BuildJson`：拉取 LegacyFabric loader profile JSON，逐库下载，置 `id` 后返回。
    async fn build_json(
        &self,
        version_id: &str,
        lf_version: &str,
        game_version: &str,
        game_dir: &Path,
    ) -> Result<String, Error> {
        // 源：using var client = CreateHttpClient();
        let client = InstallerBase::create_http_client();
        let url = format!(
            "{}v2/versions/loader/{game_version}/{lf_version}/profile/json",
            self.download_source
        );

        // 源：if (!result.IsSuccessStatusCode) throw new Exception("获取Launcher Meta失败");
        let response = client.get(&url).send().await.map_err(|e| Error::Http {
            message: format!("GET {url} 失败"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        if !response.status().is_success() {
            return Err(Error::Http {
                message: "获取Launcher Meta失败".to_string(),
                status: None,
                source: None,
            });
        }

        let meta_str = response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应体失败: {url}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let mut meta: Value = serde_json::from_str(&meta_str).map_err(|e| Error::Http {
            message: "解析 Launcher Meta JSON 失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;

        // 源：foreach (var lib in libs) —— 逐库下载，不做已存在跳过（与 Babric 不同，源即如此）
        {
            let libs = meta.get("libraries").and_then(|v| v.as_array());
            if let Some(libs) = libs {
                for lib in libs {
                    // 源：urlDomain 默认 _downloadSource，库自带非空 "url" 时覆盖
                    let mut url_domain = self.download_source.clone();
                    if let Some(url) = lib.get("url").and_then(|v| v.as_str()) {
                        if !url.is_empty() {
                            url_domain = url.to_string();
                        }
                    }

                    // 源：var mavenName = GetMavenNameWithClassifier(lib);
                    let maven_name = get_maven_name_with_classifier(lib);
                    let maven_path = maven_to_path(&maven_name);
                    // 源：await DownloadFileAsync(client, $"{urlDomain}{MavenToPath(mavenName)}",
                    //                              $"{gameDir}/libraries/{MavenToPath(mavenName)}");
                    InstallerBase::download_file_async(
                        &client,
                        &format!("{url_domain}{maven_path}"),
                        &game_dir
                            .join("libraries")
                            .join(&maven_path)
                            .to_string_lossy(),
                        50,
                    )
                    .await?;
                }
            }
        }

        // 源：meta["id"] = versionId; return meta.ToJsonString();
        meta["id"] = json!(version_id);
        Ok(meta.to_string())
    }

    /// 对应源 `GetMissLegacyFabricLibraries`：检查缺失库（SHA1 匹配则跳过）。
    pub(crate) async fn get_miss_legacy_fabric_libraries(
        &self,
        lf_version: &str,
        game_version: &str,
        game_dir: &str,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：using var client = CreateHttpClient();
        let client = InstallerBase::create_http_client();
        let url = format!(
            "{}v2/versions/loader/{game_version}/{lf_version}/profile/json",
            self.download_source
        );

        let response = client.get(&url).send().await.map_err(|e| Error::Http {
            message: format!("GET {url} 失败"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        if !response.status().is_success() {
            return Err(Error::Http {
                message: "获取Launcher Meta失败".to_string(),
                status: None,
                source: None,
            });
        }

        let meta_str = response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应体失败: {url}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let meta: Value = serde_json::from_str(&meta_str).map_err(|e| Error::Http {
            message: "解析 Launcher Meta JSON 失败".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;

        let mut miss_files: Vec<MissFileData> = Vec::new();
        {
            let libs = meta.get("libraries").and_then(|v| v.as_array());
            if let Some(libs) = libs {
                for lib in libs {
                    let mut url_domain = self.download_source.clone();
                    if let Some(url) = lib.get("url").and_then(|v| v.as_str()) {
                        if !url.is_empty() {
                            url_domain = url.to_string();
                        }
                    }

                    let maven_name = get_maven_name_with_classifier(lib);
                    let maven_path = maven_to_path(&maven_name);
                    // 源：var libPath = Path.Combine(gameDir, "libraries", MavenToPath(mavenName));
                    let lib_path = Path::new(game_dir).join("libraries").join(&maven_path);

                    // 源：if (File.Exists(libPath)) { if (!string.IsNullOrEmpty(lib["sha1"]?.ToString()) && VerifyFileSha1(...)) continue; }
                    if lib_path.is_file() {
                        let sha1 = lib.get("sha1").and_then(|v| v.as_str()).unwrap_or_default();
                        if !sha1.is_empty()
                            && verify_file_sha1(lib_path.to_str().unwrap_or_default(), sha1)
                        {
                            continue;
                        }
                    }

                    // 源：missFiles.Add(new MissFileData(lib["name"]?.ToString()!, libPath,
                    //            $"{urlDomain}{MavenToPath(mavenName)}", lib["sha1"]?.ToString() ?? ""));
                    // ⚠️ UNMAPPED：源 `lib["name"]?.ToString()!` 为 null-forgiving（键缺失时 C# 侧存 null/NRE），
                    //    此处取空字符串（防御性差异，实际 meta 恒含 name）。
                    miss_files.push(MissFileData {
                        name: lib
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        url: format!("{url_domain}{maven_path}"),
                        sha1: lib
                            .get("sha1")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        path: lib_path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
        Ok(miss_files)
    }
}

/// 对应源私有静态 `GetMavenNameWithClassifier`：
/// 读取 `name`；存在 `natives[当前OS名]` 时拼接 `{name}:{classifier}`（classifier 中 `${arch}` 替换为当前架构）。
fn get_maven_name_with_classifier(lib: &Value) -> String {
    // 源：var name = lib["name"]?.ToString(); if (string.IsNullOrEmpty(name)) return "";
    let Some(name) = lib.get("name").and_then(|v| v.as_str()) else {
        return String::new();
    };
    if name.is_empty() {
        return String::new();
    }

    // 源：var natives = lib["natives"]?.AsObject(); if (natives != null)
    if let Some(natives) = lib.get("natives").and_then(|v| v.as_object()) {
        let os_name = get_current_os_name();
        // 源：if (natives.TryGetPropertyValue(osName, out var classifierNode) && classifierNode != null)
        if let Some(classifier_node) = natives.get(os_name) {
            if !classifier_node.is_null() {
                // 源：classifierNode.ToString().Replace("${arch}", SystemHelper.GetCurrentArch())
                let classifier = classifier_node
                    .as_str()
                    .unwrap_or_default()
                    .replace("${arch}", get_current_arch());
                return format!("{name}:{classifier}");
            }
        }
    }
    name.to_string()
}

#[async_trait]
impl Installer for LegacyFabricInstaller {
    /// 对应源 `InstallAsync`：para1=lfVersion、para2=gameVersion，任一为 null → ArgumentNullException 语义。
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：if (lfVersion == null) throw new ArgumentNullException(nameof(lfVersion));
        let Some(lf_version) = para1 else {
            return Err(Error::Params {
                message: "lfVersion 为 null（源 ArgumentNullException 语义）".to_string(),
                source: None,
            });
        };
        // 源：if (gameVersion == null) throw new ArgumentNullException(nameof(gameVersion));
        let Some(game_version) = para2 else {
            return Err(Error::Params {
                message: "gameVersion 为 null（源 ArgumentNullException 语义）".to_string(),
                source: None,
            });
        };
        // 源：await InstallLegacyFabricAsync(versionId, lfVersion, gameVersion, inheritsFromJson);
        self.install_legacy_fabric_async(
            version_id,
            lf_version,
            game_version,
            Some(inherits_from_json),
        )
        .await?;
        Ok(())
    }

    /// 对应源 `GetMissLibrariesAsync`：任一参数为 null → 返回空列表，否则委托 GetMissLegacyFabricLibraries。
    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        match (para1, para2, para3) {
            (Some(lf_version), Some(game_version), Some(game_dir)) => {
                self.get_miss_legacy_fabric_libraries(lf_version, game_version, game_dir)
                    .await
            }
            _ => Ok(Vec::new()),
        }
    }
}
