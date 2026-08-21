//! 远程版本清单服务（B6，对应源：Services/VersionManifestService.cs）
//!
//! 说明：
//! - 本服务仅负责网络获取，无缓存：源的磁盘缓存（VersionManifestCache.cs，
//!   cache/version_manifest.json，有效期 5 分钟）由 VersionManagementService 持有
//!   （GetManifestAsync(forceRefresh) 内使用），Rust 侧随"版本管理服务"移植任务
//!   （VersionManagement::get_manifest）一并承载，本服务不实现缓存
//! - 端点 URL 逐字保留自源（无镜像切换逻辑，源本服务也未做 BMCLAPI 切换）
//! - 反序列化走 util::json_helper（对应源 CombinedJsonContext 的
//!   VersionManifestRoot / CompleteVersionMetadata 类型）
//! - 错误映射：网络/JSON 错误 → Error::Http（源 HttpRequestException/JsonException，
//!   B6 增补 Http 变体）；空 URL → Error::Params（源 ArgumentException）

use async_trait::async_trait;

use crate::api::version::VersionManifest;
use crate::error::Error;
use crate::models::version_manifest::VersionManifestRoot;
use crate::models::version_metadata::CompleteVersionMetadata;
use crate::util::json_helper::{
    deserialize_version_manifest, deserialize_version_metadata, parse_minecraft_datetime,
};

/// 版本清单下载地址（源：`private const string ManifestUrl`，逐字保留）
const MANIFEST_URL: &str = "https://launchermeta.mojang.com/mc/game/version_manifest.json";

/// 版本清单服务（源：`internal sealed class VersionManifestService : IVersionManifestService`）。
/// 提供远程版本清单下载与单版本元数据（version.json）获取。
pub(crate) struct VersionManifestService {
    /// 共享 HTTP 客户端（源：`_httpClient` HttpClient）
    http: reqwest::Client,
}

impl VersionManifestService {
    /// 创建版本清单服务（源：构造函数 `VersionManifestService(HttpClient httpClient)` 注入 HttpClient）
    pub(crate) fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl VersionManifest for VersionManifestService {
    /// 获取版本清单（源：GetVersionManifestAsync）。
    /// 网络/JSON 错误按源异常语义（HttpRequestException/JsonException）包装为 Error::Http。
    async fn get_version_manifest(&self) -> Result<VersionManifestRoot, Error> {
        let body = get_json(&self.http, MANIFEST_URL).await?;

        // 源：JsonSerializer.Deserialize(response, _ctx.VersionManifestRoot)
        //   ?? throw new JsonException("解析版本清单失败")
        let mut root = deserialize_version_manifest(&body)
            .map_err(|e| Error::Http {
                message: "解析版本清单失败".to_string(),
                status: None,
                source: Some(Box::new(e)),
            })?
            .ok_or_else(|| Error::Http {
                message: "解析版本清单失败".to_string(),
                status: None,
                source: None,
            })?;

        // 源：root with { Versions = root.Versions.Select(...) } —— 愚人节快照重命名：
        //   v.Type == "snapshot" && v.ReleaseTime.Month == 4 && v.ReleaseTime.Day == 1
        //   → v with { Type = "april_fools" }
        for version in &mut root.versions {
            // 源在反序列化时经 MinecraftDateTimeConverter 解析 releaseTime（失败抛 JsonException），
            // Rust 侧模型为字符串保真（B1 决策），解析推迟到此：失败按同源语义报错
            let time =
                parse_minecraft_datetime(&version.release_time).map_err(|msg| Error::Http {
                    message: msg,
                    status: None,
                    source: None,
                })?;
            if version.r#type == "snapshot" && time.month == 4 && time.day == 1 {
                version.r#type = "april_fools".to_string();
            }
        }

        Ok(root)
    }

    /// 从指定 URL 获取版本元数据（源：GetVersionMetadataAsync(string url)）。
    /// 空 URL → Error::Params（源 ArgumentException("元数据URL不能为空")）。
    async fn get_version_metadata(&self, url: &str) -> Result<CompleteVersionMetadata, Error> {
        if url.is_empty() {
            return Err(Error::Params {
                message: "元数据URL不能为空".to_string(),
                source: None,
            });
        }

        let body = get_json(&self.http, url).await?;

        // 源：JsonSerializer.Deserialize(response, _ctx.CompleteVersionMetadata)
        //   ?? throw new JsonException($"解析版本元数据失败: {url}")
        deserialize_version_metadata(&body)
            .map_err(|e| Error::Http {
                message: format!("解析版本元数据失败: {url}"),
                status: None,
                source: Some(Box::new(e)),
            })?
            .ok_or_else(|| Error::Http {
                message: format!("解析版本元数据失败: {url}"),
                status: None,
                source: None,
            })
    }
}

/// GET 并返回响应体文本（源：HttpClient.GetStringAsync）。
/// 非 2xx 按 .NET EnsureSuccessStatusCode 的 HttpRequestException 消息报错（Error::Http）；
/// 网络错误同样映射为 Error::Http。
async fn get_json(http: &reqwest::Client, url: &str) -> Result<String, Error> {
    let resp = http.get(url).send().await.map_err(http_err)?;
    let status = resp.status();
    let body = resp.text().await.map_err(http_err)?;

    if !status.is_success() {
        return Err(Error::Http {
            message: format!(
                "Response status code does not indicate success: {} ({}).",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            ),
            status: None,
            source: None,
        });
    }
    Ok(body)
}

/// 网络错误映射（源：HttpRequestException → Error::Http，消息格式沿用既有惯例）
fn http_err(e: reqwest::Error) -> Error {
    Error::Http {
        message: format!("HTTP 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    }
}
