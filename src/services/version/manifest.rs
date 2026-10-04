//! 远程版本清单服务（B6，对应源：Services/VersionManifestService.cs）
//!
//! 说明：
//! - 本服务仅负责网络获取，无缓存：源的磁盘缓存（VersionManifestCache.cs，
//!   cache/version_manifest.json，有效期 5 分钟）由 VersionManagementService 持有
//!   （GetManifestAsync(forceRefresh) 内使用），Rust 侧随"版本管理服务"移植任务
//!   （VersionManagement::get_manifest）一并承载，本服务不实现缓存
//! - 端点 URL 逐字保留自源；**BMCLAPI 镜像兜底与连接层重试为本 crate 新增**
//!   （issue #176 用户实测：launchermeta.mojang.com 国内网络下单次请求抖动即整单
//!   失败——「获取版本清单失败: HTTP 请求失败: error sending request」。
//!   `DefaultDownloadSourceManager` 明明定义了 meta 镜像改写却未覆盖此路径，
//!   故镜像 URL 形态与 mirror.rs 的 bug-for-bug 约定一致：`bmclapi /meta/{原始URL}`）
//! - 反序列化走 util::json_helper（对应源 CombinedJsonContext 的
//!   VersionManifestRoot / CompleteVersionMetadata 类型）
//! - 错误映射：网络/JSON 错误 → Error::Http（源 HttpRequestException/JsonException，
//!   B6 增补 Http 变体）；空 URL → Error::Params（源 ArgumentException）

use std::time::Duration;

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

/// BMCLAPI meta 镜像前缀（与 `DefaultDownloadSourceManager::generate_mirror_urls`
/// 对 meta 类 URL 的改写形态一致，见 mirror.rs 的 bug-for-bug 测试）。
const BMCLAPI_META_PREFIX: &str = "https://bmclapi2.bangbang93.com/meta/";

/// 连接层错误重试次数（含首次；与 completer.rs 的 `DownloadFileWithRetryAsync`
/// maxRetries 默认 3 对齐）。
const REQUEST_RETRIES: usize = 3;

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

    /// 解析版本清单正文（愚人节快照重命名 + 元数据时间解析）。
    ///
    /// 自 `get_version_manifest` 拆出，供官方 / BMCLAPI 镜像两条路径复用同一解析逻辑。
    fn parse_manifest(body: &str) -> Result<VersionManifestRoot, Error> {
        // 源：JsonSerializer.Deserialize(response, _ctx.VersionManifestRoot)
        //   ?? throw new JsonException("解析版本清单失败")
        let mut root = deserialize_version_manifest(body)
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
}

#[async_trait]
impl VersionManifest for VersionManifestService {
    /// 获取版本清单（源：GetVersionManifestAsync）。
    /// 网络/JSON 错误按源异常语义（HttpRequestException/JsonException）包装为 Error::Http。
    ///
    /// 本 crate 增补（issue #176 用户实测复盘）：官方地址先经连接层重试
    /// （[`REQUEST_RETRIES`] 次、退避 1s/2s），仍失败再切 BMCLAPI meta 镜像
    /// （同样带重试）——`launchermeta.mojang.com` 国内网络下单次抖动不应让
    /// 整个安装判死。
    async fn get_version_manifest(&self) -> Result<VersionManifestRoot, Error> {
        let official_err = match get_json_with_retry(&self.http, MANIFEST_URL).await {
            Ok(body) => return Self::parse_manifest(&body),
            Err(e) => e,
        };
        eprintln!("版本清单官方源获取失败（{official_err}），切换 BMCLAPI 镜像重试");
        let mirror_url = format!("{BMCLAPI_META_PREFIX}{MANIFEST_URL}");
        let body = get_json_with_retry(&self.http, &mirror_url)
            .await
            .map_err(|e| Error::Http {
                message: format!(
                    "版本清单官方源与 BMCLAPI 镜像均失败: 官方[{official_err}] 镜像[{e}]"
                ),
                status: None,
                source: None,
            })?;
        Self::parse_manifest(&body)
    }

    /// 从指定 URL 获取版本元数据（源：GetVersionMetadataAsync(string url)）。
    /// 空 URL → Error::Params（源 ArgumentException("元数据URL不能为空")）。
    /// 连接层失败同样重试（与 get_version_manifest 同策略，见 [`REQUEST_RETRIES`]）。
    async fn get_version_metadata(&self, url: &str) -> Result<CompleteVersionMetadata, Error> {
        if url.is_empty() {
            return Err(Error::Params {
                message: "元数据URL不能为空".to_string(),
                source: None,
            });
        }

        let body = get_json_with_retry(&self.http, url).await?;

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

/// GET + 连接层重试（issue #176 新增）：仅对**连接层错误**重试
/// [`REQUEST_RETRIES`] 次，退避 1s / 2s（与 completer.rs `download_file_with_retry`
/// 的 `1000*(retry+1)` 同节奏）。
///
/// HTTP 状态码错误不重试——4xx 重试无意义，且部分上游（如 CF 批量接口对
/// BadRequest）已有各自的跳过/降级语义，重试反而放大请求量。
async fn get_json_with_retry(http: &reqwest::Client, url: &str) -> Result<String, Error> {
    for attempt in 0..REQUEST_RETRIES {
        match get_json(http, url).await {
            Ok(body) => return Ok(body),
            Err(e) if is_connect_layer_error(&e) && attempt + 1 < REQUEST_RETRIES => {
                eprintln!(
                    "GET {url} 第 {} 次失败（{e}），{}ms 后重试",
                    attempt + 1,
                    1000 * (attempt as u64 + 1)
                );
                tokio::time::sleep(Duration::from_millis(1000 * (attempt as u64 + 1))).await;
            }
            Err(e) => return Err(e),
        }
    }
    // 最后一轮失败会走上面的 `Err(e) => return Err(e)`；循环穷尽仅当最后一轮
    // 命中重试分支（attempt+1 == REQUEST_RETRIES 不成立时不进该分支）——实际
    // 不可达，防御性兜底同 completer.rs 的 unreachable! 语义。
    unreachable!("重试循环内必然 return")
}

/// 网络错误映射（源：HttpRequestException → Error::Http，消息格式沿用既有惯例）
fn http_err(e: reqwest::Error) -> Error {
    Error::Http {
        message: format!("HTTP 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    }
}

/// 判定是否为连接层错误（可重试）：reqwest 的连接/请求发送/超时失败
/// （DNS、TLS、连接被拒、读超时等），对应日志里的
/// "error sending request for url (...)"。状态码错误（`get_json` 里手工构造、
/// 无 source）不算——4xx/5xx 重试无意义或应由调用方降级。
fn is_connect_layer_error(e: &Error) -> bool {
    let Error::Http {
        source: Some(s), ..
    } = e
    else {
        return false;
    };
    // http_err 只包一层 reqwest::Error，但为稳健起见沿 source 链向下探 3 层
    let mut cur: &(dyn std::error::Error + 'static) = s.as_ref();
    for _ in 0..3 {
        if let Some(r) = cur.downcast_ref::<reqwest::Error>() {
            return r.is_connect() || r.is_request() || r.is_timeout();
        }
        match cur.source() {
            Some(next) => cur = next,
            None => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 镜像 URL 形态与 mirror.rs 的 bug-for-bug 约定一致（meta 前缀整串拼接）。
    #[test]
    fn bmclapi_manifest_mirror_url_shape() {
        let mirror_url = format!("{BMCLAPI_META_PREFIX}{MANIFEST_URL}");
        assert_eq!(
            mirror_url,
            "https://bmclapi2.bangbang93.com/meta/https://launchermeta.mojang.com/mc/game/version_manifest.json"
        );
    }

    /// 状态码错误（无 source）不得触发重试——4xx/5xx 重试无意义且放大请求量。
    #[test]
    fn status_error_is_not_connect_layer() {
        let e = Error::Http {
            message: "Response status code does not indicate success: 502 (Bad Gateway)."
                .to_string(),
            status: None,
            source: None,
        };
        assert!(!is_connect_layer_error(&e));
    }

    /// 非网络类错误（Params 等）不得触发重试。
    #[test]
    fn params_error_is_not_connect_layer() {
        let e = Error::Params {
            message: "元数据URL不能为空".to_string(),
            source: None,
        };
        assert!(!is_connect_layer_error(&e));
    }

    /// 愚人节快照重命名逻辑在拆分后的 parse_manifest 中保持不变。
    #[test]
    fn parse_manifest_renames_april_fools_snapshot() {
        let body = r#"{"latest":{"release":"1.16.5","snapshot":"20w14infinite"},"versions":[
            {"id":"20w14infinite","type":"snapshot","url":"https://x","time":"2020-04-01T00:00:00+00:00","releaseTime":"2020-04-01T00:00:00+00:00"}
        ]}"#;
        let root = VersionManifestService::parse_manifest(body).expect("应解析成功");
        assert_eq!(root.versions[0].r#type, "april_fools");
    }
}
