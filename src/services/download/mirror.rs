//! 默认下载源管理器（B6）
//!
//! 对应源文件：Services/DefaultDownloadSourceManager.cs
//! （实现 Public/Core/IDownloadSourceManager.cs → 本文件 impl api/download.rs 的 DownloadSourceManager trait）。
//!
//! 覆盖：可用源列表、镜像 URL 生成（BMCLAPI 替换规则）、源可用性测试、
//! 自定义源注册、首选源挑选。
//! 测试向量对应源项目 Qomicex.Core.AOT.Tests/UnitTests/DownloadSourceManagerTests.cs。

use std::time::Duration;

use async_trait::async_trait;

use crate::api::download::DownloadSourceManager;
use crate::error::Error;
use crate::models::download::{DownloadMirror, DownloadSource, DownloadSourceType, ResourceType};
use crate::net::NetworkConfig;

/// 源可用性测试超时（B6 设计值：源 HttpClient 默认 100s 超时过宽，
/// ping 测试取 10s，见 test_source 的差异说明）。
const TEST_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);

/// 默认下载源管理器（源：`internal class DefaultDownloadSourceManager`）。
///
/// 内置两个源（源构造器原样）：
/// - 官方源（Mojang官方，`https://launcher.mojang.com/`，优先级 100）
/// - BMCLAPI 镜像（BMCLAPI镜像，`https://bmclapi2.bangbang93.com/`，优先级 1，默认首选）
///
/// 首选 = 优先级最小者（C# 排序后取第一个）。
/// `add_custom_source` 在 trait 中为 `&mut self`（B3 定案，api/download.rs），
/// 内部可变性由调用方可变借用提供 → 直接持有 `Vec<DownloadSource>`，无需内部锁。
pub(crate) struct DefaultDownloadSourceManager {
    sources: Vec<DownloadSource>,
    http_client: reqwest::Client,
}

/// 无参构造器（源：`DefaultDownloadSourceManager()` 链式调用
/// `: this(DownloadMirror.BMCLAPI)` → 默认 BMCLAPI 首选）。
impl Default for DefaultDownloadSourceManager {
    fn default() -> Self {
        Self::new(DownloadMirror::Bmclapi)
    }
}

impl DefaultDownloadSourceManager {
    /// 按首选镜像创建管理器（源：`DefaultDownloadSourceManager(DownloadMirror preferredMirror)`）。
    ///
    /// `preferred_mirror == DownloadMirror::Official` 时把官方源优先级
    /// 提到当前最低优先级之下（官方源变为首选）；否则保持内置默认
    /// （BMCLAPI 优先级 1 已为最低，天然首选）。
    pub(crate) fn new(preferred_mirror: DownloadMirror) -> Self {
        // 内部自建客户端：应用全局 proxy/TLS 配置（启动器经 CoreOptions 注入）
        let http_client = NetworkConfig::global()
            .apply(reqwest::Client::builder())
            .build()
            .expect("构建下载源 HTTP 客户端失败");
        let sources = vec![
            DownloadSource {
                r#type: DownloadSourceType::Official,
                name: "Mojang官方".to_string(),
                base_url: "https://launcher.mojang.com/".to_string(),
                is_enabled: true,
                priority: 100,
                description: None,
            },
            DownloadSource {
                r#type: DownloadSourceType::Bmclapi,
                name: "BMCLAPI镜像".to_string(),
                base_url: "https://bmclapi2.bangbang93.com/".to_string(),
                is_enabled: true,
                priority: 1,
                description: None,
            },
        ];

        let mut manager = Self { sources, http_client };
        if preferred_mirror == DownloadMirror::Official {
            manager.set_preferred_source(DownloadSourceType::Official);
        }
        manager
    }

    /// 把指定类型源设为首选（源：`SetPreferredSource`，私有）。
    ///
    /// 语义逐条保留：
    /// 1. 按优先级升序找到该类型源；类型不存在 → 直接返回；
    /// 2. 计算当前全部源的最低优先级 `minPriority`（C# `ordered[0].Priority`）；
    /// 3. 若该源优先级 != minPriority → 其优先级改为 `minPriority - 1`，
    ///    成为唯一最低优先级（首选）。
    fn set_preferred_source(&mut self, source_type: DownloadSourceType) {
        let Some(index) = self.sources.iter().position(|s| s.r#type == source_type) else {
            return;
        };
        let min_priority = self.sources.iter().map(|s| s.priority).min().unwrap_or(0);
        if self.sources[index].priority != min_priority {
            self.sources[index].priority = min_priority - 1;
        }
    }

    /// BMCLAPI 镜像 URL 转换（源：`ConvertToMirrorUrl`，私有静态）。
    ///
    /// 替换规则逐条保留（一个字符不改；判定用 `Contains`，切分用
    /// C# `Split(marker)[^1]` 末段）：
    /// 1. 仅 `DownloadSourceType::Bmclapi` 源参与转换，其余类型 → None；
    /// 2. base = `BaseUrl.TrimEnd('/')`（去掉全部结尾 `/`）；
    /// 3. 原 URL 含 `"launcher.mojang.com/maven"` → `{base}/maven/{按"maven/"切分后的末段}`；
    /// 4. 原 URL 含 `"resources.download.minecraft.net"` → `{base}/assets/{按"assets/"切分后的末段}`；
    /// 5. 原 URL 含 `"piston-meta.mojang.com"` → `{base}/meta/{按"meta/"切分后的末段}`；
    /// 6. 以上均不匹配 → None（不生成镜像）。
    ///
    /// 边界行为（源语义，测试向量见 DownloadSourceManagerTests.cs）：
    /// 若切分标记不在原 URL 中（assets 哈希直链、piston-meta 无 `/meta/` 路径时），
    /// C# `Split` 返回整串原 URL 作为末段 → 结果形如 `{base}/assets/{整串原URL}`。
    /// 逐字复刻该行为，不做修正。
    fn convert_to_mirror_url(original_url: &str, source: &DownloadSource) -> Option<String> {
        if source.r#type != DownloadSourceType::Bmclapi {
            return None;
        }

        let base_url = source.base_url.trim_end_matches('/');

        if original_url.contains("launcher.mojang.com/maven") {
            let tail = original_url.rsplit_once("maven/").map(|(_, t)| t).unwrap_or(original_url);
            return Some(format!("{base_url}/maven/{tail}"));
        }
        if original_url.contains("resources.download.minecraft.net") {
            let tail = original_url.rsplit_once("assets/").map(|(_, t)| t).unwrap_or(original_url);
            return Some(format!("{base_url}/assets/{tail}"));
        }
        if original_url.contains("piston-meta.mojang.com") {
            let tail = original_url.rsplit_once("meta/").map(|(_, t)| t).unwrap_or(original_url);
            return Some(format!("{base_url}/meta/{tail}"));
        }

        None
    }
}

#[async_trait]
impl DownloadSourceManager for DefaultDownloadSourceManager {
    /// 获取可用下载源（源：`GetAvailableSources`，仅返回启用源）。
    ///
    /// `resource_type` 参数源实现未使用，原样忽略。
    fn get_available_sources(&self, _resource_type: ResourceType) -> Vec<DownloadSource> {
        self.sources.iter().filter(|s| s.is_enabled).cloned().collect()
    }

    /// 生成镜像 URL 列表（源：`GenerateMirrorUrls`，惰性枚举 → Vec）。
    ///
    /// 顺序：先原始 URL，再按优先级升序（C# `OrderBy(Priority)` 稳定排序）
    /// 遍历启用源，转换结果非 None 且 != 原始 URL 时追加。
    fn generate_mirror_urls(&self, original_url: &str, _resource_type: ResourceType) -> Vec<String> {
        let mut urls = vec![original_url.to_string()];

        let mut enabled: Vec<&DownloadSource> = self.sources.iter().filter(|s| s.is_enabled).collect();
        enabled.sort_by_key(|s| s.priority);

        for source in enabled {
            if let Some(mirror_url) = Self::convert_to_mirror_url(original_url, source) {
                if mirror_url != original_url {
                    urls.push(mirror_url);
                }
            }
        }

        urls
    }

    /// 测试下载源是否可用（源：`TestSourceAsync`）。
    ///
    /// 源语义：请求源 `BaseUrl` 根路径，2xx → 可用；任何异常（含超时）
    /// 一律吞掉返回不可用 → 本实现任何失败均返回 `Ok(false)`。
    ///
    /// 差异说明（B6 定案，任务指令）：源用 HEAD 请求（HttpMethod.Head），
    /// 本实现用 GET + `tokio::time::timeout`（10s）——部分 CDN/静态站点
    /// 对 HEAD 支持不佳，GET 更可靠；行为（true/false）语义不变。
    async fn test_source(&self, source: &DownloadSource) -> Result<bool, Error> {
        let test_url = format!("{}/", source.base_url.trim_end_matches('/'));
        match tokio::time::timeout(TEST_SOURCE_TIMEOUT, self.http_client.get(&test_url).send()).await {
            Ok(Ok(response)) => Ok(response.status().is_success()),
            Ok(Err(_)) | Err(_) => Ok(false),
        }
    }

    /// 注册自定义下载源（源：`AddCustomSource`）。
    ///
    /// 已存在同类型源 → 源抛 `InvalidOperationException("已存在类型为 ... 的下载源")`；
    /// trait 签名无 Result 错误通道（B3 定案 `&mut self` + `()`），
    /// 等义映射为 `panic!`（同一消息），调用方需自行保证类型唯一。
    fn add_custom_source(&mut self, source: DownloadSource) {
        if self.sources.iter().any(|s| s.r#type == source.r#type) {
            panic!("已存在类型为 {:?} 的下载源", source.r#type);
        }
        self.sources.push(source);
    }

    /// 获取首选下载源（源：`GetPreferredSource`，启用源中优先级最小者）。
    ///
    /// `MinBy` 平局取首个 → Rust `min_by_key` 同样返回首个（文档保证），语义一致。
    /// C# `MinBy` 对空序列抛异常；本实现返回 `Ok(None)`（源内置双源，实际不可达）。
    fn get_preferred_source(&self, _resource_type: ResourceType) -> Result<Option<DownloadSource>, Error> {
        Ok(self
            .sources
            .iter()
            .filter(|s| s.is_enabled)
            .min_by_key(|s| s.priority)
            .cloned())
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmclapi_maven_url_rewrite() {
        let mgr = DefaultDownloadSourceManager::default();
        let urls = mgr.generate_mirror_urls(
            "https://launcher.mojang.com/maven/org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar",
            ResourceType::Library,
        );
        // 行为：先原始 URL，再镜像 URL（P25 逐条保留源语义）
        assert_eq!(
            urls,
            vec![
                "https://launcher.mojang.com/maven/org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar",
                "https://bmclapi2.bangbang93.com/maven/org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar"
            ]
        );
    }

    #[test]
    fn bmclapi_assets_url_rewrite_bug_for_bug() {
        let mgr = DefaultDownloadSourceManager::default();
        let urls = mgr.generate_mirror_urls(
            "https://resources.download.minecraft.net/ab/abcdef0123456789",
            ResourceType::Asset,
        );
        // bug-for-bug：URL 无 "assets/" 标记时 C# Split 末段 = 整串原 URL
        assert_eq!(
            urls,
            vec![
                "https://resources.download.minecraft.net/ab/abcdef0123456789",
                "https://bmclapi2.bangbang93.com/assets/https://resources.download.minecraft.net/ab/abcdef0123456789"
            ]
        );
    }

    #[test]
    fn bmclapi_meta_url_rewrite_bug_for_bug() {
        let mgr = DefaultDownloadSourceManager::default();
        let urls = mgr.generate_mirror_urls(
            "https://piston-meta.mojang.com/mc/game/version_manifest.json",
            ResourceType::Library,
        );
        // bug-for-bug：URL 无 "meta/" 标记时同样整串拼接（源行为）
        assert_eq!(
            urls,
            vec![
                "https://piston-meta.mojang.com/mc/game/version_manifest.json",
                "https://bmclapi2.bangbang93.com/meta/https://piston-meta.mojang.com/mc/game/version_manifest.json"
            ]
        );
    }

    #[test]
    fn official_preferred_still_generates_mirrors() {
        let mgr = DefaultDownloadSourceManager::new(DownloadMirror::Official);
        let urls = mgr.generate_mirror_urls(
            "https://launcher.mojang.com/maven/org/x.jar",
            ResourceType::Library,
        );
        // Official 仅改变"首选源"，不剔除 BMCLAPI → 镜像仍追加（源语义）
        assert_eq!(
            urls,
            vec![
                "https://launcher.mojang.com/maven/org/x.jar",
                "https://bmclapi2.bangbang93.com/maven/org/x.jar"
            ]
        );
    }

    #[test]
    fn unmatched_url_no_mirror() {
        let mgr = DefaultDownloadSourceManager::default();
        let urls = mgr.generate_mirror_urls("https://example.com/other/file.jar", ResourceType::Library);
        assert_eq!(urls, vec!["https://example.com/other/file.jar"]);
    }
}

