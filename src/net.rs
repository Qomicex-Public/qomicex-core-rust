//! 核心内部网络配置（proxy + TLS 校验开关）。
//!
//! 启动器后端把已配好 proxy / ignore-TLS 的 `reqwest::Client` 经
//! `GameCoreBuilder::with_http_client` 注入核心，但核心内部多个服务会自建自己的
//! `reqwest` 客户端，并不接收注入的那一个。本模块提供统一的 `NetworkConfig` 与
//! 构建辅助，让这些内部客户端同样走代理 / 跳过证书校验。
//!
//! 设计（最小侵入 + 与既有 `InstallerBase::DEFAULT_USER_AGENT` 全局先例一致）：
//! - `build()` 在组装开始时把 `CoreOptions` 的 proxy/TLS 配置写入全局
//!   `NetworkConfig::set_global`；
//! - 各内部自建客户端的构造函数/工厂读取 `NetworkConfig::global()` 并应用，
//!   从而无需逐个改动构造签名或调用方。

use std::sync::OnceLock;

/// 网络配置：可选代理 URL + 是否禁用所有代理 + 是否跳过 TLS 证书校验。
///
/// `proxy_url` 支持 HTTP(S)/SOCKS5 代理（如 `http://127.0.0.1:7890`、
/// `socks5://127.0.0.1:1080`），`None` = 不使用自定义代理。
/// `no_proxy = true` → 禁用所有代理（含系统代理，等价 reqwest `ClientBuilder::no_proxy()`）。
/// `ignore_ssl_certs = true` → 关闭 TLS 校验（等同事先注入客户端的
/// `danger_accept_invalid_certs`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NetworkConfig {
    pub proxy_url: Option<String>,
    pub no_proxy: bool,
    pub ignore_ssl_certs: bool,
}

/// 全局网络配置（OnceLock：随最早一次 `set_global` 生效，与既有
/// `InstallerBase::DEFAULT_USER_AGENT` 同语调）。
static GLOBAL_CONFIG: OnceLock<NetworkConfig> = OnceLock::new();

impl NetworkConfig {
    /// 设置全局网络配置（幂等：仅首次调用生效，同默认 User-Agent 语义）。
    pub(crate) fn set_global(config: NetworkConfig) {
        let _ = GLOBAL_CONFIG.set(config);
    }

    /// 读取全局网络配置（未设置时返回默认值）。
    pub(crate) fn global() -> &'static NetworkConfig {
        GLOBAL_CONFIG.get_or_init(NetworkConfig::default)
    }

    /// 把代理 / 禁用代理 / TLS 校验开关应用到 reqwest builder。
    ///
    /// - `no_proxy` → `.no_proxy()`（禁用系统代理）。
    /// - `proxy_url` 为非法 URL 时静默跳过（`reqwest::Proxy::all` 返回 Err 时不应用，
    ///   保持 reqwest 标准语义）；SOCKS5 需 reqwest `socks` feature。
    /// - `ignore_ssl_certs` → `.danger_accept_invalid_certs(true)`。
    pub(crate) fn apply(&self, mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        if self.no_proxy {
            builder = builder.no_proxy();
        }
        if self.ignore_ssl_certs {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(url) = self.proxy_url.as_deref()
            && let Ok(proxy) = reqwest::Proxy::all(url)
        {
            builder = builder.proxy(proxy);
        }
        builder
    }
}

/// 构建核心内部 HTTP 客户端（带 User-Agent + 可选 proxy/TLS 配置）。
///
/// 由 `builder::build_http_client` 与其它需要显式传参的调用点使用；行为等价于
/// 原默认客户端，额外叠加代理与 TLS 校验开关。构建失败 panic
/// （等价"构建即失败"语义，同源 HttpClient 构造）。
pub(crate) fn build_http_client_ex(
    user_agent: &str,
    proxy_url: Option<&str>,
    no_proxy: bool,
    ignore_ssl_certs: bool,
) -> reqwest::Client {
    let config = NetworkConfig {
        proxy_url: proxy_url.map(str::to_string),
        no_proxy,
        ignore_ssl_certs,
    };
    config
        .apply(reqwest::Client::builder().user_agent(user_agent))
        .build()
        .unwrap_or_else(|e| panic!("构建 HTTP 客户端失败（UserAgent: {user_agent}）: {e}"))
}
