//! Quilt 安装器（B9）
//! 对应源：Services/Installers/QuiltInstaller.cs（115 行）
//!
//! ⚠️ 协同契约（P35 并行写入 src/services/installers/installer.rs，本文件按其签名引用，以实际为准）：
//! 1. `crate::services::installers::installer::Installer` trait（#[async_trait]，源 IInstaller）：
//!    - `install(&self, version_id: &str, inherits_from_json: &str, para1: Option<&str>,
//!      para2: Option<&str>, para3: Option<&str>, para4: Option<&str>) -> Result<(), Error>`
//!    - `get_miss_libraries(&self, para1: Option<&str>, para2: Option<&str>,
//!      para3: Option<&str>) -> Result<Vec<MissFileData>, Error>`
//! 2. `InstallerBase` 静态工具（pub(crate)）：create_http_client / merge_version_json /
//!    download_file_async（签名同 fabric/install.rs 契约说明）。
//! 3. 源 QuiltInstaller 跨类调用 `FabricInstaller.VerifyFileSha1` → 复用
//!    `crate::services::installers::fabric::install::verify_file_sha1`。
//!
//! 与 Fabric 源的差异保留（逐字移植）：
//! - meta 端点 v3（Fabric 为 v2）；构造默认源 `https://meta.quiltmc.org/`，
//!   BMCLAPI 镜像 `https://bmclapi2.bangbang93.com/quilt-meta/`
//! - GetMissQuiltLibraries 的 url 用字符串插值 `$"{urlDomain}{MavenToPath(name)}"`
//!   （Fabric 用 Path.Combine），path 字段用 `$"{gameDir}/libraries/{MavenToPath(name)}"`，
//!   且无官方域名替换逻辑
//!
//! 错误语义（同 fabric/install.rs）：传输层/JSON 解析 → Error::Http；
//! 文件 IO/下载 → Error::DownloadFailed；参数缺失（源 ArgumentNullException）→ Error::Params

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::services::installers::installer::MissFileData;
use crate::services::installers::fabric::install::verify_file_sha1;
use crate::services::installers::installer::{Installer, InstallerBase};
use crate::util::lib_helper::maven_to_path;

/// Quilt 安装器（源：`internal class QuiltInstaller : InstallerBase, IInstaller`）
pub(crate) struct QuiltInstaller {
    /// 下载源根地址（源 `_downloadSource`）：int 1 → BMCLAPI 镜像
    /// （`https://bmclapi2.bangbang93.com/quilt-meta/`），其余 → 官方
    /// （`https://meta.quiltmc.org/`）。镜像语义映射 DownloadMirror（1 → Bmclapi，0 → Official）。
    download_source: String,
    /// 游戏根目录（源 `_gameDir`，readonly）
    game_dir: String,
}

impl QuiltInstaller {
    /// 创建 Quilt 安装器（源：`QuiltInstaller(int downloadSource, string gameDir)`）
    pub(crate) fn new(download_source: i32, game_dir: String) -> Self {
        // 源：`downloadSource == 1 ? BMCLAPI : 官方`（其余值一律按官方处理）
        let mirror = if download_source == 1 {
            DownloadMirror::Bmclapi
        } else {
            DownloadMirror::Official
        };
        // 镜像选择日志：int → DownloadMirror 映射（源无日志，移植约定补充）
        eprintln!("[QuiltInstaller] 镜像选择: {mirror:?} (downloadSource={download_source})");
        let download_source = if download_source == 1 {
            "https://bmclapi2.bangbang93.com/quilt-meta/".to_string()
        } else {
            "https://meta.quiltmc.org/".to_string()
        };
        Self {
            download_source,
            game_dir,
        }
    }

    /// 安装 Quilt 到版本目录（源：InstallQuiltAsync）
    pub(crate) async fn install_quilt(
        &self,
        version_id: &str,
        quilt_version: &str,
        game_version: &str,
        inherits_from_json: &str,
    ) -> Result<bool, Error> {
        let json_data = self
            .build_json(version_id, quilt_version, game_version, &self.game_dir)
            .await?;
        if json_data.is_empty() {
            return Err(Error::DownloadFailed {
                message: "构建JSON数据失败".to_string(),
                source: None,
            });
        }

        // 源：`var versionDir = $"{_gameDir}/versions/{versionId}"; if (!Directory.Exists) Create`
        let version_dir = path_combine(&path_combine(&self.game_dir, "versions"), version_id);
        if !Path::new(&version_dir).is_dir() {
            std::fs::create_dir_all(&version_dir).map_err(|e| Error::DownloadFailed {
                message: format!("创建版本目录失败: {version_dir}"),
                source: Some(Box::new(e)),
            })?;
        }

        // 源：inheritsFromJson 为空 → 抛"主版本JSON文件不存在"；否则合并
        let json_data = if inherits_from_json.is_empty() {
            return Err(Error::Params {
                message: "主版本JSON文件不存在".to_string(),
                source: None,
            });
        } else {
            InstallerBase::merge_version_json(inherits_from_json, &json_data, Some(version_id))
        };

        // 源：`File.WriteAllTextAsync(Path.Combine(_gameDir, "versions", versionId, $"{versionId}.json"))`
        let dest = path_combine(
            &path_combine(&path_combine(&self.game_dir, "versions"), version_id),
            &format!("{version_id}.json"),
        );
        std::fs::write(&dest, &json_data).map_err(|e| Error::DownloadFailed {
            message: format!("写入版本JSON失败: {dest}"),
            source: Some(Box::new(e)),
        })?;
        Ok(true)
    }

    /// 构建 Quilt 版本 JSON 并下载加载器库（源：BuildJson，private）
    async fn build_json(
        &self,
        version_id: &str,
        quilt_version: &str,
        game_version: &str,
        game_dir: &str,
    ) -> Result<String, Error> {
        let client = InstallerBase::create_http_client();
        // 源：`$"{_downloadSource}v3/versions/loader/{gameVersion}/{quiltVersion}/profile/json"`
        let url = format!(
            "{}v3/versions/loader/{game_version}/{quilt_version}/profile/json",
            self.download_source
        );
        let response = client.get(&url).send().await.map_err(|e| Error::Http {
            message: format!("获取Launcher Meta失败: {e}"),
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
            message: format!("读取Launcher Meta响应失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let meta_value: Value = serde_json::from_str(&meta_str).map_err(|e| Error::Http {
            message: format!("Launcher Meta JSON 解析失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let mut meta = meta_value.as_object().ok_or_else(|| Error::Http {
            message: "Launcher Meta 顶层非 JSON 对象".to_string(),
            status: None,
            source: None,
        })?.clone();

        // 源：`meta["libraries"] as JsonArray`（null/非数组 → 跳过下载）
        if let Some(libs) = meta.get("libraries").and_then(|v| v.as_array()) {
            for lib in libs {
                let name = lib.get("name").and_then(|v| v.as_str()).ok_or_else(|| Error::Params {
                    message: "库条目缺少 name（源 NullReferenceException）".to_string(),
                    source: None,
                })?;
                let url_domain = lib
                    .get("url")
                    .and_then(|v| v.as_str())
                    .filter(|u| !u.is_empty())
                    .unwrap_or(&self.download_source);
                let maven_path = maven_to_path(name);
                // 源：`await DownloadFileAsync(client, $"{urlDomain}{MavenToPath(name)}",
                //      $"{gameDir}/libraries/{MavenToPath(name)}")`（返回值被源忽略；失败抛异常传播）
                InstallerBase::download_file_async(
                    &client,
                    &format!("{url_domain}{maven_path}"),
                    &format!("{game_dir}/libraries/{maven_path}"),
                    5,
                )
                .await?;
            }
        }

        // 源：`meta["id"] = versionId`
        meta.insert("id".to_string(), Value::String(version_id.to_string()));
        serde_json::to_string(&Value::Object(meta)).map_err(|e| Error::Http {
            message: format!("序列化版本 JSON 失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 获取缺失的 Quilt 库文件列表（源：GetMissQuiltLibraries）
    pub(crate) async fn get_miss_quilt_libraries(
        &self,
        quilt_version: &str,
        game_version: &str,
        game_dir: &str,
    ) -> Result<Vec<MissFileData>, Error> {
        let client = InstallerBase::create_http_client();
        let url = format!(
            "{}v3/versions/loader/{game_version}/{quilt_version}/profile/json",
            self.download_source
        );
        let response = client.get(&url).send().await.map_err(|e| Error::Http {
            message: format!("获取Launcher Meta失败: {e}"),
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
            message: format!("读取Launcher Meta响应失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let meta_value: Value = serde_json::from_str(&meta_str).map_err(|e| Error::Http {
            message: format!("Launcher Meta JSON 解析失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let meta = meta_value.as_object().ok_or_else(|| Error::Http {
            message: "Launcher Meta 顶层非 JSON 对象".to_string(),
            status: None,
            source: None,
        })?;

        let mut miss_files = Vec::new();
        if let Some(libs) = meta.get("libraries").and_then(|v| v.as_array()) {
            for lib in libs {
                let name = lib.get("name").and_then(|v| v.as_str()).ok_or_else(|| Error::Params {
                    message: "库条目缺少 name（源 NullReferenceException）".to_string(),
                    source: None,
                })?;
                let url_domain = lib
                    .get("url")
                    .and_then(|v| v.as_str())
                    .filter(|u| !u.is_empty())
                    .unwrap_or(&self.download_source);
                let maven_path = maven_to_path(name);
                // 源：`var libPath = Path.Combine(gameDir, "libraries", MavenToPath(name));`
                let lib_path = path_combine(&path_combine(game_dir, "libraries"), &maven_path);
                if Path::new(&lib_path).is_file() {
                    // 源：sha1 非空且校验通过 → continue（复用 FabricInstaller.VerifyFileSha1）
                    if let Some(sha1) = lib
                        .get("sha1")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        if verify_file_sha1(&lib_path, sha1) {
                            continue;
                        }
                    }
                }
                // 源（与 Fabric 差异保留）：url 与 path 均为字符串插值（非 Path.Combine），
                // 无官方域名替换逻辑
                miss_files.push(MissFileData {
                    name: name.to_string(),
                    url: format!("{url_domain}{maven_path}"),
                    sha1: lib
                        .get("sha1")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    path: format!("{game_dir}/libraries/{maven_path}"),
                });
            }
        }
        Ok(miss_files)
    }
}

/// 源 InstallAsync 的 trait 实现（para1 = quiltVersion，para2 = gameVersion，
/// para3/para4 未使用）。签名按 P35 契约推断，installer.rs 交付后按实际校对。
#[async_trait]
impl Installer for QuiltInstaller {
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
    _para3: Option<&str>,
    _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：`if (quiltVersion == null) throw new ArgumentNullException(nameof(quiltVersion));`
        let quilt_version = para1.ok_or_else(|| Error::Params {
            message: "quiltVersion 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        let game_version = para2.ok_or_else(|| Error::Params {
            message: "gameVersion 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        self.install_quilt(version_id, quilt_version, game_version, inherits_from_json)
            .await?;
        Ok(())
    }

    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        // 源：任一参数为 null → 返回空列表（不报错）
        let (Some(quilt_version), Some(game_version), Some(game_dir)) = (para1, para2, para3)
        else {
            return Ok(Vec::new());
        };
        self.get_miss_quilt_libraries(quilt_version, game_version, game_dir)
            .await
    }
}

/// 模拟 C# `Path.Combine(a, b)` 的拼接语义（源在 GetMissQuiltLibraries 中用于 libPath 拼接）。
/// 细节同 fabric/install.rs 的 path_combine（url 与 path 字段在源中为字符串插值，不经过本函数）。
fn path_combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if a.ends_with(['/', '\\']) {
        format!("{a}{b}")
    } else {
        format!("{a}{}{b}", std::path::MAIN_SEPARATOR)
    }
}











