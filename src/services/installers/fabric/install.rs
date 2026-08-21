//! Fabric 安装器（B9）
//! 对应源：Services/Installers/FabricInstaller.cs（131 行）
//!
//! ⚠️ 协同契约（P35 并行写入 src/services/installers/installer.rs，本文件按其签名引用，以实际为准）：
//! 1. `crate::services::installers::installer::Installer` trait（#[async_trait]，源 IInstaller）：
//!    - `install(&self, version_id: &str, inherits_from_json: &str, para1: Option<&str>,
//!      para2: Option<&str>, para3: Option<&str>, para4: Option<&str>) -> Result<(), Error>`
//!      （源 `Task InstallAsync(string versionId, string inheritsFromJson, string? para1..para4)`）
//!    - `get_miss_libraries(&self, para1: Option<&str>, para2: Option<&str>,
//!      para3: Option<&str>) -> Result<Vec<MissFileData>, Error>`
//!      （源 `Task<List<MissFileData>> GetMissLibrariesAsync(string? para1..para3)`；
//!      MissFileData record → models::installer::MissFileData，字段同名映射）
//! 2. `crate::services::installers::installer::InstallerBase` 静态工具（pub(crate)）：
//!    - `InstallerBase::create_http_client() -> reqwest::Client`（源 CreateHttpClient，可选 DefaultUserAgent）
//!    - `InstallerBase::merge_version_json(main_version_json: &str, merged_version_json: &str,
//!      default_version_id: Option<&str>) -> Result<String, Error>`
//!      （源 MergeVersionJson：合并 + 移除 inheritsFrom + 回写 id）
//!    - `InstallerBase::download_file_async(client: &reqwest::Client, url: &str, destination_path: &str,
//!      max_redirects: u32) -> Result<bool, Error>`（源 DownloadFileAsync，默认 5 次重定向）
//! 3. MavenToPath 复用 util::lib_helper::maven_to_path（MAPPING_TABLE utils 既定映射）。
//!    ⚠️ 微差：源 InstallerBase.MavenToPath 对 group/artifact 不截断 '@'，lib_helper 版截断；
//!    实际 Fabric 库坐标不含 '@'，无行为差异（见翻译日志 p36）。
//!
//! 错误语义（同 locator_miss.rs 定案）：
//! - 传输层（请求/状态码/响应读取/JSON 解析）→ Error::Http（error.rs 注：Http 变体含
//!   源 JsonException 语义）
//! - 文件 IO / 下载 → Error::DownloadFailed
//! - 参数缺失（源 ArgumentNullException）/ 主版本 JSON 缺失 / 库条目畸形（源 NRE）→ Error::Params

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::services::download::checksum::sha1_hex;
use crate::services::installers::installer::MissFileData;
use crate::services::installers::installer::{Installer, InstallerBase};
use crate::util::lib_helper::maven_to_path;

/// Fabric 安装器（源：`internal class FabricInstaller : InstallerBase, IInstaller`）
pub(crate) struct FabricInstaller {
    /// 下载源根地址（源 `_downloadSource`）：int 1 → BMCLAPI 镜像
    /// （`https://bmclapi2.bangbang93.com/fabric-meta/`），其余 → 官方
    /// （`https://meta.fabricmc.net/`）。镜像语义映射 DownloadMirror（1 → Bmclapi，0 → Official）。
    download_source: String,
    /// 游戏根目录（源 `_gameDir`，readonly）
    game_dir: String,
}

impl FabricInstaller {
    /// 创建 Fabric 安装器（源：`FabricInstaller(int downloadSource, string gameDir)`）
    pub(crate) fn new(download_source: i32, game_dir: String) -> Self {
        // 源：`downloadSource == 1 ? BMCLAPI : 官方`（其余值一律按官方处理）
        let mirror = if download_source == 1 {
            DownloadMirror::Bmclapi
        } else {
            DownloadMirror::Official
        };
        // 镜像选择日志：int → DownloadMirror 映射（源无日志，移植约定补充）
        eprintln!("[FabricInstaller] 镜像选择: {mirror:?} (downloadSource={download_source})");
        let download_source = if download_source == 1 {
            "https://bmclapi2.bangbang93.com/fabric-meta/".to_string()
        } else {
            "https://meta.fabricmc.net/".to_string()
        };
        Self {
            download_source,
            game_dir,
        }
    }

    /// 安装 Fabric 到版本目录（源：InstallFabricAsync）
    pub(crate) async fn install_fabric(
        &self,
        version_id: &str,
        fabric_version: &str,
        game_version: &str,
        inherits_from_json: &str,
    ) -> Result<bool, Error> {
        let json_data = self
            .build_json(version_id, fabric_version, game_version, &self.game_dir)
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

        // 源：inheritsFromJson 为空 → 抛"主版本JSON文件不存在"；否则合并（合并失败经
        // merge_version_json 返回错误，源 MergeJson catch 返回空串后 Parse("") 抛异常，同传播语义）
        let json_data = if inherits_from_json.is_empty() {
            return Err(Error::Params {
                message: "主版本JSON文件不存在".to_string(),
                source: None,
            });
        } else {
            let merged =
                InstallerBase::merge_version_json(inherits_from_json, &json_data, Some(version_id));
            if merged.is_empty() {
                return Err(Error::DownloadFailed {
                    message: "版本JSON合并失败".to_string(),
                    source: None,
                });
            }
            merged
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

    /// 构建 Fabric 版本 JSON 并下载加载器库（源：BuildJson，private）
    async fn build_json(
        &self,
        version_id: &str,
        fabric_version: &str,
        game_version: &str,
        game_dir: &str,
    ) -> Result<String, Error> {
        let client = InstallerBase::create_http_client();
        // 源：`$"{_downloadSource}v2/versions/loader/{gameVersion}/{fabricVersion}/profile/json"`
        let url = format!(
            "{}v2/versions/loader/{game_version}/{fabric_version}/profile/json",
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
        // 源：`JsonNode.Parse(metaStr)!.AsObject()`（Parse 失败 → JsonException；非对象 → InvalidOperationException）
        let meta_value: Value = serde_json::from_str(&meta_str).map_err(|e| Error::Http {
            message: format!("Launcher Meta JSON 解析失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let mut meta = meta_value
            .as_object()
            .ok_or_else(|| Error::Http {
                message: "Launcher Meta 顶层非 JSON 对象".to_string(),
                status: None,
                source: None,
            })?
            .clone();

        // 源：`meta["libraries"] as JsonArray`（null/非数组 → 跳过下载）
        if let Some(libs) = meta.get("libraries").and_then(|v| v.as_array()) {
            for lib in libs {
                // 源：`lib["name"]?.ToString()!`（name 缺失 → NRE，此处等价报错）
                let name =
                    lib.get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| Error::Params {
                            message: "库条目缺少 name（源 NullReferenceException）".to_string(),
                            source: None,
                        })?;
                // 源：`if (!string.IsNullOrEmpty(lib["url"]?.ToString())) urlDomain = ...`
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

    /// 获取缺失的 Fabric 库文件列表（源：GetMissFabricLibraries）
    pub(crate) async fn get_miss_fabric_libraries(
        &self,
        fabric_version: &str,
        game_version: &str,
        game_dir: &str,
    ) -> Result<Vec<MissFileData>, Error> {
        let client = InstallerBase::create_http_client();
        let url = format!(
            "{}v2/versions/loader/{game_version}/{fabric_version}/profile/json",
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
                let name =
                    lib.get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| Error::Params {
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
                    // 源：sha1 非空且校验通过 → continue（跳过该库）
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
                // 源：`var url = Path.Combine(urlDomain, MavenToPath(name));`
                // （urlDomain 以 / 结尾时 Combine 等价直接拼接；库 url 通常含尾斜杠）
                let mut url = path_combine(url_domain, &maven_path);
                // 源：非官方源时把官方域名替换为 BMCLAPI 域名（String.Replace 全局替换，原样保留）
                if self.download_source != "https://meta.fabricmc.net/" {
                    url = url
                        .replace(
                            "https://meta.fabricmc.net/",
                            "https://bmclapi2.bangbang93.com/fabric-meta",
                        )
                        .replace(
                            "https://maven.fabricmc.net/",
                            "https://bmclapi2.bangbang93.com/maven",
                        );
                }
                miss_files.push(MissFileData {
                    name: name.to_string(),
                    url,
                    sha1: lib
                        .get("sha1")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    path: lib_path,
                });
            }
        }
        Ok(miss_files)
    }
}

/// 源 InstallAsync 的 trait 实现（para1 = fabricVersion，para2 = gameVersion，
/// para3/para4 未使用）。签名按 P35 契约推断，installer.rs 交付后按实际校对。
#[async_trait]
impl Installer for FabricInstaller {
    async fn install(
        &self,
        version_id: &str,
        inherits_from_json: &str,
        para1: Option<&str>,
        para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：`if (fabricVersion == null) throw new ArgumentNullException(nameof(fabricVersion));`
        let fabric_version = para1.ok_or_else(|| Error::Params {
            message: "fabricVersion 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        let game_version = para2.ok_or_else(|| Error::Params {
            message: "gameVersion 为 null（源 ArgumentNullException）".to_string(),
            source: None,
        })?;
        self.install_fabric(version_id, fabric_version, game_version, inherits_from_json)
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
        let (Some(fabric_version), Some(game_version), Some(game_dir)) = (para1, para2, para3)
        else {
            return Ok(Vec::new());
        };
        self.get_miss_fabric_libraries(fabric_version, game_version, game_dir)
            .await
    }
}

/// 校验文件 SHA1（源：`FabricInstaller.VerifyFileSha1`，internal static；
/// QuiltInstaller 亦跨类调用本函数，故保持 pub(crate)）。
pub(crate) fn verify_file_sha1(file_path: &str, expected_hash: &str) -> bool {
    // 源：`if (!File.Exists(filePath)) return false;`
    if !Path::new(file_path).is_file() {
        return false;
    }
    let bytes = match std::fs::read(file_path) {
        Ok(b) => b,
        Err(e) => {
            // 源 File.OpenRead 失败抛异常（调用方已 File.Exists 前置检查，实际不可达）；
            // 遵循 util::file_helper::validate_file_hash 先例：IO 失败视为校验失败（记录日志）
            eprintln!("SHA1 校验读取文件失败：{e}");
            return false;
        }
    };
    // 源：`BitConverter.ToString(hash).Replace("-", "").ToLower()` → 小写十六进制；
    // 两端 Trim 后 `OrdinalIgnoreCase` 比较
    sha1_hex(&bytes)
        .trim()
        .eq_ignore_ascii_case(expected_hash.trim())
}

/// 模拟 C# `Path.Combine(a, b)` 的拼接语义（源在 GetMissFabricLibraries 中用于
/// libPath 与 url 拼接）：a 以分隔符（/ 或 \）结尾 → 直接拼接；否则插入平台主分隔符
/// （Windows: \，其余: /）。C# Combine 仅字符串拼接 + 分隔符判断，不规范化路径。
fn path_combine(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if a.ends_with(['/', '\\']) {
        format!("{a}{b}")
    } else {
        format!("{a}{}{b}", std::path::MAIN_SEPARATOR)
    }
}
