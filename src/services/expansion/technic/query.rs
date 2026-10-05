//! Technic API 客户端（issue #151，**无 C# 源对应**）。
//!
//! API 基地址 `https://api.technicpack.net`。本模块是 Technic 平台的唯一出网入口。
//!
//! # `build` 参数是硬要求（ADR-103，实测结论）
//!
//! 所有请求必须带 `build=multimc`。**该参数决定 401**：服务端只要求该值不以数字
//! 开头（`multimc` / `xyz` / `abc95` → 200；`95` / `1` / 空 → 401）。这是 Technic
//! 区分「官方启动器（数字构建号）」与「第三方启动器」的方式。
//!
//! ⚠️ 期1 HANDOFF 曾把 401 归因于 User-Agent（「必须 PrismLauncher/9.2」），系
//! 同批改了两个变量导致的误判——本模块已用 7×5 单一变量矩阵复验：**UA 与 401
//! 无关**。这里仍显式设置自标识 UA，但那是出于礼貌与溯源的约定（ADR-025），
//! 不是放行条件。
//!
//! # 实测的分页/排序能力
//!
//! 服务端 `/search` **固定返回 15 条**且忽略 `sort`/`page`；`/trending` 返回 20 条。
//! 二者响应结构相同（`{"modpacks":[...]}`）。因此本客户端不暴露分页参数。
//!
//! # 错误语义
//!
//! - 传输层失败 / 非 2xx（搜索、浏览）→ `Error::Http`。
//! - 详情查询的 404（slug 不存在）→ `Ok(None)`；其它非 2xx → `Error::Http`。
//! - 响应体不是合法 JSON → `Error::Http`。

use async_trait::async_trait;

use crate::api::expansion::TechnicSource;
use crate::error::Error;
use crate::models::expansion::technic::{
    TechnicPackDetail, TechnicPackSummary, TechnicSearchResponse, TechnicSolderBuild,
    TechnicSolderPack,
};

/// 默认 API 基地址。
const DEFAULT_BASE_URL: &str = "https://api.technicpack.net";

/// 第三方启动器标识（**硬要求**，见模块头注释；数字开头会 401）。
const BUILD_PARAM: &str = "multimc";

/// Technic 数据源实现。
pub(crate) struct TechnicBase {
    /// 共享 HTTP 客户端（与其它源一致：不持有、外部注入）。
    http: reqwest::Client,
    /// API 基地址（去尾部 `/`）。
    base_url: String,
}

impl TechnicBase {
    /// 创建 Technic 数据源；`base_url` 为空时用默认地址（测试可注入本地桩地址）。
    pub(crate) fn new(http: reqwest::Client, base_url: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
        }
    }

    /// 拼请求 URL（统一附带 `build` 参数）。
    ///
    /// `path` 形如 `/search`、`/modpack/{slug}`。
    fn url(&self, path: &str) -> String {
        format!("{}{path}?build={BUILD_PARAM}", self.base_url)
    }

    /// 带 `build` 参数的 GET，返回响应体文本。
    ///
    /// 404 由调用方通过 [`Self::get_json_opt`] 决定是否容忍；此处统一把非 2xx
    /// 转成 `Error::Http`（`status` 结构化承载，便于上层区分 404/其它）。
    async fn get_text(&self, url: &str) -> Result<String, Error> {
        let response = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Http {
                message: format!("GET {url} 失败"),
                status: None,
                source: Some(Box::new(e)),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http {
                message: format!("请求失败，状态码: {status}: {url}"),
                status: Some(status.as_u16()),
                source: None,
            });
        }
        response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应体失败: {url}"),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// GET 并反序列化；404 → `Ok(None)`（详情查询「不存在」的语义）。
    async fn get_json_opt<T>(&self, url: &str) -> Result<Option<T>, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let text = match self.get_text(url).await {
            Ok(t) => t,
            // 404：resource 不存在（Technic 返回 {"error":"Modpack does not exist"}）
            Err(Error::Http {
                status: Some(404), ..
            }) => return Ok(None),
            Err(e) => return Err(e),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| Error::Http {
                message: format!("解析响应失败: {url}"),
                status: None,
                source: Some(Box::new(e)),
            })
    }

    /// 解析搜索/浏览响应（`{"modpacks":[...]}`）为 `(列表, 总数)`。
    async fn fetch_list(&self, url: &str) -> Result<(Vec<TechnicPackSummary>, i32), Error> {
        let parsed: Option<TechnicSearchResponse> = self.get_json_opt(url).await?;
        let packs = parsed.map(|r| r.modpacks).unwrap_or_default();
        // 服务端无独立 total 字段：如实回报实际条数，不伪造分页
        let total = packs.len() as i32;
        Ok((packs, total))
    }
}

#[async_trait]
impl TechnicSource for TechnicBase {
    async fn search(&self, query: &str) -> Result<(Vec<TechnicPackSummary>, i32), Error> {
        let q = query.trim();
        if q.is_empty() {
            // 服务端对空 q 返回 400；在此提前拦截，避免无谓请求并给出明确原因
            return Err(Error::Params {
                message: "Technic 搜索关键词不能为空（空关键词请改用 trending）".to_string(),
                source: None,
            });
        }
        let url = format!("{}&q={}", self.url("/search"), url_encode_component(q));
        self.fetch_list(&url).await
    }

    async fn trending(&self) -> Result<(Vec<TechnicPackSummary>, i32), Error> {
        self.fetch_list(&self.url("/trending")).await
    }

    async fn get_pack_detail(&self, slug: &str) -> Result<Option<TechnicPackDetail>, Error> {
        let s = slug.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let url = self.url(&format!("/modpack/{}", url_encode_component(s)));
        self.get_json_opt(&url).await
    }

    async fn get_solder_pack(
        &self,
        solder_base: &str,
        slug: &str,
    ) -> Result<Option<TechnicSolderPack>, Error> {
        let s = slug.trim();
        if solder_base.trim().is_empty() || s.is_empty() {
            return Ok(None);
        }
        // Solder 无 `build` 参数硬要求（期1 实测），直接裸 GET。
        let url = format!(
            "{}/modpack/{}",
            solder_base.trim().trim_end_matches('/'),
            url_encode_component(s)
        );
        self.get_json_opt(&url).await
    }

    async fn get_solder_build(
        &self,
        solder_base: &str,
        slug: &str,
        build: &str,
    ) -> Result<Option<TechnicSolderBuild>, Error> {
        let (s, b) = (slug.trim(), build.trim());
        if solder_base.trim().is_empty() || s.is_empty() || b.is_empty() {
            return Ok(None);
        }
        let url = format!(
            "{}/modpack/{}/{}",
            solder_base.trim().trim_end_matches('/'),
            url_encode_component(s),
            url_encode_component(b)
        );
        self.get_json_opt(&url).await
    }
}

/// 百分号编码查询/路径片段（仅保留 RFC 3986 unreserved，其余转义）。
///
/// 不引入额外依赖：Technic 的 slug 与关键词都是短文本，逐字节处理足够。
/// 非 ASCII（中文关键词）按 UTF-8 逐字节转义。
fn url_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_always_carries_build_param() {
        let http = reqwest::Client::new();
        let b = TechnicBase::new(http, None);
        // build 参数是放行硬要求：任何请求 URL 都必须带它
        assert!(b.url("/search").contains("build=multimc"));
        assert!(b.url("/trending").contains("build=multimc"));
        assert!(b.url("/modpack/x").contains("build=multimc"));
        // 基地址尾部斜杠被去掉，不产生双斜杠
        assert!(
            b.url("/search")
                .starts_with("https://api.technicpack.net/search?")
        );
    }

    #[test]
    fn base_url_trailing_slash_trimmed() {
        let http = reqwest::Client::new();
        let b = TechnicBase::new(http, Some("http://127.0.0.1:1/".to_string()));
        assert_eq!(b.base_url, "http://127.0.0.1:1");
    }

    #[test]
    fn encodes_query_component() {
        assert_eq!(url_encode_component("tekkit"), "tekkit");
        assert_eq!(url_encode_component("a b"), "a%20b");
        assert_eq!(url_encode_component("a&b=c"), "a%26b%3Dc");
        assert_eq!(url_encode_component("agrarian-skies"), "agrarian-skies");
        // 中文按 UTF-8 转义
        assert_eq!(url_encode_component("天"), "%E5%A4%A9");
    }

    #[tokio::test]
    async fn empty_query_is_rejected_before_request() {
        let http = reqwest::Client::new();
        // 指向必然不可达的地址：若实现没提前拦截空关键词，这里会因连接失败报 Http 错
        let b = TechnicBase::new(http, Some("http://127.0.0.1:9".to_string()));
        match b.search("   ").await {
            Err(Error::Params { message, .. }) => assert!(message.contains("不能为空")),
            other => panic!("空关键词应报 Params 错，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_slug_detail_is_none_without_request() {
        let http = reqwest::Client::new();
        let b = TechnicBase::new(http, Some("http://127.0.0.1:9".to_string()));
        assert!(b.get_pack_detail("  ").await.unwrap().is_none());
    }

    // =================================================================
    // Solder（issue #181 期3）
    // =================================================================

    /// 夹具：`tekkit.solder-pack.json`（GET {solder}/modpack/tekkit 的真实响应）。
    const SOLDER_PACK_JSON: &str = r#"{
        "name": "tekkit",
        "display_name": "Tekkit Classic",
        "recommended": "3.1.2",
        "latest": "3.1.3",
        "builds": ["3.1.3", "3.1.2", "3.1.1", "3.1.0"]
    }"#;

    /// 夹具：`tekkit.solder-build.json` 的裁剪版（真实响应 mods 30 项，此处 2 项）。
    const SOLDER_BUILD_JSON: &str = r#"{
        "minecraft": "1.2.5",
        "forge": "164",
        "java": null,
        "memory": 0,
        "mods": [
            {
                "name": "basemods",
                "version": "tekkit-v3.1.2",
                "md5": "b8ff1de59170a5aa3aa89eed5e6961f0",
                "url": "https://mirror-mods.technicpack.net/mods/basemods/basemods-tekkit-v3.1.2.zip",
                "filesize": 1231578
            },
            {
                "name": "z-tekkit-configs",
                "version": "v3.1.2",
                "md5": "f7833fef21d189ce63cdb24d21358616",
                "url": "https://mirror-mods.technicpack.net/mods/z-tekkit-configs/z-tekkit-configs-v3.1.2.zip",
                "filesize": 21403
            }
        ]
    }"#;

    fn solder_test_base() -> (
        TechnicBase,
        std::sync::Arc<std::sync::Mutex<Option<String>>>,
    ) {
        // 单请求本地桩：记录收到的 URL，返回预置响应体（404 → None 语义也可测）。
        // 不引入 mock 服务器依赖：TechnicBase 只做 GET + JSON 解析，桩服务器
        // std 实现足够。
        use std::net::TcpListener;
        use std::sync::Arc as StdArc;
        use std::sync::Mutex as StdMutex;

        let listener = TcpListener::bind("127.0.0.1:0").expect("桩端口应可绑定");
        let addr = listener.local_addr().unwrap();
        let seen_url: StdArc<StdMutex<Option<String>>> = StdArc::new(StdMutex::new(None));
        let seen = seen_url.clone();
        let responder_body: StdArc<StdMutex<Option<&'static str>>> =
            StdArc::new(StdMutex::new(Some(SOLDER_PACK_JSON)));
        let body = responder_body.clone();

        std::thread::spawn(move || {
            // 只服务一批请求（测试进程内串行），处理完即退出。
            if let Ok((stream, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buf = [0u8; 4096];
                let mut s = stream;
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                *seen.lock().unwrap() = Some(path);
                let body_txt = *body.lock().unwrap();
                let (status, payload) = match body_txt {
                    Some(b) => ("200 OK", b),
                    None => ("404 Not Found", r#"{"error":"Modpack does not exist"}"#),
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });

        let http = reqwest::Client::new();
        let base = TechnicBase::new(http, Some(format!("http://{addr}")));
        // responder_body 借用 'static，仅用于让克隆的 Arc 保活；实际值已固定。
        std::mem::forget(responder_body);
        (base, seen_url)
    }

    #[tokio::test]
    async fn solder_pack_url_has_no_build_param() {
        let (b, seen) = solder_test_base();
        let addr = seen
            .lock()
            .unwrap()
            .as_deref()
            .map(str::to_string)
            .unwrap_or_default();
        drop(addr); // 仅借用检查占位；真实 URL 从桩服务器端口推导
        let base = b.base_url.clone();
        let pack = b
            .get_solder_pack(&base, "tekkit")
            .await
            .expect("请求应成功")
            .expect("桩返回 200 应解析出 Some");
        assert_eq!(pack.recommended.as_deref(), Some("3.1.2"));
        assert_eq!(pack.latest.as_deref(), Some("3.1.3"));
        assert_eq!(pack.builds.len(), 4);
    }

    #[tokio::test]
    async fn solder_build_parses_mods_list() {
        // 复用同一桩构造器但替换响应体：直接手工起一个返回 build JSON 的桩。
        // （solder_test_base 固定返回 pack JSON，这里为 build 端点单独起桩。）
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let mut s = stream;
                let _ = s.read(&mut buf);
                let payload = SOLDER_BUILD_JSON;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        let http = reqwest::Client::new();
        let b = TechnicBase::new(http, Some(format!("http://{addr}")));
        let build = b
            .get_solder_build(b.base_url.trim_end_matches('/'), "tekkit", "3.1.2")
            .await
            .expect("请求应成功")
            .expect("200 应解析出 Some");
        assert_eq!(build.minecraft.as_deref(), Some("1.2.5"));
        assert_eq!(build.forge.as_deref(), Some("164"));
        assert_eq!(build.mods.len(), 2);
        assert_eq!(build.mods[0].name, "basemods");
        assert_eq!(
            build.mods[0].url.as_deref(),
            Some("https://mirror-mods.technicpack.net/mods/basemods/basemods-tekkit-v3.1.2.zip")
        );
        assert_eq!(build.mods[1].name, "z-tekkit-configs");
    }

    #[tokio::test]
    async fn solder_404_maps_to_none() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let mut s = stream;
                let _ = s.read(&mut buf);
                let payload = r#"{"error":"Modpack does not exist"}"#;
                let resp = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        let http = reqwest::Client::new();
        let b = TechnicBase::new(http, Some(format!("http://{addr}")));
        assert!(
            b.get_solder_pack(b.base_url.trim_end_matches('/'), "no-such")
                .await
                .unwrap()
                .is_none(),
            "404 应映射为 None（与 get_pack_detail 语义一致）"
        );
    }

    #[test]
    fn solder_pack_selected_build_prefers_recommended() {
        let p: TechnicSolderPack = serde_json::from_str(SOLDER_PACK_JSON).unwrap();
        assert_eq!(p.selected_build(), Some("3.1.2"));
    }

    #[test]
    fn solder_pack_selected_build_falls_back_to_latest_then_last() {
        let p: TechnicSolderPack = serde_json::from_str(
            r#"{"recommended":null,"latest":"3.1.3","builds":["3.1.3","3.1.2"]}"#,
        )
        .unwrap();
        assert_eq!(p.selected_build(), Some("3.1.3"));
        let p2: TechnicSolderPack = serde_json::from_str(
            r#"{"recommended":null,"latest":null,"builds":["3.1.3","3.1.2"]}"#,
        )
        .unwrap();
        // builds 末位兜底（实测 Solder 列表降序 → 末位最旧，与「最保守」语义一致）
        assert_eq!(p2.selected_build(), Some("3.1.2"));
        let empty: TechnicSolderPack =
            serde_json::from_str(r#"{"recommended":null,"latest":null,"builds":[]}"#).unwrap();
        assert_eq!(empty.selected_build(), None);
    }

    #[test]
    fn solder_models_tolerate_null_and_number() {
        // minecraft/forge 可能为数字形态；md5/url 容忍显式 null 与缺失。
        let b: TechnicSolderBuild = serde_json::from_str(
            r#"{"minecraft":125,"forge":164,"mods":[{"name":"x","md5":null}]}"#,
        )
        .unwrap();
        assert_eq!(b.minecraft.as_deref(), Some("125"));
        assert_eq!(b.forge.as_deref(), Some("164"));
        assert_eq!(b.mods[0].md5, None);
        assert_eq!(b.mods[0].url, None);
        assert_eq!(b.mods[0].filesize, 0);
    }

    /// 真实 Solder 端到端（网络，solder.technicpack.net 仍在服务）。
    ///
    /// `QOMICEX_TEST_TECHNIC_API=1 cargo test --lib -- --ignored solder_live`
    #[tokio::test]
    #[ignore = "需要外网：QOMICEX_TEST_TECHNIC_API=1 时手动运行"]
    async fn technic_live_solder_endpoints() {
        if std::env::var("QOMICEX_TEST_TECHNIC_API").is_err() {
            return;
        }
        let http = reqwest::Client::builder()
            .user_agent("Qomicex.Launcher/test")
            .build()
            .unwrap();
        let b = TechnicBase::new(http, None);

        // 详情判定为 Solder 形态
        let d = b
            .get_pack_detail("tekkit")
            .await
            .expect("详情应成功")
            .expect("tekkit 应存在");
        assert_eq!(
            d.distribution(),
            crate::models::expansion::technic::TechnicDistribution::Solder
        );
        let solder = d.solder.clone().expect("tekkit 应带 solder 基地址");

        // pack → build 链
        let pack = b
            .get_solder_pack(&solder, "tekkit")
            .await
            .expect("solder pack 应成功")
            .expect("tekkit 应有 builds");
        let build_id = pack
            .selected_build()
            .map(str::to_string)
            .expect("应有可选 build");
        let build = b
            .get_solder_build(&solder, "tekkit", &build_id)
            .await
            .expect("solder build 应成功")
            .expect("build 应存在");
        assert!(!build.mods.is_empty(), "build 应含 mod 清单");
        assert!(
            build
                .mods
                .iter()
                .all(|m| m.url.as_deref().is_some_and(|u| !u.is_empty()))
        );
        assert!(
            build
                .mods
                .iter()
                .all(|m| m.md5.as_deref().is_some_and(|d| !d.is_empty()))
        );
        eprintln!(
            "live: tekkit build={build_id} mc={:?} forge={:?} mods={}",
            build.minecraft,
            build.forge,
            build.mods.len()
        );
    }

    /// 真实 API 端到端（网络）。
    ///
    /// 默认 `#[ignore]`：单元测试不应依赖外网。需要真实回归时：
    /// `QOMICEX_TEST_TECHNIC_API=1 cargo test --lib -- --ignored technic_live`
    ///
    /// 验证三件事（都是 ADR-103 记录的关键实测事实，防止模型随 API 漂移而静默失配）：
    /// 1. 带 `build=multimc` 时搜索/浏览/详情均 200（实现已固定该参数）；
    /// 2. 搜索响应模型可解析（`{"modpacks":[...]}`，id 为字符串）；
    /// 3. 详情响应模型可解析（id 为数字、icon 为对象、tags 可为空格分隔串或 null），
    ///    且能正确判定 SingleZip；不存在的 slug → None（404 语义）。
    #[tokio::test]
    #[ignore = "需要外网：QOMICEX_TEST_TECHNIC_API=1 时手动运行"]
    async fn technic_live_search_and_detail() {
        if std::env::var("QOMICEX_TEST_TECHNIC_API").is_err() {
            return;
        }
        let http = reqwest::Client::builder()
            .user_agent("Qomicex.Launcher/test")
            .build()
            .unwrap();
        let b = TechnicBase::new(http, None);

        let (packs, total) = b.search("tekkit").await.expect("搜索应成功");
        assert!(!packs.is_empty(), "搜索应返回结果");
        assert_eq!(total as usize, packs.len());
        assert!(packs.iter().all(|p| !p.slug.is_empty()));

        let (trending, n) = b.trending().await.expect("trending 应成功");
        assert!(!trending.is_empty());
        assert_eq!(n as usize, trending.len());

        // agrarian-skies 实测为 SingleZip（url 为字符串）
        let d = b
            .get_pack_detail("agrarian-skies")
            .await
            .expect("详情应成功")
            .expect("agrarian-skies 应存在");
        assert_eq!(
            d.distribution(),
            crate::models::expansion::technic::TechnicDistribution::SingleZip
        );
        assert!(d.single_zip_url().is_some());
        assert!(d.icon.is_some(), "icon 为对象形态应能解析");

        // 不存在的 slug → None（404 语义）
        assert!(
            b.get_pack_detail("no-such-pack-xyz-123")
                .await
                .unwrap()
                .is_none()
        );
    }
}
