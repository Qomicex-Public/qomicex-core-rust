//! Babric 安装器（B9）
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/BabricInstaller.cs（158 行）
//!
//! 流程要点（逐字保留源逻辑，含 Trace 日志 → eprintln!）：
//! - Meta API 固定为：`https://meta.babric.glass-launcher.net/v2/versions/loader/{gameVersion}/{babricVersion}/profile/json`
//!   （构造参数 downloadSource 被源忽略）；
//! - 安装流程：拉取 loader profile JSON → 遍历 libraries，本地已存在则跳过计数，否则下载
//!   （下载 URL = urlDomain + MavenToPath，库自身的 "url" 字段非空时作为 urlDomain 覆盖）→
//!   meta["id"] = versionId → 与主版本 JSON 合并（MergeVersionJson，去 inheritsFrom）→
//!   写 {gameDir}/versions/{versionId}/{versionId}.json；
//! - 缺失库检查：存在且 SHA1 匹配 → 跳过（记日志）；否则计入 MissFileData。
//!
//! ⚠️ UNMAPPED：契约模块 `crate::services::installers::installer`（Installer trait +
//! InstallerBase 静态工具）由 P35 并行写入，本文件编写时尚未存在；所有引用为假定签名，
//! 详见翻译日志 p37。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::services::installers::installer::MissFileData;
use crate::services::installers::installer::{Installer, InstallerBase};
use crate::services::installers::fabric::install::verify_file_sha1;
use crate::util::lib_helper::maven_to_path;
use crate::util::platform::{get_current_arch, get_current_os_name};

/// Babric Meta API 基地址（源 BuildJson/GetMissBabricLibraries 内的固定字面量）
const BABRIC_META_BASE: &str = "https://meta.babric.glass-launcher.net/";

/// Babric 安装器（源：`internal class BabricInstaller : InstallerBase, IInstaller`）。
pub(crate) struct BabricInstaller {
    /// 源 `_gameDir`：Minecraft 根目录
    game_dir: PathBuf,
}

impl BabricInstaller {
    /// 对应源构造器 `BabricInstaller(int downloadSource, string gameDir)`。
    /// 源语义：downloadSource 参数完全被忽略（Meta 地址为固定字面量），
    /// 按映射表 `downloadSource int → DownloadMirror` 收参。
    pub(crate) fn new(_download_source: DownloadMirror, game_dir: impl Into<PathBuf>) -> Self {
        Self {
            game_dir: game_dir.into(),
        }
    }

    /// 对应源 `InstallBabricAsync`：
    /// 构建 JSON → 创建版本目录 → 与主版本 JSON 合并 → 写入 `{gameDir}/versions/{versionId}/{versionId}.json`。
    pub(crate) async fn install_babric_async(
        &self,
        version_id: &str,
        babric_version: &str,
        game_version: &str,
        inherits_from_json: Option<&str>,
    ) -> Result<bool, Error> {
        // 源：Trace.WriteLine($"Babric 安装开始: versionId={versionId}, babricVersion={babricVersion}, gameVersion={gameVersion}")
        eprintln!("Babric 安装开始: versionId={version_id}, babricVersion={babric_version}, gameVersion={game_version}");

        // 源：var jsonData = await BuildJson(...); if (string.IsNullOrEmpty(jsonData)) throw new Exception("构建JSON数据失败");
        let json_data = self
            .build_json(version_id, babric_version, game_version, &self.game_dir)
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

        // 源：if (!string.IsNullOrEmpty(inheritsFromJson)) { Trace.WriteLine("合并版本 JSON..."); jsonData = MergeVersionJson(...); } else throw new Exception("主版本JSON文件不存在");
        let json_data = match inherits_from_json {
            Some(main_json) if !main_json.is_empty() => {
                eprintln!("合并版本 JSON...");
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

        // 源：var jsonPath = Path.Combine(_gameDir, "versions", versionId, $"{versionId}.json");
        //      await File.WriteAllTextAsync(jsonPath, jsonData);
        let json_path = version_dir.join(format!("{version_id}.json"));
        tokio::fs::write(&json_path, json_data)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("写入版本 JSON 失败: {}", json_path.display()),
                source: Some(Box::new(e)),
            })?;
        eprintln!("版本 JSON 已写入: {}", json_path.display());
        eprintln!("Babric 安装完成: {version_id}");
        Ok(true)
    }

    /// 对应源私有 `BuildJson`：拉取 Babric loader profile JSON，逐库下载（已存在跳过），置 `id` 后返回。
    async fn build_json(
        &self,
        version_id: &str,
        babric_version: &str,
        game_version: &str,
        game_dir: &Path,
    ) -> Result<String, Error> {
        // 源：using var client = CreateHttpClient();
        let client = InstallerBase::create_http_client();
        let url = format!(
            "{}v2/versions/loader/{game_version}/{babric_version}/profile/json",
            BABRIC_META_BASE
        );
        eprintln!("获取 Babric Meta: {url}");

        // 源：if (!result.IsSuccessStatusCode) { Trace.WriteLine($"Babric Meta 请求失败: {result.StatusCode}"); throw new Exception("获取Launcher Meta失败"); }
        let response = client.get(&url).send().await.map_err(|e| Error::Http {
            message: format!("GET {url} 失败"),
            source: Some(Box::new(e)),
        })?;
        if !response.status().is_success() {
            eprintln!("Babric Meta 请求失败: {}", response.status());
            return Err(Error::Http {
                message: "获取Launcher Meta失败".to_string(),
                source: None,
            });
        }

        let meta_str = response
            .text()
            .await
            .map_err(|e| Error::Http {
                message: format!("读取响应体失败: {url}"),
                source: Some(Box::new(e)),
            })?;
        let mut meta: Value =
            serde_json::from_str(&meta_str).map_err(|e| Error::Http {
                message: "解析 Launcher Meta JSON 失败".to_string(),
                source: Some(Box::new(e)),
            })?;
        eprintln!("Babric Meta 获取成功");

        // 源：var libs = meta["libraries"] as JsonArray; if (libs != null) { Trace.WriteLine($"处理 {libs.Count} 个库文件..."); ... }
        {
            let libs = meta.get("libraries").and_then(|v| v.as_array());
            if let Some(libs) = libs {
                eprintln!("处理 {} 个库文件...", libs.len());
                let mut download_count = 0;
                let mut skip_count = 0;
                for lib in libs {
                    download_count += 1;
                    // 源：var libName = lib!["name"]?.ToString() ?? "未知";
                    let lib_name = lib
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("未知");
                    // 源：urlDomain 默认固定 Babric 基地址，库自带非空 "url" 时覆盖
                    let mut url_domain = BABRIC_META_BASE.to_string();
                    if let Some(url) = lib.get("url").and_then(|v| v.as_str()) {
                        if !url.is_empty() {
                            url_domain = url.to_string();
                        }
                    }

                    let maven_name = get_maven_name_with_classifier(lib);
                    let maven_path = maven_to_path(&maven_name);
                    // 源：var destPath = $"{gameDir}/libraries/{MavenToPath(mavenName)}";
                    let dest_path = game_dir.join("libraries").join(&maven_path);
                    if dest_path.is_file() {
                        skip_count += 1;
                        eprintln!("[{download_count}/{}] 已存在: {lib_name}", libs.len());
                        continue;
                    }
                    eprintln!("[{download_count}/{}] 下载库: {lib_name}", libs.len());
                    // 源：await DownloadFileAsync(client, $"{urlDomain}{MavenToPath(mavenName)}", destPath);
                    InstallerBase::download_file_async(
                        &client,
                        &format!("{url_domain}{maven_path}"),
                        dest_path.to_str().unwrap_or_default(),
                        50,
                    )
                    .await?;
                    eprintln!("[{download_count}/{}] 完成: {lib_name}", libs.len());
                }
                // 源：if (skipCount > 0) Trace.WriteLine($"跳过 {skipCount} 个已存在文件, 下载 {libs.Count - skipCount} 个");
                if skip_count > 0 {
                    eprintln!(
                        "跳过 {} 个已存在文件, 下载 {} 个",
                        skip_count,
                        libs.len() - skip_count
                    );
                }
                eprintln!("所有库文件处理完成");
            } else {
                // 源：else Trace.WriteLine("无库文件需要下载");
                eprintln!("无库文件需要下载");
            }
        }

        // 源：meta["id"] = versionId; return meta.ToJsonString();
        meta["id"] = json!(version_id);
        Ok(meta.to_string())
    }

    /// 对应源 `GetMissBabricLibraries`：检查缺失库（存在且 SHA1 匹配则跳过，含逐库日志）。
    pub(crate) async fn get_miss_babric_libraries(
        &self,
        babric_version: &str,
        game_version: &str,
        game_dir: &str,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：Trace.WriteLine($"Babric 缺失库检查: babricVersion={babricVersion}, gameVersion={gameVersion}");
        eprintln!(
            "Babric 缺失库检查: babricVersion={babric_version}, gameVersion={game_version}"
        );
        // 源：using var client = CreateHttpClient();
        let client = InstallerBase::create_http_client();
        let url = format!(
            "{}v2/versions/loader/{game_version}/{babric_version}/profile/json",
            BABRIC_META_BASE
        );

        let response = client.get(&url).send().await.map_err(|e| Error::Http {
            message: format!("GET {url} 失败"),
            source: Some(Box::new(e)),
        })?;
        if !response.status().is_success() {
            // 源：Trace.WriteLine($"Babric Meta 请求失败: {result.StatusCode}")
            eprintln!("Babric Meta 请求失败: {}", response.status());
            return Err(Error::Http {
                message: "获取Launcher Meta失败".to_string(),
                source: None,
            });
        }

        let meta_str = response
            .text()
            .await
            .map_err(|e| Error::Http {
                message: format!("读取响应体失败: {url}"),
                source: Some(Box::new(e)),
            })?;
        let meta: Value = serde_json::from_str(&meta_str).map_err(|e| Error::Http {
            message: "解析 Launcher Meta JSON 失败".to_string(),
            source: Some(Box::new(e)),
        })?;

        let mut miss_files: Vec<MissFileData> = Vec::new();
        {
            // 源：if (libs != null) { Trace.WriteLine($"检查 {libs.Count} 个库文件..."); }
            let libs = meta.get("libraries").and_then(|v| v.as_array());
            if let Some(libs) = libs {
                eprintln!("检查 {} 个库文件...", libs.len());
                for lib in libs {
                    let mut url_domain = BABRIC_META_BASE.to_string();
                    if let Some(url) = lib.get("url").and_then(|v| v.as_str()) {
                        if !url.is_empty() {
                            url_domain = url.to_string();
                        }
                    }

                    let maven_name = get_maven_name_with_classifier(lib);
                    let maven_path = maven_to_path(&maven_name);
                    // 源：var libPath = Path.Combine(gameDir, "libraries", MavenToPath(mavenName));
                    let lib_path = Path::new(game_dir).join("libraries").join(&maven_path);

                    // 源：if (File.Exists(libPath)) { if (!string.IsNullOrEmpty(...) && VerifyFileSha1(...)) { Trace.WriteLine("  已存在 (SHA1匹配): ..."); continue; } else Trace.WriteLine("  SHA1不匹配，需重下: ..."); } else Trace.WriteLine($"  缺失: ...");
                    if lib_path.is_file() {
                        let sha1 = lib
                            .get("sha1")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if !sha1.is_empty() && verify_file_sha1(lib_path.to_str().unwrap_or_default(), sha1) {
                            eprintln!("  已存在 (SHA1匹配): {:?}", lib.get("name"));
                            continue;
                        } else {
                            eprintln!("  SHA1不匹配，需重下: {:?}", lib.get("name"));
                        }
                    } else {
                        eprintln!("  缺失: {:?}", lib.get("name"));
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
        // 源：Trace.WriteLine($"Babric 缺失库检查完成: {missFiles.Count} 个缺失");
        eprintln!("Babric 缺失库检查完成: {} 个缺失", miss_files.len());
        Ok(miss_files)
    }
}

/// 对应源私有静态 `GetMavenNameWithClassifier`（Babric 版含 Trace 日志）：
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
                // 源：Trace.WriteLine($"检测到原生库: {name}, 分类器: {classifier}")
                eprintln!("检测到原生库: {name}, 分类器: {classifier}");
                return format!("{name}:{classifier}");
            }
        }
    }
    name.to_string()
}

#[async_trait]
impl Installer for BabricInstaller {
    /// 对应源 `InstallAsync`：para1=babricVersion、para2=gameVersion，任一为 null → ArgumentNullException 语义。
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
    _para3: Option<&str>,
    _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：if (babricVersion == null) throw new ArgumentNullException(nameof(babricVersion));
        let Some(babric_version) = para1 else {
            return Err(Error::Params {
                message: "babricVersion 为 null（源 ArgumentNullException 语义）".to_string(),
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
        // 源：await InstallBabricAsync(versionId, babricVersion, gameVersion, inheritsFromJson);
        self.install_babric_async(version_id, babric_version, game_version, Some(inherits_from_json))
            .await?;
        Ok(())
    }

    /// 对应源 `GetMissLibrariesAsync`：任一参数为 null → 返回空列表，否则委托 GetMissBabricLibraries。
    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        match (para1, para2, para3) {
            (Some(babric_version), Some(game_version), Some(game_dir)) => {
                self.get_miss_babric_libraries(babric_version, game_version, game_dir)
                    .await
            }
            _ => Ok(Vec::new()),
        }
    }
}









