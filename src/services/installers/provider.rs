//! InstallerProvider 实现（B13）：模组加载器版本查询（主结构 + Fabric 系）
//!
//! 对应源文件：Qomicex.Core.AOT/Services/InstallerProvider.cs（1106 行）
//!
//! 本批范围（主结构与 Fabric 系）：字段/常量（DownloadSource、ForgeVersionCacheDir）、
//! SetDownloadSource、构造函数、GetAvailableModLoaders（All 合并 + 各单类型分支）、
//! GetFabricBaseUrl、WithTimeout、SortAndDeduplicate、VersionComparer.Compare、
//! VersionSortInteger、ConvertSpecialLabel、GetSupportedGameVersions、
//! NormalizeMinecraftVersion、GetMinecraftVersionAliases、MatchesMinecraftVersion、
//! SupportsMinecraftVersion、GetFabricVersions、GetQuiltVersions、GetOptifineVersions、
//! GetLiteloaderVersions、GetLegacyFabricVersions、GetBabricVersions、IsVersionBelowOrEqual。
//!
//! ⚠️ UNMAPPED（后续批次）：GetForgeVersions（BMCLAPI / Official HTML 双分支）、
//! GetNeoForgeFromOfficialApi / GetNeoForgeFromBmclApi、GetCleanroomVersions
//! —— GetAvailableModLoaders 的相应分支按源保留结构、以空列表占位（见各 stub 标注）。
//!
//! 命名说明：结构体命名 `InstallerProviderService`（规避与 api/installer.rs 的
//! `InstallerProvider` trait 同名冲突，详见翻译日志 p60）。
//!
//! 约定（见翻译日志 p60）：
//! - Trace.WriteLine → eprintln!（仓库既有惯例，同 lib_helper.rs / babric.rs）；
//! - 源 `DateTimeOffset.MinValue` → `DATETIME_OFFSET_MIN_VALUE` 文本
//!   （B1 决策：DateTimeOffset 字段 → String 原始文本保真，chrono 不引入）；
//! - ⚠️ 偏差（通用）：源 `j["field"]?.ToString()` 对非字符串 JSON 节点（数字/布尔/null）
//!   返回其文本（如 "null"），Rust 侧统一 `as_str()`（缺失/非字符串 → 空串）；
//!   实际端点字段均为字符串，语义等价；
//! - `Uri.EscapeDataString` → `escape_data_string`（RFC 3986 unreserved 集合一致）。

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;

use crate::api::installer::InstallerProvider;
use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::models::installer::{ModLoaderResult, ModLoaderType};

/// 源 `DateTimeOffset.MinValue` 的 ISO-8601 文本表示
/// （B1 决策：DateTimeOffset 字段 → String 原始文本保真，详见 MAPPING_TABLE b1_decisions）。
const DATETIME_OFFSET_MIN_VALUE: &str = "0001-01-01T00:00:00+00:00";

/// 源 `ForgeVersionCacheDir = Path.Combine(Path.GetTempPath(), "ForgeVersionCache")`
/// （供 Forge 官方 HTML 缓存使用；Forge 版本查询未在本批范围，常量保留待用）。
/// 安装器提供商实现（源：`internal class InstallerProvider : IInstallerProvider`）。
///
/// 命名说明：与 `crate::api::installer::InstallerProvider` trait 同名冲突规避，
/// 结构体命名为 `InstallerProviderService`（翻译日志 p60 决策 D1）。
pub(crate) struct InstallerProviderService {
    /// 源 `_http`（HttpClient → reqwest::Client，MAPPING_TABLE runtime 映射）
    http: reqwest::Client,
    /// 源 `_mirror`（DownloadMirror；BMCLAPI → Bmclapi）
    mirror: DownloadMirror,
}

impl InstallerProviderService {
    /// 对应源构造器 `InstallerProvider(HttpClient http, DownloadMirror mirror)`
    pub(crate) fn new(http: reqwest::Client, mirror: DownloadMirror) -> Self {
        Self { http, mirror }
    }

    /// 对应源私有 `GetFabricBaseUrl()`：BMCLAPI 镜像 → bmclapi2 fabric-meta，否则 meta.fabricmc.net。
    fn get_fabric_base_url(&self) -> &'static str {
        if self.mirror == DownloadMirror::Bmclapi {
            "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions"
        } else {
            "https://meta.fabricmc.net/v2/versions"
        }
    }

    // ==================== Fabric ====================

    /// 对应源 `GetFabricVersions(string minecraftVersion, string baseUrl)`：
    /// game 列表支持性检查 → `{baseUrl}/loader/{mcVersion}` → 逐项解析（loader.version / loader.stable）。
    /// 源 catch(HttpRequestException) → "Fabric API 请求失败"，catch(Exception) → "Fabric API 处理失败"；
    /// 成功/失败统一在 catch 外走 SortAndDeduplicate（源即如此）。
    async fn get_fabric_versions(
        &self,
        minecraft_version: &str,
        base_url: &str,
    ) -> Vec<ModLoaderResult> {
        let mut versions: Vec<ModLoaderResult> = Vec::new();
        let outcome: Result<(), Error> = async {
            // 源：var gameVersions = await GetSupportedGameVersions(_http, $"{baseUrl}/game");
            let game_versions = self
                .get_supported_game_versions(&format!("{base_url}/game"))
                .await?;
            if !supports_minecraft_version(&game_versions, minecraft_version) {
                // 源：Trace.WriteLine($"Fabric 不支持 MC 版本 {minecraftVersion}")，随后 return versions（空）
                eprintln!("Fabric 不支持 MC 版本 {minecraft_version}");
                return Ok(());
            }

            // 源：var encodedMcVersion = Uri.EscapeDataString(minecraftVersion);
            let encoded_mc_version = escape_data_string(minecraft_version);
            let loader_url = format!("{base_url}/loader/{encoded_mc_version}");
            let loader_response =
                self.http
                    .get(&loader_url)
                    .send()
                    .await
                    .map_err(|e| Error::Http {
                        message: format!("GET {loader_url} 失败"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
            if !loader_response.status().is_success() {
                return Err(Error::Http {
                    message: format!("请求失败，状态码 {}", loader_response.status()),
                    status: None,
                    source: None,
                });
            }
            let loader_json = loader_response.text().await.map_err(|e| Error::Http {
                message: format!("读取响应体失败: {loader_url}"),
                status: None,
                source: Some(Box::new(e)),
            })?;

            // 源：var loaderArray = JsonNode.Parse(loaderJson)!.AsArray();
            let loader_array = parse_json_array(&loader_json, "Fabric loader 响应")?;

            for item in loader_array {
                // 源：var loaderInfo = item["loader"] as JsonObject; if (loaderInfo == null) continue;
                let Some(loader_info) = item.get("loader") else {
                    continue;
                };

                let loader_version = loader_info
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_stable = loader_info
                    .get("stable")
                    .map(|s| match s {
                        // 源：loaderInfo["stable"]?.ToString().Equals("true", OrdinalIgnoreCase)
                        //     JSON 布尔 true → "True" → 也匹配（faithful 处理）
                        Value::String(s) => s.eq_ignore_ascii_case("true"),
                        Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);

                if loader_version.is_empty() {
                    continue;
                }

                versions.push(new_loader_result(
                    ModLoaderType::Fabric,
                    loader_version,
                    minecraft_version,
                    "API未提供",
                    "",
                    is_stable,
                ));
            }
            // 源：Trace.WriteLine($"Fabric：成功解析 {versions.Count} 个版本");
            eprintln!("Fabric：成功解析 {} 个版本", versions.len());
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {}
            // 源：catch (HttpRequestException ex) —— 请求/状态码失败
            Err(Error::Http { message, .. }) => eprintln!("Fabric API 请求失败：{message}"),
            // 源：catch (Exception ex)
            Err(e) => eprintln!("Fabric API 处理失败：{e}"),
        }
        Self::sort_and_deduplicate(versions)
    }

    // ==================== Quilt ====================

    /// 对应源 `GetQuiltVersions(string minecraftVersion)`：
    /// game 支持性检查 → 按版本别名逐个查 `{baseUrl}/loader/{alias}`（首个非空结果 break；
    /// 404 等失败跳过换下一别名）→ 全空时回退全局 `{baseUrl}/loader`（hashed/intermediary
    /// 版本匹配）→ 逐项解析。端点固定 `https://meta.quiltmc.org/v3/versions`（源方法内字面量）。
    async fn get_quilt_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        let base_url = "https://meta.quiltmc.org/v3/versions";
        let mut versions: Vec<ModLoaderResult> = Vec::new();
        let outcome: Result<(), Error> = async {
            let game_versions = self
                .get_supported_game_versions(&format!("{base_url}/game"))
                .await?;
            if !supports_minecraft_version(&game_versions, minecraft_version) {
                eprintln!("Quilt 不支持 MC 版本 {minecraft_version}");
                return Ok(());
            }

            // 源：foreach (var alias in GetMinecraftVersionAliases(minecraftVersion))
            let mut loader_array: Vec<Value> = Vec::new();
            for alias in get_minecraft_version_aliases(minecraft_version) {
                let encoded_mc_version = escape_data_string(&alias);
                let url = format!("{base_url}/loader/{encoded_mc_version}");
                let loader_response =
                    self.http.get(&url).send().await.map_err(|e| Error::Http {
                        message: format!("GET {url} 失败"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
                // 源：if (!loaderResponse.IsSuccessStatusCode) continue;
                if !loader_response.status().is_success() {
                    continue;
                }
                let loader_json = loader_response.text().await.map_err(|e| Error::Http {
                    message: format!("读取响应体失败: {url}"),
                    status: None,
                    source: Some(Box::new(e)),
                })?;
                loader_array = parse_json_array(&loader_json, "Quilt loader 响应")?;
                // 源：if (loaderArray.Count > 0) break;
                if !loader_array.is_empty() {
                    break;
                }
            }

            if loader_array.is_empty() {
                // 源：var globalLoaderResponse = await _http.GetAsync($"{baseUrl}/loader");
                let url = format!("{base_url}/loader");
                let global_loader_response =
                    self.http.get(&url).send().await.map_err(|e| Error::Http {
                        message: format!("GET {url} 失败"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
                // 源：globalLoaderResponse.EnsureSuccessStatusCode();
                if !global_loader_response.status().is_success() {
                    return Err(Error::Http {
                        message: format!("请求失败，状态码 {}", global_loader_response.status()),
                        status: None,
                        source: None,
                    });
                }
                let global_loader_json =
                    global_loader_response
                        .text()
                        .await
                        .map_err(|e| Error::Http {
                            message: format!("读取响应体失败: {url}"),
                            status: None,
                            source: Some(Box::new(e)),
                        })?;
                let global_loader_items =
                    parse_json_array(&global_loader_json, "Quilt 全局 loader 响应")?;
                // 源：item["hashed"]?["version"]?.ToString() / item["intermediary"]?["version"]?.ToString()
                //     → MatchesMinecraftVersion（?? string.Empty）
                loader_array = global_loader_items
                    .into_iter()
                    .filter(|item| {
                        let hashed_version = item
                            .get("hashed")
                            .and_then(|h| h.get("version"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let intermediary_version = item
                            .get("intermediary")
                            .and_then(|i| i.get("version"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        matches_minecraft_version(hashed_version, minecraft_version)
                            || matches_minecraft_version(intermediary_version, minecraft_version)
                    })
                    .collect();
            }

            for item in loader_array {
                let Some(loader_info) = item.get("loader") else {
                    continue;
                };
                let loader_version = loader_info
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_stable = loader_info
                    .get("stable")
                    .map(|s| match s {
                        Value::String(s) => s.eq_ignore_ascii_case("true"),
                        Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);
                if loader_version.is_empty() {
                    continue;
                }
                versions.push(new_loader_result(
                    ModLoaderType::Quilt,
                    loader_version,
                    minecraft_version,
                    "API未提供",
                    "",
                    is_stable,
                ));
            }
            eprintln!("Quilt：成功解析 {} 个版本", versions.len());
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {}
            Err(Error::Http { message, .. }) => eprintln!("Quilt API 请求失败：{message}"),
            Err(e) => eprintln!("Quilt API 处理失败：{e}"),
        }
        Self::sort_and_deduplicate(versions)
    }

    // ==================== OptiFine ====================

    /// 对应源 `GetOptifineVersions(string minecraftVersion)`：
    /// 按版本别名逐个查 `https://bmclapi2.bangbang93.com/optifine/{alias}`（内层 try/catch
    /// 逐别名容错，首个非空结果 break）→ 解析 mcversion/patch/type/forge →
    /// 构造 `{type}-{patch}` 版本与下载 URL。⚠️ 源 catch 外 `return result`（未排序、未去重）。
    async fn get_optifine_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        let mut result: Vec<ModLoaderResult> = Vec::new();
        let outcome: Result<(), Error> = async {
            let mut optifine_list: Vec<Value> = Vec::new();
            for alias in get_minecraft_version_aliases(minecraft_version) {
                let alias_result: Result<(), Error> = async {
                    let url = format!(
                        "https://bmclapi2.bangbang93.com/optifine/{}",
                        escape_data_string(&alias)
                    );
                    let response = self.http.get(&url).send().await.map_err(|e| Error::Http {
                        message: format!("GET {url} 失败"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
                    if !response.status().is_success() {
                        return Err(Error::Http {
                            message: format!("请求失败，状态码 {}", response.status()),
                            status: None,
                            source: None,
                        });
                    }
                    let json = response.text().await.map_err(|e| Error::Http {
                        message: format!("读取响应体失败: {url}"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
                    // 源：optifineList = JsonNode.Parse(json)!.AsArray().OfType<JsonObject>().ToList();
                    optifine_list = parse_json_array(&json, "Optifine 列表")?;
                    Ok(())
                }
                .await;

                // 源：内层 catch —— 记录后继续下一别名
                match alias_result {
                    Ok(()) => {
                        if !optifine_list.is_empty() {
                            break;
                        }
                    }
                    Err(e) => eprintln!("Optifine 获取列表失败 (MC版本: {alias}): {e}"),
                }
            }

            for info in optifine_list {
                let mc_ver = info
                    .get("mcversion")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let patch = info
                    .get("patch")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let r#type = info
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let forge = info
                    .get("forge")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();

                // 源：if (string.IsNullOrEmpty(mcVer) || string.IsNullOrEmpty(type) || string.IsNullOrEmpty(patch)) continue;
                if mc_ver.is_empty() || r#type.is_empty() || patch.is_empty() {
                    continue;
                }

                let download_url = format!(
                    "https://bmclapi2.bangbang93.com/optifine/{}/{}/{}",
                    escape_data_string(&mc_ver),
                    r#type,
                    patch
                );
                result.push(new_loader_result(
                    ModLoaderType::OptiFine,
                    &format!("{}-{patch}", r#type),
                    &mc_ver,
                    &download_url,
                    "",
                    // 源：forge.Contains("Forge N/A")
                    forge.contains("Forge N/A"),
                ));
            }
            Ok(())
        }
        .await;

        match outcome {
            // 源：try 内 return SortAndDeduplicate(result);
            Ok(()) => Self::sort_and_deduplicate(result),
            // 源：catch —— Trace.WriteLine($"OptiFine 版本获取失败: {ex.Message}")，return result（未排序）
            Err(e) => {
                eprintln!("OptiFine 版本获取失败: {e}");
                result
            }
        }
    }

    // ==================== LiteLoader ====================

    /// 对应源 `GetLiteloaderVersions(string minecraftVersion)`：
    /// 空版本直接返回 → `https://bmclapi2.bangbang93.com/liteloader/list/?mcversion={v}` →
    /// 数组或单对象兼容解析（源内层 try/catch：AsArray 失败回退 JsonObject 单条）→ 逐项解析。
    /// ⚠️ 源 catch 外 `return result`（未排序、未去重）。
    async fn get_liteloader_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        let mut result: Vec<ModLoaderResult> = Vec::new();
        let outcome: Result<(), Error> = async {
            // 源：if (string.IsNullOrEmpty(minecraftVersion)) return result;
            if minecraft_version.trim().is_empty() {
                return Ok(());
            }

            let url = format!(
                "https://bmclapi2.bangbang93.com/liteloader/list/?mcversion={}",
                escape_data_string(minecraft_version)
            );
            let response = self.http.get(&url).send().await.map_err(|e| Error::Http {
                message: format!("GET {url} 失败"),
                status: None,
                source: Some(Box::new(e)),
            })?;
            if !response.status().is_success() {
                return Err(Error::Http {
                    message: format!("请求失败，状态码 {}", response.status()),
                    status: None,
                    source: None,
                });
            }
            let json = response.text().await.map_err(|e| Error::Http {
                message: format!("读取响应体失败: {url}"),
                status: None,
                source: Some(Box::new(e)),
            })?;

            // 源：内层 try：AsArray；catch：JsonObject 单条（二次 Parse 仍失败 → 抛向外层 catch）
            let mut liteloader_list: Vec<Value> = Vec::new();
            match parse_json_array(&json, "LiteLoader 列表") {
                Ok(array) => liteloader_list = array,
                Err(_) => {
                    // 源：var single = JsonNode.Parse(json) as JsonObject; if (single != null) liteloaderList.Add(single);
                    let single: Value = serde_json::from_str(&json).map_err(|e| Error::Http {
                        message: "解析 LiteLoader 单对象 JSON 失败".to_string(),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
                    if single.is_object() {
                        liteloader_list.push(single);
                    }
                }
            }

            for info in liteloader_list {
                let mc_ver = info
                    .get("mcversion")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let version = info
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let hash = info
                    .get("hash")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();

                // 源：if (string.IsNullOrEmpty(version)) continue;
                if version.is_empty() {
                    continue;
                }

                let download_url = format!(
                    "https://bmclapi2.bangbang93.com/liteloader/download/?version={}",
                    escape_data_string(&version)
                );
                result.push(new_loader_result(
                    ModLoaderType::LiteLoader,
                    &version,
                    &mc_ver,
                    &download_url,
                    &hash,
                    true,
                ));
            }
            Ok(())
        }
        .await;

        match outcome {
            // 源：try 内 return SortAndDeduplicate(result);
            Ok(()) => Self::sort_and_deduplicate(result),
            // 源：catch —— Trace.WriteLine($"LiteLoader 版本获取失败: {ex.Message}")，return result（未排序）
            Err(e) => {
                eprintln!("LiteLoader 版本获取失败: {e}");
                result
            }
        }
    }

    // ==================== Legacy Fabric ====================

    /// 对应源 `GetLegacyFabricVersions(string minecraftVersion)`：
    /// 空版本 → 空；仅支持 MC 1.12.2 及以下（IsVersionBelowOrEqual）；game 支持性检查 →
    /// `{baseUrl}/loader/{mcVersion}` → 逐项解析。端点固定
    /// `https://meta.legacyfabric.net/v2/versions`（源方法内 const）。
    async fn get_legacy_fabric_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        let mut versions: Vec<ModLoaderResult> = Vec::new();
        let outcome: Result<(), Error> = async {
            // 源：if (string.IsNullOrEmpty(minecraftVersion)) return versions;
            if minecraft_version.trim().is_empty() {
                return Ok(());
            }

            // ponytail: Legacy Fabric 仅支持 MC 1.12.2 及以下（源注释）
            if !is_version_below_or_equal(minecraft_version, "1.12.2") {
                eprintln!(
                    "Legacy Fabric 不支持 MC 版本 {minecraft_version}（仅支持 1.12.2 及以下）"
                );
                return Ok(());
            }

            // 源：const string baseUrl = "https://meta.legacyfabric.net/v2/versions";
            const BASE_URL: &str = "https://meta.legacyfabric.net/v2/versions";
            let game_versions = self
                .get_supported_game_versions(&format!("{BASE_URL}/game"))
                .await?;
            if !supports_minecraft_version(&game_versions, minecraft_version) {
                eprintln!("Legacy Fabric 不支持 MC 版本 {minecraft_version}");
                return Ok(());
            }

            let encoded_mc_version = escape_data_string(minecraft_version);
            let loader_url = format!("{BASE_URL}/loader/{encoded_mc_version}");
            let loader_response =
                self.http
                    .get(&loader_url)
                    .send()
                    .await
                    .map_err(|e| Error::Http {
                        message: format!("GET {loader_url} 失败"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
            if !loader_response.status().is_success() {
                return Err(Error::Http {
                    message: format!("请求失败，状态码 {}", loader_response.status()),
                    status: None,
                    source: None,
                });
            }
            let loader_json = loader_response.text().await.map_err(|e| Error::Http {
                message: format!("读取响应体失败: {loader_url}"),
                status: None,
                source: Some(Box::new(e)),
            })?;
            let loader_array = parse_json_array(&loader_json, "Legacy Fabric loader 响应")?;

            for item in loader_array {
                let Some(loader_info) = item.get("loader") else {
                    continue;
                };
                let loader_version = loader_info
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_stable = loader_info
                    .get("stable")
                    .map(|s| match s {
                        Value::String(s) => s.eq_ignore_ascii_case("true"),
                        Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);
                if loader_version.is_empty() {
                    continue;
                }
                versions.push(new_loader_result(
                    ModLoaderType::LegacyFabric,
                    loader_version,
                    minecraft_version,
                    "API未提供",
                    "",
                    is_stable,
                ));
            }
            eprintln!("Legacy Fabric：成功解析 {} 个版本", versions.len());
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {}
            Err(Error::Http { message, .. }) => eprintln!("Legacy Fabric API 请求失败：{message}"),
            Err(e) => eprintln!("Legacy Fabric API 处理失败：{e}"),
        }
        Self::sort_and_deduplicate(versions)
    }

    // ==================== Babric ====================

    /// 对应源 `GetBabricVersions(string minecraftVersion)`：game 支持性检查 →
    /// `{baseUrl}/loader/{mcVersion}` → 逐项解析。端点固定
    /// `https://meta.babric.glass-launcher.net/v2/versions`（源方法内 const）。
    async fn get_babric_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        let mut versions: Vec<ModLoaderResult> = Vec::new();
        let outcome: Result<(), Error> = async {
            // 源：const string baseUrl = "https://meta.babric.glass-launcher.net/v2/versions";
            const BASE_URL: &str = "https://meta.babric.glass-launcher.net/v2/versions";
            let game_versions = self
                .get_supported_game_versions(&format!("{BASE_URL}/game"))
                .await?;
            if !supports_minecraft_version(&game_versions, minecraft_version) {
                eprintln!("Babric 不支持 MC 版本 {minecraft_version}");
                return Ok(());
            }

            let encoded_mc_version = escape_data_string(minecraft_version);
            let loader_url = format!("{BASE_URL}/loader/{encoded_mc_version}");
            let loader_response =
                self.http
                    .get(&loader_url)
                    .send()
                    .await
                    .map_err(|e| Error::Http {
                        message: format!("GET {loader_url} 失败"),
                        status: None,
                        source: Some(Box::new(e)),
                    })?;
            if !loader_response.status().is_success() {
                return Err(Error::Http {
                    message: format!("请求失败，状态码 {}", loader_response.status()),
                    status: None,
                    source: None,
                });
            }
            let loader_json = loader_response.text().await.map_err(|e| Error::Http {
                message: format!("读取响应体失败: {loader_url}"),
                status: None,
                source: Some(Box::new(e)),
            })?;
            let loader_array = parse_json_array(&loader_json, "Babric loader 响应")?;

            for item in loader_array {
                let Some(loader_info) = item.get("loader") else {
                    continue;
                };
                let loader_version = loader_info
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_stable = loader_info
                    .get("stable")
                    .map(|s| match s {
                        Value::String(s) => s.eq_ignore_ascii_case("true"),
                        Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);
                if loader_version.is_empty() {
                    continue;
                }
                versions.push(new_loader_result(
                    ModLoaderType::Babric,
                    loader_version,
                    minecraft_version,
                    "API未提供",
                    "",
                    is_stable,
                ));
            }
            eprintln!("Babric：成功解析 {} 个版本", versions.len());
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {}
            Err(Error::Http { message, .. }) => eprintln!("Babric API 请求失败：{message}"),
            Err(e) => eprintln!("Babric API 处理失败：{e}"),
        }
        Self::sort_and_deduplicate(versions)
    }

    // ==================== Forge / NeoForge / Cleanroom（UNMAPPED 占位） ====================

    /// 源 `GetForgeVersions`（镜像分派 GetForgeVersionsFromBmclApi / GetForgeVersionsFromOfficialHtml，
    /// 含 HTML 正则解析与 ForgeVersionCacheDir 缓存；实现在 provider_forge.rs，B13 接线）
    async fn get_forge_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        crate::services::installers::provider_forge::get_forge_versions(
            &self.http,
            self.mirror,
            minecraft_version,
        )
        .await
        .unwrap_or_default()
    }

    /// 源 NeoForge 镜像分派
    /// `_mirror == DownloadMirror.Official ? GetNeoForgeFromOfficialApi : GetNeoForgeFromBmclApi`
    /// （Official：maven.neoforged.net 双端点 + ParseNeoForgeMinecraftVersion；BMCLAPI 镜像）；
    /// 实现在 provider_forge.rs，B13 接线
    ///
    /// 额外行为：Official 源无结果时自动回退 BMCLAPI，避免国内网络下官方 API 不可达导致列表为空。
    async fn get_neoforge_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        if self.mirror == DownloadMirror::Official {
            let official =
                crate::services::installers::provider_forge::get_neoforge_from_official_api(
                    &self.http,
                    self.mirror,
                    minecraft_version,
                )
                .await
                .unwrap_or_default();
            if !official.is_empty() {
                return official;
            }
            crate::services::installers::provider_forge::get_neoforge_from_bmcl_api(
                &self.http,
                self.mirror,
                minecraft_version,
            )
            .await
            .unwrap_or_default()
        } else {
            crate::services::installers::provider_forge::get_neoforge_from_bmcl_api(
                &self.http,
                self.mirror,
                minecraft_version,
            )
            .await
            .unwrap_or_default()
        }
    }

    /// 源 `GetCleanroomVersions`（GitHub releases API：
    /// `https://api.github.com/repos/CleanroomMC/Cleanroom/releases`，仅 1.12.2）；
    /// 实现在 provider_forge.rs，B13 接线
    async fn get_cleanroom_versions(&self, minecraft_version: &str) -> Vec<ModLoaderResult> {
        crate::services::installers::provider_forge::get_cleanroom_versions(
            &self.http,
            self.mirror,
            minecraft_version,
        )
        .await
        .unwrap_or_default()
    }

    // ==================== 基础工具方法 ====================

    /// 对应源私有静态 `GetSupportedGameVersions(HttpClient client, string gameVersionsUrl)`：
    /// GET → 200 校验 → 数组逐项取 "version" 字段（空值过滤）→ HashSet。
    async fn get_supported_game_versions(
        &self,
        game_versions_url: &str,
    ) -> Result<HashSet<String>, Error> {
        let response = self
            .http
            .get(game_versions_url)
            .send()
            .await
            .map_err(|e| Error::Http {
                message: format!("GET {game_versions_url} 失败"),
                status: None,
                source: Some(Box::new(e)),
            })?;
        if !response.status().is_success() {
            return Err(Error::Http {
                message: format!("请求失败，状态码 {}", response.status()),
                status: None,
                source: None,
            });
        }
        let json = response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应体失败: {game_versions_url}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let parsed: Value = serde_json::from_str(&json).map_err(|e| Error::Http {
            message: format!("解析 {game_versions_url} JSON 失败"),
            status: None,
            source: Some(Box::new(e)),
        })?;

        let mut result: HashSet<String> = HashSet::new();
        if let Some(items) = parsed.as_array() {
            for item in items {
                // 源：j["version"]?.ToString()，空值过滤（见文件头通用偏差说明）
                let version = item
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !version.is_empty() {
                    result.insert(version.to_string());
                }
            }
        }
        Ok(result)
    }

    /// 对应源私有静态 `WithTimeout(Task task, int timeoutMs = 10000)`：10 秒超时返回空列表。
    /// ⚠️ 偏差：C# 超时后内层任务仍在后台继续执行；Rust `tokio::time::timeout` 超时会取消
    /// （drop）future——加载器查询无外部副作用，语义等价。
    async fn with_timeout(
        future: impl std::future::Future<Output = Vec<ModLoaderResult>>,
    ) -> Vec<ModLoaderResult> {
        match tokio::time::timeout(Duration::from_secs(10), future).await {
            Ok(result) => result,
            Err(_) => Vec::new(),
        }
    }

    /// 对应源私有静态 `SortAndDeduplicate`：
    /// 按 Version 分组去重（保留组内首项，同 C# GroupBy + First）→ 降序排序（VersionComparer）。
    fn sort_and_deduplicate(versions: Vec<ModLoaderResult>) -> Vec<ModLoaderResult> {
        let mut deduped: Vec<ModLoaderResult> = Vec::new();
        for v in versions {
            if !deduped.iter().any(|d| d.version == v.version) {
                deduped.push(v);
            }
        }
        Self::sort_descending(&mut deduped);
        deduped
    }

    /// 对应源 `.OrderByDescending(l => l.Version, new VersionComparer())`（降序，不去重）。
    fn sort_descending(versions: &mut Vec<ModLoaderResult>) {
        versions.sort_by(|a, b| compare(&b.version, &a.version).cmp(&0));
    }
}

// ==================== 版本排序（源 InstallerProvider.cs 本地副本） ====================

/// 对应源私有类 `VersionComparer.Compare(x, y)`：VersionSortInteger 包装。
fn compare(x: &str, y: &str) -> i32 {
    version_sort_integer(x, y)
}

/// 版本号比较（对应源私有静态 `VersionSortInteger`）。
///
/// ⚠️ 与 util/lib_helper.rs 的 `version_sort_integer` 不同：源 InstallerProvider.cs 版本
/// 含「未知版本」特判与「快照/预览版」标签替换，且 lib_helper 为 private —— 故保留本地副本。
/// 返回 -1/0/1（源 VersionComparer 仅消费符号；内部 string.Compare(Ordinal) 按
/// lib_helper.rs 惯例归一为 -1/0/1，版本串均为 ASCII，语义等价）。
fn version_sort_integer(left: &str, right: &str) -> i32 {
    if left == "未知版本" || right == "未知版本" {
        if left == "未知版本" && right != "未知版本" {
            return 1;
        }
        if left != "未知版本" && right == "未知版本" {
            return -1;
        }
        return 0;
    }

    // 源：left.ToLowerInvariant().Replace("快照", "snapshot").Replace("预览版", "pre")
    let left = left
        .to_lowercase()
        .replace("快照", "snapshot")
        .replace("预览版", "pre");
    let right = right
        .to_lowercase()
        .replace("快照", "snapshot")
        .replace("预览版", "pre");

    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    let token_re = TOKEN_RE.get_or_init(|| Regex::new(r"[a-z]+|[0-9]+").expect("静态正则编译失败"));

    // 源：Regex.Matches(left, "[a-z]+|[0-9]+") 分词
    let left_parts: Vec<String> = token_re
        .find_iter(&left)
        .map(|m| m.as_str().to_string())
        .collect();
    let right_parts: Vec<String> = token_re
        .find_iter(&right)
        .map(|m| m.as_str().to_string())
        .collect();

    let mut i = 0;
    loop {
        // 源：i >= 两侧 Count → string.Compare(left, right, Ordinal)
        if i >= left_parts.len() && i >= right_parts.len() {
            return string_compare_ordinal(&left, &right);
        }

        // 源：越界补 "-1"
        let l_val = left_parts
            .get(i)
            .cloned()
            .unwrap_or_else(|| "-1".to_string());
        let r_val = right_parts
            .get(i)
            .cloned()
            .unwrap_or_else(|| "-1".to_string());

        if l_val == r_val {
            i += 1;
            continue;
        }

        let l_val = convert_special_label(&l_val);
        let r_val = convert_special_label(&r_val);

        // 源：int.TryParse（i32 与 C# int 一致）；任一侧解析失败 → 序数比较
        match (l_val.parse::<i32>(), r_val.parse::<i32>()) {
            (Ok(l_num), Ok(r_num)) => {
                if l_num > r_num {
                    return 1;
                }
                if l_num < r_num {
                    return -1;
                }
                i += 1;
            }
            _ => return string_compare_ordinal(&l_val, &r_val),
        }
    }
}

/// 特殊版本标签转换（对应源私有静态 `ConvertSpecialLabel`；内容与 util/lib_helper.rs 的
/// convert_special_label 相同，但属源 InstallerProvider.cs 的本地副本，保留）。
fn convert_special_label(label: &str) -> String {
    match label {
        "pre" | "snapshot" => "-3".to_string(),
        "rc" => "-2".to_string(),
        "experimental" => "-4".to_string(),
        _ => label.to_string(),
    }
}

/// C# `string.Compare(left, right, StringComparison.Ordinal)` 的符号等价物（-1/0/1；
/// 同 util/lib_helper.rs string_compare_ordinal 惯例；版本串均为 ASCII，UTF-8 字节序
/// 与 UTF-16 码元序语义等价）。
fn string_compare_ordinal(left: &str, right: &str) -> i32 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

// ==================== Minecraft 版本工具方法 ====================

/// 对应源私有静态 `NormalizeMinecraftVersion`：
/// 空 → 空串；以 "1." 开头原样返回；否则取 '-' 前主干按 '.' 分割，
/// 首段 ≥ 22（快照版本号，如 24w14a）→ 补 "1." 前缀。
fn normalize_minecraft_version(version: &str) -> String {
    if version.trim().is_empty() {
        return String::new();
    }

    let version = version.trim();
    if version.starts_with("1.") {
        return version.to_string();
    }

    // 源：var dashIndex = version.IndexOf('-'); var baseVersion = dashIndex >= 0 ? version[..dashIndex] : version;
    let base_version = match version.find('-') {
        Some(dash_index) => &version[..dash_index],
        None => version,
    };
    // 源：baseVersion.Split('.', StringSplitOptions.RemoveEmptyEntries)
    let parts: Vec<&str> = base_version.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return version.to_string();
    }

    // 源：int.TryParse(parts[0], out var major) && major >= 22 ? $"1.{baseVersion}" : version;
    match parts[0].parse::<i32>() {
        Ok(major) if major >= 22 => format!("1.{base_version}"),
        _ => version.to_string(),
    }
}

/// 对应源私有静态 `GetMinecraftVersionAliases`：返回 [原版本, 规范化版本, 去 "1." 前缀或补 "1." 前缀]，
/// 去重（源 HashSet(StringComparer.OrdinalIgnoreCase) → Vec + 忽略大小写判重，保序保原大小写）。
fn get_minecraft_version_aliases(version: &str) -> Vec<String> {
    let mut aliases: Vec<String> = Vec::new();
    if version.trim().is_empty() {
        return aliases;
    }

    let mut push_unique = |s: String| {
        if !aliases.iter().any(|a| a.eq_ignore_ascii_case(&s)) {
            aliases.push(s);
        }
    };

    push_unique(version.to_string());
    let normalized = normalize_minecraft_version(version);
    push_unique(normalized.clone());

    // 源：normalized.StartsWith("1.") ? normalized[2..] : $"1.{normalized}"
    if normalized.starts_with("1.") {
        push_unique(normalized[2..].to_string());
    } else {
        push_unique(format!("1.{normalized}"));
    }

    aliases
}

/// 对应源私有静态 `MatchesMinecraftVersion`：两侧别名集合交集（忽略大小写）。
fn matches_minecraft_version(candidate_version: &str, requested_version: &str) -> bool {
    if candidate_version.trim().is_empty() || requested_version.trim().is_empty() {
        return false;
    }

    let candidate_aliases = get_minecraft_version_aliases(candidate_version);
    let requested_aliases = get_minecraft_version_aliases(requested_version);
    candidate_aliases
        .iter()
        .any(|c| requested_aliases.iter().any(|r| c.eq_ignore_ascii_case(r)))
}

/// 对应源私有静态 `SupportsMinecraftVersion`：supportedVersions 全量展开别名（忽略大小写）
/// 后，requestedVersion 任一带用名命中即支持。
fn supports_minecraft_version(
    supported_versions: &HashSet<String>,
    requested_version: &str,
) -> bool {
    let mut normalized_supported: HashSet<String> = HashSet::new();
    for supported_version in supported_versions {
        for alias in get_minecraft_version_aliases(supported_version) {
            normalized_supported.insert(alias.to_lowercase());
        }
    }
    get_minecraft_version_aliases(requested_version)
        .iter()
        .any(|a| normalized_supported.contains(&a.to_lowercase()))
}

/// 对应源私有静态 `IsVersionBelowOrEqual`（ponytail: 简单版本比较，不处理复杂版本格式）：
/// 按 ['.', '-', '_'] 分割，逐位数值比较，非数字段按 0；任一 v < r → true，v > r → false，
/// 全部相等 → true。源 try/catch 恒返回 false 的分支在 Rust 不可达（split/parse 不 panic），省略。
fn is_version_below_or_equal(version: &str, reference: &str) -> bool {
    let split_parts = |s: &str| -> Vec<i32> {
        s.split(['.', '-', '_'])
            .filter(|p| !p.is_empty())
            .map(|p| p.parse::<i32>().unwrap_or(0))
            .collect()
    };

    let v_parts = split_parts(version);
    let r_parts = split_parts(reference);

    for i in 0..v_parts.len().max(r_parts.len()) {
        // 源：越界按 0（i < Count && int.TryParse ? val : 0）
        let v = v_parts.get(i).copied().unwrap_or(0);
        let r = r_parts.get(i).copied().unwrap_or(0);
        if v < r {
            return true;
        }
        if v > r {
            return false;
        }
    }
    true
}

// ==================== 通用小工具 ====================

/// 对应源 `Uri.EscapeDataString`：RFC 3986 unreserved（A-Z a-z 0-9 - . _ ~）原样，
/// 其余按 UTF-8 字节 %XX 大写十六进制编码（.NET EscapeDataString 行为一致）。
fn escape_data_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 解析 JSON 文本为数组（对应源 `JsonNode.Parse(json)!.AsArray()`；解析失败或非数组 → Err，
/// 由调用方映射到源 catch 分支）。
fn parse_json_array(json: &str, what: &str) -> Result<Vec<Value>, Error> {
    let value: Value = serde_json::from_str(json).map_err(|e| Error::Http {
        message: format!("解析 {what} JSON 失败"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    value.as_array().cloned().ok_or_else(|| Error::Http {
        message: format!("{what} JSON 非数组"),
        status: None,
        source: None,
    })
}

/// 构造 ModLoaderResult（对应源 7 参构造；
/// 源 `DateTimeOffset.MinValue` → `DATETIME_OFFSET_MIN_VALUE` 文本，见文件头约定）。
fn new_loader_result(
    r#type: ModLoaderType,
    version: &str,
    game_version: &str,
    url: &str,
    sha1: &str,
    is_recommand: bool,
) -> ModLoaderResult {
    ModLoaderResult {
        r#type,
        version: version.to_string(),
        game_version: game_version.to_string(),
        url: url.to_string(),
        sha1: sha1.to_string(),
        is_recommand,
        release_time: DATETIME_OFFSET_MIN_VALUE.to_string(),
    }
}

#[async_trait]
impl InstallerProvider for InstallerProviderService {
    /// 对应源 `GetAvailableModLoaders(string gameVersion, ModLoaderType type = ModLoaderType.All)`。
    /// C# 默认参数 `type = All`：Rust 无默认参数，调用方需显式传 `ModLoaderType::All`
    /// （同 api/installer.rs 契约说明）。
    async fn get_available_mod_loaders(
        &self,
        game_version: &str,
        r#type: ModLoaderType,
    ) -> Result<Vec<ModLoaderResult>, Error> {
        // 源：if (type == ModLoaderType.All) —— 9 个加载器并行（Task.WhenAll）+ 各 10s 超时（WithTimeout）
        if r#type == ModLoaderType::All {
            let (
                forge_task,
                fabric_task,
                neo_forge_task,
                quilt_task,
                optifine_task,
                liteloader_task,
                cleanroom_task,
                legacy_fabric_task,
                babric_task,
            ) = tokio::join!(
                InstallerProviderService::with_timeout(self.get_forge_versions(game_version)),
                InstallerProviderService::with_timeout(
                    self.get_fabric_versions(game_version, self.get_fabric_base_url())
                ),
                InstallerProviderService::with_timeout(self.get_neoforge_versions(game_version)),
                InstallerProviderService::with_timeout(self.get_quilt_versions(game_version)),
                InstallerProviderService::with_timeout(self.get_optifine_versions(game_version)),
                InstallerProviderService::with_timeout(self.get_liteloader_versions(game_version)),
                InstallerProviderService::with_timeout(self.get_cleanroom_versions(game_version)),
                InstallerProviderService::with_timeout(
                    self.get_legacy_fabric_versions(game_version)
                ),
                InstallerProviderService::with_timeout(self.get_babric_versions(game_version)),
            );

            // 源 AddRange 顺序：forge, fabric, neoForge, optifine, liteloader, quilt, cleanroom, legacyFabric, babric
            let mut all_loaders: Vec<ModLoaderResult> = Vec::new();
            all_loaders.extend(forge_task);
            all_loaders.extend(fabric_task);
            all_loaders.extend(neo_forge_task);
            all_loaders.extend(optifine_task);
            all_loaders.extend(liteloader_task);
            all_loaders.extend(quilt_task);
            all_loaders.extend(cleanroom_task);
            all_loaders.extend(legacy_fabric_task);
            all_loaders.extend(babric_task);

            // 源：allLoaders.OrderByDescending(l => l.Version, new VersionComparer()).ToList()
            Self::sort_descending(&mut all_loaders);
            return Ok(all_loaders);
        }

        // 源：type switch —— 单类型分支：仅 OrderByDescending（无 WithTimeout、无去重）
        let mut loaders = match r#type {
            ModLoaderType::Forge => self.get_forge_versions(game_version).await, // ⚠️ UNMAPPED stub
            ModLoaderType::Fabric => {
                self.get_fabric_versions(game_version, self.get_fabric_base_url())
                    .await
            }
            ModLoaderType::Quilt => self.get_quilt_versions(game_version).await,
            ModLoaderType::LiteLoader => self.get_liteloader_versions(game_version).await,
            // 源：_mirror == DownloadMirror.Official ? GetNeoForgeFromOfficialApi : GetNeoForgeFromBmclApi
            ModLoaderType::NeoForge => self.get_neoforge_versions(game_version).await, // ⚠️ UNMAPPED stub
            ModLoaderType::OptiFine => self.get_optifine_versions(game_version).await,
            ModLoaderType::Cleanroom => self.get_cleanroom_versions(game_version).await, // ⚠️ UNMAPPED stub
            ModLoaderType::LegacyFabric => self.get_legacy_fabric_versions(game_version).await,
            ModLoaderType::Babric => self.get_babric_versions(game_version).await,
            // 源：_ => throw new ArgumentException($"不支持的ModLoader类型: {type}")
            _ => {
                return Err(Error::Params {
                    message: format!("不支持的ModLoader类型: {:?}", r#type),
                    source: None,
                });
            }
        };
        Self::sort_descending(&mut loaders);
        Ok(loaders)
    }
}
