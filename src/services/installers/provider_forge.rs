//! Forge 系版本查询（P61）：InstallerProvider.cs 的 Forge/NeoForge/Cleanroom 部分
//!
//! 对应源文件：Qomicex.Core.AOT/Services/InstallerProvider.cs（1106 行）中的以下方法：
//! - GetCleanroomVersions（行 508-551）
//! - GetNeoForgeFromOfficialApi / GetNeoForgeFromBmclApi / ParseNeoForgeMinecraftVersion（行 698-849）
//! - GetForgeVersions / GetForgeVersionsFromBmclApi / GetForgeVersionsFromOfficialHtml /
//!   ParseForgeVersions / CleanDownloadUrl / GetForgeDownloadUrl / IsRecommendedVersion /
//!   GetCacheFilePath（行 853-1104）
//!
//! 设计决策（详见 b13-logs/p61-provider-forge.md）：
//! - 本文件为模块级 pub(crate) 函数（协同契约：P60 provider.rs 经
//!   `super::provider_forge::*` 调用；本文件不定义 struct InstallerProvider）；
//! - 源实例字段 `_http`/`_mirror` → 显式参数 `http`/`mirror`；源未使用 `_mirror` 的
//!   方法（Cleanroom/NeoForge/官方 HTML）参数命名为 `_mirror`（仅对齐 P60 统一调用形态）；
//! - 源 catch-all（返回部分结果/空列表）→ 私有 inner 函数返回 Err，公开包装函数
//!   eprintln!（对应 Trace.WriteLine）后返回 Ok(空/部分)；错误映射：网络/状态码/
//!   JSON 解析 → Error::Http（error.rs 注明 Http 含义含源 JsonException 语义）；
//! - 源 `DateTimeOffset.MinValue` → 常量 `MIN_RELEASE_TIME`（System.Text.Json
//!   round-trip 文本 "0001-01-01T00:00:00+00:00"，B1 "String 原始文本保真"决策）；
//! - 源 `Uri.EscapeDataString` → 私有 `escape_data_string`（RFC 3986 非保留字符集 +
//!   大写十六进制；无 percent-encoding crate，禁止改 Cargo.toml）；
//! - 源 `WebUtility.UrlDecode` → 私有 `web_url_decode`（%XX 十六进制 + `+`→空格）；
//! - ⚠️ UNMAPPED U1：ParseForgeVersions 的版本正则含 lookbehind/lookahead，Rust regex
//!   crate 不支持环视 → 改写为消费式 + 手工后置断言（等价性论证见日志 U1）；
//! - ⚠️ UNMAPPED U2：Regex.Escape 用 regex::escape 近似（后者多转义 `-`/`~` 等，
//!   对版本串匹配语义等价，见日志 U2）；
//! - ⚠️ UNMAPPED U3：`System.Version.TryParse` → 私有 `version_try_parse` 近似（见日志 U3）；
//! - ⚠️ UNMAPPED U4：`DateTimeOffset.TryParse(modified)` 门控用 chrono rfc3339 近似
//!   （C# TryParse 更宽松；解析成功仍存原始文本，见日志 U4）。
//!
//! ## 有意偏离源逻辑（issue #176 修复，勿当作移植误差回退）
//!
//! - `GetForgeVersions` 的 Official 分支：源在「HTML 抓取失败/解析为空」时直接返回空；
//!   本实现改为 **Maven 元数据优先 → HTML 末位回退 → BMCLAPI 兜底** 三级链。
//!   起因：用户下载源为「官方源」时，HTML 链路一旦失败，前端表现为「暂无可加载器版本，
//!   无法下载」、整合包安装报「找不到 forge <版本> 的安装器」。（#176 实测日志与截图）
//! - 缓存读取：源「命中缓存即无条件采信解析结果」；本实现要求**解析非空**才采信，
//!   否则一份坏缓存会在 24h 内持续续命（回归用例见 tests::cache_with_unparseable_content_is_rejected）。
//! - 回退 BMCLAPI 时必须显式传 `DownloadMirror::Bmclapi`：该分支用 mirror 拼安装器直链，
//!   透传 Official 会拼出 build 号当版本号的坏链（列表非空但下载必败）。

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{Map, Value};

use crate::error::Error;
use crate::models::download::DownloadMirror;
use crate::models::installer::{ModLoaderResult, ModLoaderType};

/// 源 `DateTimeOffset.MinValue` 的字符串化文本（System.Text.Json round-trip：
/// "0001-01-01T00:00:00+00:00"）。
const MIN_RELEASE_TIME: &str = "0001-01-01T00:00:00+00:00";

/// Forge Maven 元数据（issue #176 新增主路径）。
///
/// 取代对 `files.minecraftforge.net` 下载页 HTML 表格的抓取：同一份版本信息，
/// XML 结构化（无渲染/无 adfoc 跳转/无表格 class 依赖），体积约为 HTML 的 1/10
/// （实测 211KB vs 2.27MB），且 `1.12.2-*` 条目与 HTML 分页逐条一致（实测各 355 条）。
const FORGE_MAVEN_METADATA_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";

/// Forge 官方 Maven 仓库该 artifact 的根地址（拼安装器 jar 直链）。
const FORGE_MAVEN_BASE_URL: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";

/// Forge 推荐版本清单（约 4KB）。`maven-metadata.xml` 不含 promo 标记，推荐状态
/// 由此补齐，取代「在 HTML 行里匹配 `promo-latest`/`promo-recommended` class」。
const FORGE_PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";

/// Forge 版本信息缓存有效期（小时）。元数据 / HTML / 推荐清单三种缓存共用。
const FORGE_CACHE_EXPIRY_HOURS: u64 = 24;

/// 获取 Cleanroom 版本列表（源：`GetCleanroomVersions(string minecraftVersion)`）。
///
/// - 仅支持 MC 1.12.2（源 `string.Equals(..., "1.12.2", OrdinalIgnoreCase)`），其余返回空列表；
/// - GET `https://api.github.com/repos/CleanroomMC/Cleanroom/releases`（GitHub Releases API），
///   逐条 release：`tag_name` 含 "alpha"（忽略大小写）→ 非推荐（beta）；
///   版本串取 `tag_name` 中首个 `-` 之前的部分，须通过 `System.Version.TryParse`
///   （近似 `version_try_parse`）才收录；
/// - 下载 URL = `https://github.com/CleanroomMC/Cleanroom/releases/download/{tagName}/cleanroom-{tagName}-installer.jar`；
/// - 结果经 `SortAndDeduplicate`（按版本去重 + VersionComparer 降序）。
///
/// ⚠️ 源方法未使用 `_mirror`，参数 `_mirror` 仅为对齐 P60 协同契约统一调用形态。
/// 源 catch-all → 失败记日志并返回空列表。
pub(crate) async fn get_cleanroom_versions(
    http: &reqwest::Client,
    _mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    if !mc_version.eq_ignore_ascii_case("1.12.2") {
        return Ok(Vec::new());
    }
    match cleanroom_releases_inner(http).await {
        Ok(versions) => Ok(versions),
        Err(e) => {
            eprintln!("Cleanroom 版本获取失败: {e}");
            Ok(Vec::new())
        }
    }
}

/// 从 NeoForge 官方 Maven API 获取版本列表（源：`GetNeoForgeFromOfficialApi`）。
///
/// - 并行请求（源 `Task.WhenAll`，Rust `tokio::join!`）：
///   - 旧版（Minecraft 1.20.1 Forge 系）：
///     `https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge`
///   - 新版：`https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge`
/// - 仅当请求版本匹配 1.20.1（`MatchesMinecraftVersion`）时收录旧版列表，游戏版本固定
///   "1.20.1"，下载 URL = `https://maven.neoforged.net/releases/net/neoforged/forge/{ver}/forge-{ver}-installer.jar`；
/// - 新版列表逐条：`ParseNeoForgeMinecraftVersion` 解析 MC 版本，空 → 跳过；请求版本非空
///   且不匹配 → 跳过；下载 URL =
///   `https://maven.neoforged.net/releases/net/neoforged/neoforge/{ver}/neoforge-{ver}-installer.jar`；
/// - 推荐判定：`!ver.Contains("beta", OrdinalIgnoreCase)`；
/// - 结果 GroupBy(Version).First + VersionComparer 降序。
///
/// ⚠️ 源方法未使用 `_mirror`，参数 `_mirror` 仅为对齐 P60 协同契约统一调用形态。
pub(crate) async fn get_neoforge_from_official_api(
    http: &reqwest::Client,
    _mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    match neoforge_official_api_inner(http, mc_version).await {
        Ok(versions) => Ok(versions),
        Err(e) => {
            eprintln!("NeoForge 版本获取失败: {e}");
            Ok(Vec::new())
        }
    }
}

/// 从 BMCLAPI 获取 NeoForge 版本列表（源：`GetNeoForgeFromBmclApi`）。
///
/// - 空 MC 版本 → 空列表；GET
///   `https://bmclapi2.bangbang93.com/neoforge/list/{Uri.EscapeDataString(mcVersion)}`；
/// - 逐条：`version`/`mcversion` 任一为空 → 跳过；下载 URL =
///   `https://bmclapi2.bangbang93.com/neoforge/version/{EscapeDataString(version)}/download/installer.jar`；
/// - 推荐判定：`!version.Contains("-beta") && !version.Contains("-alpha")`（源区分大小写）；
/// - 结果 GroupBy(Version).First + VersionComparer 降序。
///
/// ⚠️ 源方法未使用 `_mirror`，参数 `_mirror` 仅为对齐 P60 协同契约统一调用形态。
pub(crate) async fn get_neoforge_from_bmcl_api(
    http: &reqwest::Client,
    _mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    match neoforge_bmcl_api_inner(http, mc_version).await {
        Ok(versions) => Ok(versions),
        Err(e) => {
            eprintln!("NeoForge BMCLAPI 版本获取失败: {e}");
            Ok(Vec::new())
        }
    }
}

/// 解析 NeoForge 版本号对应的 Minecraft 版本（源：`ParseNeoForgeMinecraftVersion`）。
///
/// 逐字保留源逻辑：首个 `.` 与第二个 `.` 缺失 → 空；主版本号 >= 22（如 20.4.80 → 1.20.4
/// 时代的命名）→ 截取到第二个 `.`；主版本号 == 0 → 截取两个点之间；其余按
/// `1.{major}`（minor == 0）或 `1.{major}.{minor}` 拼装。解析失败记日志并返回空
/// （源 catch → Trace + string.Empty）。
pub(crate) fn parse_neoforge_minecraft_version(neo_forge_version: &str) -> String {
    let first_dot = match neo_forge_version.find('.') {
        Some(idx) => idx,
        None => return String::new(),
    };
    let second_dot = match neo_forge_version[first_dot + 1..].find('.') {
        Some(rel) => first_dot + 1 + rel,
        None => return String::new(),
    };
    let major_version = match neo_forge_version[..first_dot].parse::<i32>() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("解析 NeoForge 版本号失败 {neo_forge_version}: {e}");
            return String::new();
        }
    };
    if major_version >= 22 {
        return neo_forge_version[..second_dot].to_string();
    }
    if major_version == 0 {
        return neo_forge_version[first_dot + 1..second_dot].to_string();
    }
    let minor_version = match neo_forge_version[first_dot + 1..second_dot].parse::<i32>() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("解析 NeoForge 版本号失败 {neo_forge_version}: {e}");
            return String::new();
        }
    };
    if minor_version == 0 {
        format!("1.{major_version}")
    } else {
        format!("1.{major_version}.{minor_version}")
    }
}

/// 获取 Forge 版本列表（源：`GetForgeVersions(string minecraftVersion)`）。
///
/// 按下载源分发（源 `_mirror == DownloadMirror.BMCLAPI ? ... : ...`）：
/// - BMCLAPI → [`get_forge_versions_from_bmcl_api`]；
/// - Official → [`get_forge_versions_from_official`]（Maven 元数据优先 → HTML 末位回退），
///   拿到空列表时再回退 BMCLAPI。
///
/// ⚠️ 与源的差异（issue #176）：源在 Official 分支失败即返回空；此处增加了两级回退，
/// 因为「拿不到版本列表」在用户侧等于「无法安装该加载器」，不能由单一上游决定。
pub(crate) async fn get_forge_versions(
    http: &reqwest::Client,
    mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    match mirror {
        DownloadMirror::Bmclapi => get_forge_versions_from_bmcl_api(http, mirror, mc_version).await,
        DownloadMirror::Official => {
            // issue #176：官方源（Maven 元数据 → HTML 末位回退）**任一环节**失败或拿到空
            // 列表，一律回退 BMCLAPI。此前无回退——官方抓取一旦失败整链就返回空，
            // 用户下载源为「官方源」时表现为前端「暂无可加载器版本，无法下载」与整合包
            // 「找不到 forge <版本> 的安装器」（#176 实测）。此处与 get_neoforge_versions
            // 的既有回退策略对齐。
            //
            // 注意用 match 吞掉 Err 而不是 `?`：官方链路自身的错误（含缓存元数据读取
            // 失败等）同样不得让整链失败，否则回退形同虚设。
            let official = match get_forge_versions_from_official(http, mirror, mc_version).await {
                Ok(versions) => versions,
                Err(e) => {
                    eprintln!("Forge 官方源获取失败: {e}");
                    Vec::new()
                }
            };
            if !official.is_empty() {
                return Ok(official);
            }
            eprintln!("Forge 官方源未返回任何版本，回退 BMCLAPI");
            // ⚠️ 必须显式传 Bmclapi：BMCLAPI 分支会用 mirror 拼安装器直链
            // （get_forge_download_url）。若透传调用方的 Official，会拼出
            // `maven.minecraftforge.net/.../forge-{mc}-{build}/...`（build 号当版本号，
            // 必然 404）——列表非空但下载必败，比不回退更糟。实测：故障注入验证时
            // 曾得到 `.../forge-1.12.2-2860/...` 这样的坏链。
            get_forge_versions_from_bmcl_api(http, DownloadMirror::Bmclapi, mc_version).await
        }
    }
}

/// 官方源（非 BMCLAPI）的 Forge 版本获取：**Maven 元数据优先，HTML 抓取末位回退**。
///
/// issue #176：HTML 下载页表格抓取是本链路最不稳定的一环（依赖表格 class、
/// 单页 2.27MB、行内还混着 adfoc.us 跳转链），而同一份版本信息在
/// `maven-metadata.xml` 里是结构化且体积约 1/10 的。故主路径换成元数据，
/// HTML 仅在其拿到空列表时才尝试（保留旧行为作为兜底，不直接删除）。
async fn get_forge_versions_from_official(
    http: &reqwest::Client,
    mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    let from_metadata = get_forge_versions_from_maven_metadata(http, mirror, mc_version).await?;
    if !from_metadata.is_empty() {
        return Ok(from_metadata);
    }
    eprintln!("Forge Maven 元数据未返回版本，回退官方 HTML 抓取");
    get_forge_versions_from_official_html(http, mirror, mc_version).await
}

/// 从 BMCLAPI JSON 获取 Forge 版本列表（源：`GetForgeVersionsFromBmclApi`）。
///
/// - GET `https://bmclapi2.bangbang93.com/forge/minecraft/{EscapeDataString(mcVersion)}`；
///   源用 `if (response.IsSuccessStatusCode)`（非 2xx 静默跳过，不记日志）；
/// - 逐条：`mcversion` 与请求版本忽略大小写不相等 → 跳过；`files` 数组中首个
///   `category` == "installer"（忽略大小写）的文件作为 installer，缺失 → 跳过；
/// - 下载 URL = `GetForgeDownloadUrl(mcVersion, build)`（本路径恒为 BMCLAPI 分支）；
/// - 推荐判定：`IsRecommendedVersion(build, 已收录列表)`（build 号须大于列表中
///   各版本最后一段数字，逐字见 `is_recommended_version`）；
/// - 发布时间：`modified` 字段存在且 `DateTimeOffset.TryParse` 通过 → 原始文本；
///   否则 MinValue。
///
/// ⚠️ UNMAPPED U5：源循环内 `files` 非数组等行内异常会抛到外层 catch 返回*部分*
/// 已收录结果；Rust 统一为外层 catch 返回空列表（此类异常实际不可达——行内仅
/// JsonNode 索引访问，异常仅能来自 `AsArray()` 类型断言）。
pub(crate) async fn get_forge_versions_from_bmcl_api(
    http: &reqwest::Client,
    mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    match forge_versions_from_bmcl_api_inner(http, mirror, mc_version).await {
        Ok(versions) => Ok(versions),
        Err(e) => {
            eprintln!("BMCLAPI JSON 获取 Forge 版本失败: {e}");
            Ok(Vec::new())
        }
    }
}

/// 官方 Maven 元数据的 Forge 版本获取（issue #176 主路径）。
///
/// - 缓存：`%TEMP%/ForgeVersionCache/{mc}_forge_metadata.xml`，24h 内命中且**解析非空**
///   才直接采用（空结果视为缓存不可用，继续走网络，防坏缓存续命）；
/// - 网络：GET [`FORGE_MAVEN_METADATA_URL`]，非 2xx / 读取失败 → Err（由本函数吞为
///   空列表，交由 [`get_forge_versions_from_official`] 继续回退）；
/// - 解析见 [`parse_forge_metadata_versions`]；推荐标记由 [`fetch_forge_promotions`] 补齐。
pub(crate) async fn get_forge_versions_from_maven_metadata(
    http: &reqwest::Client,
    _mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    match forge_versions_from_maven_metadata_inner(http, mc_version).await {
        Ok(versions) => Ok(versions),
        Err(e) => {
            eprintln!("Forge Maven 元数据获取失败: {e}");
            Ok(Vec::new())
        }
    }
}

/// 从 `maven-metadata.xml` 正文提取指定 MC 版本的 Forge 版本列表（降序）。
///
/// 逐条 `<version>` 文本形如 `{mc_version}-{forge_version}`（如 `1.12.2-14.23.5.2860`），
/// 前缀不匹配的丢弃（严格前缀 + `-` 分隔，故 `1.12.2` 不会误吃 `1.12.20-*`）；
/// 返回值取前缀之后的 Forge 版本号，`url` 按 Maven artifact 规则拼接。
///
/// 实测与旧 HTML 分页的一致性：1.12.2 / 1.16.5 / 1.20.1 / 1.21 / 1.6.4 逐条相同；
/// 1.7.10 处元数据为超集（多出 `1.7.10_pre4-*` 预发布，旧 HTML 需 `index_1.7.10_pre4`
/// 另一页才看得到）。
///
/// 安装器命名统一为 `forge-{artifactId}-installer.jar`，对 `1.7.10-10.13.4.1614-1.7.10`、
/// `1.7.10_pre4-10.12.2.1149-prerelease` 等古怪 artifact 同样成立（已 HEAD 实测 200）。
pub(crate) fn parse_forge_metadata_versions(xml: &str, mc_version: &str) -> Vec<ModLoaderResult> {
    let prefix = format!("{mc_version}-");
    let mut versions: Vec<String> = Vec::new();

    for cap in metadata_version_regex().captures_iter(xml) {
        let artifact = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let Some(forge_version) = artifact.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if forge_version.is_empty() || versions.iter().any(|v| v == forge_version) {
            continue;
        }
        versions.push(forge_version.to_string());
    }

    let mut results: Vec<ModLoaderResult> = versions
        .into_iter()
        .map(|forge_version| {
            let artifact = format!("{prefix}{forge_version}");
            ModLoaderResult {
                r#type: ModLoaderType::Forge,
                version: forge_version,
                game_version: mc_version.to_string(),
                url: format!("{FORGE_MAVEN_BASE_URL}/{artifact}/forge-{artifact}-installer.jar"),
                sha1: String::new(),
                is_recommand: false,
                release_time: MIN_RELEASE_TIME.to_string(),
            }
        })
        .collect();

    // 与 HTML 路径同序（VersionSortInteger 降序），保持前端「最新版」展示一致。
    results.sort_by(|a, b| version_sort_integer(&b.version, &a.version).cmp(&0));
    results
}

/// 解析 `promotions_slim.json`，返回指定 MC 版本的 latest + recommended 版本号。
///
/// 结构：`{"homepage":"...","promos":{"1.12.2-latest":"14.23.5.2864",
/// "1.12.2-recommended":"14.23.5.2859",...}}`。
/// 非 JSON / 缺 `promos` / 值非字符串 → None（调用方据此保留默认标记，不阻断版本列表）；
/// 值重复（latest == recommended）只保留一份。
pub(crate) fn parse_forge_promotions(json: &str, mc_version: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(json).ok()?;
    let promos = value.get("promos")?.as_object()?;

    let mut out: Vec<String> = Vec::new();
    for key in [
        format!("{mc_version}-latest"),
        format!("{mc_version}-recommended"),
    ] {
        if let Some(Value::String(v)) = promos.get(key.as_str())
            && !v.trim().is_empty()
            && !out.iter().any(|x| x == v)
        {
            out.push(v.clone());
        }
    }
    // 契约统一：拿不到该 MC 的推荐信息一律 None（调用方据此保留默认标记，
    // 也令 `read_usable_cached_versions` 把空的推荐缓存判为不可用）。
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// 拉取并解析推荐版本清单；失败返回 None（调用方保留默认标记）。带 24h 缓存。
async fn fetch_forge_promotions(http: &reqwest::Client, mc_version: &str) -> Option<Vec<String>> {
    let cache_path = get_promotions_cache_file_path(mc_version);
    if let Some(promos) = read_usable_cached_versions(&cache_path, FORGE_CACHE_EXPIRY_HOURS, |j| {
        parse_forge_promotions(j, mc_version).unwrap_or_default()
    }) {
        return Some(promos);
    }

    let response = match http.get(FORGE_PROMOTIONS_URL).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Forge 推荐版本清单请求失败: {e}（推荐标记回退默认值）");
            return None;
        }
    };
    if !response.status().is_success() {
        eprintln!(
            "Forge 推荐版本清单请求失败: {}（推荐标记回退默认值）",
            response.status()
        );
        return None;
    }
    let body = match response.text().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Forge 推荐版本清单读取失败: {e}（推荐标记回退默认值）");
            return None;
        }
    };
    write_cache(&cache_path, &body);
    parse_forge_promotions(&body, mc_version)
}

/// 用推荐版本清单给列表打 `is_recommand`（清单拿不到则原样返回）。
async fn with_promotions(
    http: &reqwest::Client,
    mc_version: &str,
    versions: Vec<ModLoaderResult>,
) -> Vec<ModLoaderResult> {
    let Some(promos) = fetch_forge_promotions(http, mc_version).await else {
        return versions;
    };
    versions
        .into_iter()
        .map(|mut v| {
            v.is_recommand = promos.iter().any(|p| p.eq_ignore_ascii_case(&v.version));
            v
        })
        .collect()
}

/// 元数据主流程：缓存 → 网络 → 解析 → 补推荐标记。
async fn forge_versions_from_maven_metadata_inner(
    http: &reqwest::Client,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    let cache_path = get_metadata_cache_file_path(mc_version);

    if let Some(parsed) = read_usable_cached_versions(&cache_path, FORGE_CACHE_EXPIRY_HOURS, |t| {
        parse_forge_metadata_versions(t, mc_version)
    }) {
        eprintln!("Forge 元数据缓存命中: {} 个版本", parsed.len());
        return Ok(with_promotions(http, mc_version, parsed).await);
    }

    let response = http
        .get(FORGE_MAVEN_METADATA_URL)
        .send()
        .await
        .map_err(|e| Error::Http {
            message: format!("Forge Maven 元数据请求失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
    let response = response.error_for_status().map_err(|e| Error::Http {
        message: format!("Forge Maven 元数据请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let body = response.text().await.map_err(|e| Error::Http {
        message: format!("Forge Maven 元数据响应读取失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;

    write_cache(&cache_path, &body);

    let parsed = parse_forge_metadata_versions(&body, mc_version);
    if parsed.is_empty() {
        eprintln!("Forge 元数据中未找到 {mc_version} 的版本");
        return Ok(Vec::new());
    }
    eprintln!("Forge 元数据解析到 {} 个版本", parsed.len());
    Ok(with_promotions(http, mc_version, parsed).await)
}

/// 读 24h 内的缓存文本；文件缺失 / 超期 / 读取失败 → None（调用方走网络）。
fn read_fresh_cache(cache_path: &str, expiry_hours: u64) -> Option<String> {
    if !Path::new(cache_path).is_file() {
        return None;
    }
    let modified = std::fs::metadata(cache_path)
        .and_then(|m| m.modified())
        .ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    if age >= Duration::from_secs(expiry_hours * 3600) {
        return None;
    }
    std::fs::read_to_string(cache_path).ok()
}

/// 读取有效期内缓存并解析，**解析结果为空则视为缓存不可用**（issue #176）。
///
/// 修复前两条路径都是「命中缓存即无条件采信解析结果」：一份残缺 / 被上游改版 / 中断
/// 写入的缓存会在 24h 内持续续命，把用户锁死在「无可用加载器版本」。抽成独立函数后
/// 该约束可被单测直接覆盖（见本文件 tests 模块 `cache_*` 用例）。
fn read_usable_cached_versions<T>(
    cache_path: &str,
    expiry_hours: u64,
    parse: impl Fn(&str) -> Vec<T>,
) -> Option<Vec<T>> {
    let raw = read_fresh_cache(cache_path, expiry_hours)?;
    let parsed = parse(&raw);
    if parsed.is_empty() {
        return None;
    }
    Some(parsed)
}

/// 写缓存（目录不存在则创建）；失败仅告警，不阻断主流程。
fn write_cache(cache_path: &str, content: &str) {
    let write = (|| -> std::io::Result<()> {
        if let Some(dir) = Path::new(cache_path).parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(cache_path, content)
    })();
    match write {
        Ok(()) => eprintln!("已缓存到{cache_path}"),
        Err(e) => eprintln!("缓存写入失败: {e}"),
    }
}

/// 从 files.minecraftforge.net 官方 HTML 获取 Forge 版本列表（源：
/// `GetForgeVersionsFromOfficialHtml`）。
///
/// - 缓存路径：`GetCacheFilePath(mcVersion.Replace('-', '_'))`（`%TEMP%/ForgeVersionCache`）；
/// - 缓存命中：文件存在且最后写入时间距今 < 24 小时 → 直接读缓存解析
///   （读取/解析失败 → 日志"使用缓存失败…将重新获取"继续走网络）；缓存文件时间
///   元数据读取失败 → 传播 Err（源 File.GetLastWriteTime 异常同层无 catch）；
/// - 网络源：`https://files.minecraftforge.net/net/minecraftforge/forge/index_{forgeMcVersion}.html`
///   （源为单元素列表，保留循环结构）；非 2xx → 日志后继续下一源；响应字节按
///   UTF-8 解码（源 `Encoding.UTF8.GetString`，无效字节 → U+FFFD，同
///   `String::from_utf8_lossy`）；
/// - 下载成功即写缓存（`%TEMP%/ForgeVersionCache` 目录不存在 → 创建；失败仅日志），
///   解析结果非空 → 直接返回；
/// - 全部源失败 → 回退读取过期缓存（失败仅日志）→ 空列表。
pub(crate) async fn get_forge_versions_from_official_html(
    http: &reqwest::Client,
    _mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    // 源：`minecraftVersion.Replace('-', '_')`
    let forge_mc_version = mc_version.replace('-', "_");
    let cache_file_path = get_cache_file_path(&forge_mc_version);

    // 源：`File.Exists(cacheFilePath) && (DateTime.Now - File.GetLastWriteTime(...)).TotalHours < 24`
    // issue #176：命中缓存还须**解析非空**才采用——否则一份坏 HTML（残缺页 / 上游
    // 改版 / 中断写入）会在 24h 内持续续命，把用户锁死在「无可用版本」。
    if let Some(parsed) =
        read_usable_cached_versions(&cache_file_path, FORGE_CACHE_EXPIRY_HOURS, |html| {
            parse_forge_versions(mc_version, &forge_mc_version, html)
        })
    {
        return Ok(parsed);
    }

    // 源 sourceUrls 列表（当前仅一个官方 URL，保留循环结构）
    let source_urls = [format!(
        "https://files.minecraftforge.net/net/minecraftforge/forge/index_{forge_mc_version}.html"
    )];

    for url in source_urls {
        let response = match http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("从源 {url} 提取数据失败: {e}");
                continue;
            }
        };
        if !response.status().is_success() {
            eprintln!("源 {url} 请求失败: {}", response.status());
            continue;
        }
        let html_bytes = match response.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => {
                eprintln!("从源 {url} 提取数据失败: {e}");
                continue;
            }
        };
        // 源 `Encoding.UTF8.GetString(htmlBytes)`：UTF-8 解码，无效字节 → U+FFFD
        let html_content = String::from_utf8_lossy(&html_bytes).to_string();

        // 源：缓存写入（try/catch → "缓存写入失败"）
        write_cache(&cache_file_path, &html_content);

        let result = parse_forge_versions(mc_version, &forge_mc_version, &html_content);
        if !result.is_empty() {
            return Ok(result);
        }
    }

    // 源：全部源失败后回退读取过期缓存
    if Path::new(&cache_file_path).is_file() {
        match std::fs::read_to_string(&cache_file_path) {
            Ok(cached_html) => {
                eprintln!("读取已缓存html到{cache_file_path}中的数据");
                return Ok(parse_forge_versions(
                    mc_version,
                    &forge_mc_version,
                    &cached_html,
                ));
            }
            Err(e) => eprintln!("使用过期缓存失败: {e}"),
        }
    }

    Ok(Vec::new())
}

/// 解析 Forge 官方 HTML 版本表格（源：`ParseForgeVersions(string minecraftVersion,
/// string forgeMcVersion, string htmlContent)`，static）。
///
/// 逐字保留源逻辑：
/// - 表格正则 `<table[^>]+class="[^"]*download-list[^"]*"[^>]*>.*?</table>`
///   （源 Singleline，即 (?s)）；未找到 → 日志"未找到版本表格" + 空列表；
/// - 行正则 `<tr[^>]*>.*?<td[^>]+class="[^"]*download-version[^"]*"[^>]*>.*?</tr>`
///   （(?s)），日志"找到 N 个版本行"；
/// - 每行：版本号（download-version 单元格，见 U1 环视改写）+ 可选 `-` 后缀；
///  分类 `classifier-(installer|universal|client)`（缺省 installer）；
///  下载 URL 正则
///  `href="([^"]*?forge-(?:{Escape(mc)}|{Escape(forgeMc)}|.{Escape(mc)})-{Escape(ver)}.*?{category}\.(jar|zip)[^"]*)"`
///  失败回退 `href="([^"]*?forge-.*?\.jar[^"]*)"`（均忽略大小写）；
///  URL 经 `clean_download_url` 清理；SHA1 正则 `(?i)sha1[:=]\s*([a-f0-9]{40})`；
///  推荐标记：行含 "promo-recommended" 或 "promo-latest"（忽略大小写）；
/// - 收集后按 `VersionSortInteger` 降序（源 `Sort((a, b) => VersionSortInteger(b.Version, a.Version))`），
///   日志"最终提取到 N 个有效版本"。
pub(crate) fn parse_forge_versions(
    mc_version: &str,
    forge_mc_version: &str,
    html_content: &str,
) -> Vec<ModLoaderResult> {
    let mut forge_loaders: Vec<ModLoaderResult> = Vec::new();

    let table_match = download_table_regex().find(html_content);
    let Some(table_match) = table_match else {
        eprintln!("未找到版本表格");
        return forge_loaders;
    };

    let row_matches: Vec<regex::Match<'_>> = download_row_regex()
        .find_iter(table_match.as_str())
        .collect();
    eprintln!("找到 {} 个版本行", row_matches.len());

    for row_match in row_matches {
        let row_html = row_match.as_str();

        // 源版本正则（IgnoreCase，lookbehind + lookahead）改写为消费式 + 后置断言（U1）
        let version_match = version_cell_regex().captures(row_html);
        let Some(version_match) = version_match else {
            continue;
        };
        let match_end = version_match.get(0).map(|m| m.end()).unwrap_or(0);
        // 等价于源 lookahead `(?=\s*<)`：捕获后必须为可选空白 + '<'
        let after =
            row_html[match_end..].trim_start_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c']);
        if !after.starts_with('<') {
            continue;
        }
        let forge_version = version_match
            .get(1)
            .map(|g| g.as_str())
            .unwrap_or_default()
            .to_string();

        let category_match = classifier_regex().captures(row_html);
        let file_category = match category_match {
            Some(m) => m.get(1).map(|g| g.as_str()).unwrap_or("installer"),
            None => "installer",
        };

        // 源 URL 正则（U2：Regex.Escape → regex::escape）
        let url_pattern = format!(
            r#"(?i)href="([^"]*?forge-(?:{}|{}|.{})-{}.*?{}\.(jar|zip)[^"]*)""#,
            regex::escape(mc_version),
            regex::escape(forge_mc_version),
            regex::escape(mc_version),
            regex::escape(&forge_version),
            file_category,
        );
        let url_regex = Regex::new(&url_pattern).expect("Forge 版本行 URL 正则编译失败");
        let mut url_match = url_regex.captures(row_html);
        if url_match.is_none() {
            url_match = fallback_url_regex().captures(row_html);
        }
        let Some(url_match) = url_match else {
            continue;
        };
        let raw_download_url = url_match
            .get(1)
            .map(|g| g.as_str())
            .unwrap_or_default()
            .to_string();
        let clean_download_url = clean_download_url(&raw_download_url);

        let sha1_match = sha1_regex().captures(row_html);
        let sha1 = sha1_match
            .map(|m| {
                m.get(1)
                    .map(|g| g.as_str().trim().to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let lower_row = row_html.to_ascii_lowercase();
        let is_recommended =
            lower_row.contains("promo-recommended") || lower_row.contains("promo-latest");

        forge_loaders.push(ModLoaderResult {
            r#type: ModLoaderType::Forge,
            version: forge_version,
            game_version: mc_version.to_string(),
            url: clean_download_url,
            sha1,
            is_recommand: is_recommended,
            release_time: MIN_RELEASE_TIME.to_string(),
        });
    }

    // 源：`forgeLoaders.Sort((a, b) => VersionSortInteger(b.Version, a.Version))`
    forge_loaders.sort_by(|a, b| version_sort_integer(&b.version, &a.version).cmp(&0));
    eprintln!("最终提取到 {} 个有效版本", forge_loaders.len());
    forge_loaders
}

/// 清理 Forge 下载 URL（源：`CleanDownloadUrl(string rawUrl)`，static）。
///
/// 逐字保留源逻辑：
/// - 含 "adfoc.us"：先 `WebUtility.UrlDecode`（`web_url_decode`）解码，再正则
///   `https://maven\.minecraftforge\.net/.*?\.jar` 提取直链，命中 → 返回；
///   未命中 → 继续执行下方分支（源无 return/else）；
/// - 不以 "http" 开头：拼接 `https://files.minecraftforge.net`（开头为 `/` 直接
///   拼接，否则补 `/`）；
/// - 其余原样返回。
pub(crate) fn clean_download_url(raw_url: &str) -> String {
    if raw_url.contains("adfoc.us") {
        let decoded_url = web_url_decode(raw_url);
        if let Some(m) = maven_jar_regex().find(&decoded_url) {
            return m.as_str().to_string();
        }
    }
    if !raw_url.starts_with("http") {
        let prefix = "https://files.minecraftforge.net";
        return if raw_url.starts_with('/') {
            format!("{prefix}{raw_url}")
        } else {
            format!("{prefix}/{raw_url}")
        };
    }
    raw_url.to_string()
}

/// 构造 Forge 安装器下载 URL（源：`GetForgeDownloadUrl(string mcVersion,
/// string forgeVersion)`，实例方法，源读 `_mirror` → 显式参数）。
///
/// - `forgeVersion` 为空 → 空字符串（源直接返回 string.Empty）；
/// - BMCLAPI：`https://bmclapi2.bangbang93.com/forge/download/{forgeVersion}`；
/// - 官方：`https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{ver}/forge-{mc}-{ver}-installer.jar`，
///   其中 `mc` = `mcVersion.Replace('-', "_")`（两次替换，源同）。
pub(crate) fn get_forge_download_url(
    mirror: DownloadMirror,
    mc_version: &str,
    forge_version: &str,
) -> String {
    if forge_version.is_empty() {
        return String::new();
    }
    match mirror {
        DownloadMirror::Bmclapi => {
            format!("https://bmclapi2.bangbang93.com/forge/download/{forge_version}")
        }
        DownloadMirror::Official => {
            let mc = mc_version.replace('-', "_");
            format!(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{forge_version}/forge-{mc}-{forge_version}-installer.jar"
            )
        }
    }
}

/// 推荐版本判定（源：`IsRecommendedVersion(string buildNumber,
/// List<ModLoaderResult> existingLoaders)`，static）。
///
/// 逐字保留源逻辑：`buildNumber` 非整数 → false；已收录列表中任一版本的
/// `Version.Split('.').LastOrDefault()`（末段）可解析为整数且 `currentBuild <=
/// existingBuild` → false；否则 true。
pub(crate) fn is_recommended_version(
    build_number: &str,
    existing_loaders: &[ModLoaderResult],
) -> bool {
    let Ok(current_build) = build_number.parse::<i32>() else {
        return false;
    };
    for loader in existing_loaders {
        let last_part = loader.version.rsplit('.').next().unwrap_or_default();
        if let Ok(existing_build) = last_part.parse::<i32>() {
            if current_build <= existing_build {
                return false;
            }
        }
    }
    true
}

/// Forge 版本缓存文件路径（源：`GetCacheFilePath(string minecraftVersion)`，static）。
///
/// `Path.Combine(Path.GetTempPath(), "ForgeVersionCache", $"{minecraftVersion}_forge.html")`。
pub(crate) fn get_cache_file_path(minecraft_version: &str) -> String {
    forge_cache_dir()
        .join(format!("{minecraft_version}_forge.html"))
        .to_string_lossy()
        .to_string()
}

/// Forge `maven-metadata.xml` 缓存路径（issue #176 新增主路径）。
pub(crate) fn get_metadata_cache_file_path(minecraft_version: &str) -> String {
    forge_cache_dir()
        .join(format!("{minecraft_version}_forge_metadata.xml"))
        .to_string_lossy()
        .to_string()
}

/// Forge 推荐版本清单缓存路径（issue #176 新增主路径）。
pub(crate) fn get_promotions_cache_file_path(minecraft_version: &str) -> String {
    forge_cache_dir()
        .join(format!("{minecraft_version}_forge_promotions.json"))
        .to_string_lossy()
        .to_string()
}

/// `%TEMP%/ForgeVersionCache`（三种 Forge 版本信息缓存的公共目录）。
fn forge_cache_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("ForgeVersionCache")
}

/// Cleanroom GitHub Releases 请求与解析（源 `GetCleanroomVersions` 的 try 块）。
async fn cleanroom_releases_inner(http: &reqwest::Client) -> Result<Vec<ModLoaderResult>, Error> {
    let response = http
        .get("https://api.github.com/repos/CleanroomMC/Cleanroom/releases")
        .send()
        .await
        .map_err(|e| Error::Http {
            message: format!("Cleanroom GitHub API 请求失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
    // 源 `EnsureSuccessStatusCode()`
    let response = response.error_for_status().map_err(|e| Error::Http {
        message: format!("Cleanroom GitHub API 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let json = response.text().await.map_err(|e| Error::Http {
        message: format!("Cleanroom GitHub API 响应读取失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let value: Value = serde_json::from_str(&json).map_err(|e| Error::Http {
        message: format!("Cleanroom 版本列表 JSON 解析失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    // 源 `JsonNode.Parse(json)!.AsArray()`：非数组 → InvalidOperationException
    let releases = value.as_array().ok_or_else(|| Error::Http {
        message: "Cleanroom 版本列表非数组".to_string(),
        status: None,
        source: None,
    })?;

    let mut result: Vec<ModLoaderResult> = Vec::new();
    for release in releases.iter().filter_map(|r| r.as_object()) {
        let tag_name = node_to_string(release.get("tag_name").unwrap_or(&Value::Null));
        if tag_name.is_empty() {
            continue;
        }

        // 源：`tagName.Contains("alpha", StringComparison.OrdinalIgnoreCase)`
        let is_beta = tag_name.to_ascii_lowercase().contains("alpha");

        // 源：`tagName.Contains('-') ? tagName[..tagName.IndexOf('-')] : tagName`
        let ver_str = match tag_name.find('-') {
            Some(idx) => &tag_name[..idx],
            None => tag_name.as_str(),
        };
        // 源：`if (!System.Version.TryParse(verStr, out _)) continue;`（U3 近似）
        if !version_try_parse(ver_str) {
            continue;
        }

        result.push(ModLoaderResult {
            r#type: ModLoaderType::Cleanroom,
            version: tag_name.clone(),
            game_version: "1.12.2".to_string(),
            url: format!(
                "https://github.com/CleanroomMC/Cleanroom/releases/download/{tag_name}/cleanroom-{tag_name}-installer.jar"
            ),
            sha1: String::new(),
            is_recommand: !is_beta,
            release_time: MIN_RELEASE_TIME.to_string(),
        });
    }
    Ok(sort_and_deduplicate(result))
}

/// NeoForge 官方 API 请求与解析（源 `GetNeoForgeFromOfficialApi` 的 try 块）。
async fn neoforge_official_api_inner(
    http: &reqwest::Client,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    const OLD_URL: &str =
        "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge";
    const META_URL: &str =
        "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";

    // 源 `Task.WhenAll(oldTask, metaTask)`：两个请求并行
    let (old_task, meta_task) = tokio::join!(http.get(OLD_URL).send(), http.get(META_URL).send());
    let old_response = old_task.map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let meta_response = meta_task.map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    // 源两处 `EnsureSuccessStatusCode()`
    let old_response = old_response.error_for_status().map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let meta_response = meta_response.error_for_status().map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let old_json = old_response.text().await.map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API 响应读取失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let meta_json = meta_response.text().await.map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API 响应读取失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let old_obj: Value = serde_json::from_str(&old_json).map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API JSON 解析失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let meta_obj: Value = serde_json::from_str(&meta_json).map_err(|e| Error::Http {
        message: format!("NeoForge 官方 API JSON 解析失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    // 源 `JsonNode.Parse(json)!.AsObject()`：非对象 → InvalidOperationException
    let old_obj = old_obj.as_object().ok_or_else(|| Error::Http {
        message: "NeoForge 官方 API 响应非对象".to_string(),
        status: None,
        source: None,
    })?;
    let meta_obj = meta_obj.as_object().ok_or_else(|| Error::Http {
        message: "NeoForge 官方 API 响应非对象".to_string(),
        status: None,
        source: None,
    })?;

    let mut versions: Vec<ModLoaderResult> = Vec::new();

    // 源：`if (MatchesMinecraftVersion("1.20.1", minecraftVersion))` → 旧版（forge）列表
    if matches_minecraft_version("1.20.1", mc_version) {
        if let Some(versions_node) = old_obj.get("versions") {
            // 源 `oldObj["versions"]?.AsArray()`：存在但非数组 → AsArray 抛异常 → catch
            let old_versions = versions_node.as_array().ok_or_else(|| Error::Http {
                message: "NeoForge 官方 API versions 非数组".to_string(),
                status: None,
                source: None,
            })?;
            for v in old_versions {
                let ver = node_to_string(v);
                if ver.is_empty() {
                    continue;
                }
                versions.push(ModLoaderResult {
                    r#type: ModLoaderType::NeoForge,
                    version: ver.clone(),
                    game_version: "1.20.1".to_string(),
                    url: format!(
                        "https://maven.neoforged.net/releases/net/neoforged/forge/{ver}/forge-{ver}-installer.jar"
                    ),
                    sha1: String::new(),
                    is_recommand: !ver.to_ascii_lowercase().contains("beta"),
                    release_time: MIN_RELEASE_TIME.to_string(),
                });
            }
        }
    }

    // 源：新版（neoforge）列表
    if let Some(versions_node) = meta_obj.get("versions") {
        let meta_versions = versions_node.as_array().ok_or_else(|| Error::Http {
            message: "NeoForge 官方 API versions 非数组".to_string(),
            status: None,
            source: None,
        })?;
        for v in meta_versions {
            let ver = node_to_string(v);
            if ver.is_empty() {
                continue;
            }
            let parsed_mc_version = parse_neoforge_minecraft_version(&ver);
            if parsed_mc_version.is_empty() {
                continue;
            }
            if !mc_version.is_empty() && !matches_minecraft_version(&parsed_mc_version, mc_version)
            {
                continue;
            }
            versions.push(ModLoaderResult {
                r#type: ModLoaderType::NeoForge,
                version: ver.clone(),
                game_version: parsed_mc_version,
                url: format!(
                    "https://maven.neoforged.net/releases/net/neoforged/neoforge/{ver}/neoforge-{ver}-installer.jar"
                ),
                sha1: String::new(),
                is_recommand: !ver.to_ascii_lowercase().contains("beta"),
                release_time: MIN_RELEASE_TIME.to_string(),
            });
        }
    }

    // 源：`.GroupBy(v => v.Version).Select(g => g.First()).OrderByDescending(v => v.Version, new VersionComparer()).ToList()`
    Ok(sort_and_deduplicate(versions))
}

/// NeoForge BMCLAPI 请求与解析（源 `GetNeoForgeFromBmclApi` 的 try 块）。
async fn neoforge_bmcl_api_inner(
    http: &reqwest::Client,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    // 源：`if (string.IsNullOrEmpty(minecraftVersion)) return result;`
    if mc_version.is_empty() {
        return Ok(Vec::new());
    }

    let url = format!(
        "https://bmclapi2.bangbang93.com/neoforge/list/{}",
        escape_data_string(mc_version)
    );
    let response = http.get(&url).send().await.map_err(|e| Error::Http {
        message: format!("NeoForge BMCLAPI 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    // 源 `EnsureSuccessStatusCode()`
    let response = response.error_for_status().map_err(|e| Error::Http {
        message: format!("NeoForge BMCLAPI 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let json = response.text().await.map_err(|e| Error::Http {
        message: format!("NeoForge BMCLAPI 响应读取失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    let value: Value = serde_json::from_str(&json).map_err(|e| Error::Http {
        message: format!("NeoForge BMCLAPI 版本列表 JSON 解析失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    // 源 `JsonNode.Parse(json)!.AsArray()`
    let array = value.as_array().ok_or_else(|| Error::Http {
        message: "NeoForge BMCLAPI 版本列表非数组".to_string(),
        status: None,
        source: None,
    })?;

    let mut result: Vec<ModLoaderResult> = Vec::new();
    for item in array.iter().filter_map(|i| i.as_object()) {
        let version = node_to_string(item.get("version").unwrap_or(&Value::Null));
        let mc_version_from_item = node_to_string(item.get("mcversion").unwrap_or(&Value::Null));
        if version.is_empty() || mc_version_from_item.is_empty() {
            continue;
        }
        let download_url = format!(
            "https://bmclapi2.bangbang93.com/neoforge/version/{}/download/installer.jar",
            escape_data_string(&version)
        );
        // 源：`!version.Contains("-beta") && !version.Contains("-alpha")`（区分大小写）
        let is_recommand = !version.contains("-beta") && !version.contains("-alpha");
        result.push(ModLoaderResult {
            r#type: ModLoaderType::NeoForge,
            version,
            game_version: mc_version_from_item,
            url: download_url,
            sha1: String::new(),
            is_recommand,
            release_time: MIN_RELEASE_TIME.to_string(),
        });
    }
    Ok(sort_and_deduplicate(result))
}

/// Forge BMCLAPI JSON 请求与解析（源 `GetForgeVersionsFromBmclApi` 的 try 块）。
async fn forge_versions_from_bmcl_api_inner(
    http: &reqwest::Client,
    mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    let url = format!(
        "https://bmclapi2.bangbang93.com/forge/minecraft/{}",
        escape_data_string(mc_version)
    );
    let mut forge_loaders: Vec<ModLoaderResult> = Vec::new();
    let response = http.get(&url).send().await.map_err(|e| Error::Http {
        message: format!("BMCLAPI Forge 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    })?;
    // 源：`if (response.IsSuccessStatusCode)`（非 2xx 静默跳过，不记日志）
    if response.status().is_success() {
        let json = response.text().await.map_err(|e| Error::Http {
            message: format!("BMCLAPI Forge 响应读取失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let value: Value = serde_json::from_str(&json).map_err(|e| Error::Http {
            message: format!("BMCLAPI Forge 版本列表 JSON 解析失败: {e}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        // 源 `JsonNode.Parse(json)!.AsArray()`
        let versions_array = value.as_array().ok_or_else(|| Error::Http {
            message: "BMCLAPI Forge 版本列表非数组".to_string(),
            status: None,
            source: None,
        })?;

        for version in versions_array.iter().filter_map(|v| v.as_object()) {
            let api_mc_version = node_to_string(version.get("mcversion").unwrap_or(&Value::Null));
            // 源：`!apiMcVersion.Equals(minecraftVersion, StringComparison.OrdinalIgnoreCase)`
            if !api_mc_version.eq_ignore_ascii_case(mc_version) {
                continue;
            }

            // 源：files 数组中首个 `category` == "installer"（OrdinalIgnoreCase）的文件
            let mut installer_file: Option<&Map<String, Value>> = None;
            if let Some(files) = version.get("files").and_then(|f| f.as_array()) {
                for f in files.iter().filter_map(|f| f.as_object()) {
                    let category = node_to_string(f.get("category").unwrap_or(&Value::Null));
                    if category.eq_ignore_ascii_case("installer") {
                        installer_file = Some(f);
                        break;
                    }
                }
            }
            let Some(installer_file) = installer_file else {
                continue;
            };

            let build = node_to_string(version.get("build").unwrap_or(&Value::Null));
            let loader_version = node_to_string(version.get("version").unwrap_or(&Value::Null));
            let modified = node_to_string(version.get("modified").unwrap_or(&Value::Null));

            // 源：`version["modified"] != null ? DateTimeOffset.TryParse(...) ? dt : MinValue : MinValue`
            //  U4：TryParse 门控用 chrono rfc3339 近似；解析成功仍存原始文本（B1 原始文本保真）
            let release_time = if modified.is_empty() {
                MIN_RELEASE_TIME.to_string()
            } else if chrono::DateTime::parse_from_rfc3339(&modified).is_ok() {
                modified
            } else {
                MIN_RELEASE_TIME.to_string()
            };

            forge_loaders.push(ModLoaderResult {
                r#type: ModLoaderType::Forge,
                version: loader_version,
                game_version: mc_version.to_string(),
                url: get_forge_download_url(mirror, mc_version, &build),
                sha1: node_to_string(installer_file.get("hash").unwrap_or(&Value::Null)),
                is_recommand: is_recommended_version(&build, &forge_loaders),
                release_time,
            });
        }
    }
    Ok(forge_loaders)
}

/// 模拟 C# `JsonNode.ToString()`：`JsonValue(string)` 返回不带引号的原始字符串，
/// 其余节点（数字/布尔/对象/数组）返回其 JSON 序列化文本；JSON null → 空字符串
/// （对应源 `?.ToString()` 可空传播语义，同 forge_base.rs 的 node_to_string）。
fn node_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 模拟 C# `Uri.EscapeDataString`（U2 详见日志）：除 RFC 3986 非保留字符
/// （ALPHA / DIGIT / `-` / `.` / `_` / `~`）外全部按 UTF-8 字节百分号编码，十六进制大写。
fn escape_data_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 模拟 C# `WebUtility.UrlDecode`：`%XX` 十六进制解码（无效序列原样保留）+
/// `+` → 空格；解码字节按 UTF-8 lossy 还原为字符串。
fn web_url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h1), Some(h2)) => {
                    out.push(h1 << 4 | h2);
                    i += 3;
                }
                _ => {
                    out.push(b);
                    i += 1;
                }
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 近似 .NET `System.Version.TryParse`（U3 详见日志）：trim 后按 `.` 分段，
/// 1..=4 段、每段非空且为可解析为 i32 的十进制数。省略 .NET Core 3.0+ 的
/// `v`/`V` 前缀与负分量支持（Cleanroom GitHub 标签实际形态不受影响）。
fn version_try_parse(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 4 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<i32>().is_ok())
}

/// 版本号比较（源 InstallerProvider 私有静态 `VersionSortInteger` + `VersionComparer`，
/// 逐字保留，含"未知版本"分支）。⚠️ 与 util/lib_helper.rs 的私有同名实现
/// （LibHelper 版，无"未知版本"分支）并存，按源分别移植。
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

    let left = left
        .to_lowercase()
        .replace("快照", "snapshot")
        .replace("预览版", "pre");
    let right = right
        .to_lowercase()
        .replace("快照", "snapshot")
        .replace("预览版", "pre");

    // 源：`Regex.Matches(left, "[a-z]+|[0-9]+")`
    let left_parts: Vec<String> = version_token_regex()
        .find_iter(&left)
        .map(|m| m.as_str().to_string())
        .collect();
    let right_parts: Vec<String> = version_token_regex()
        .find_iter(&right)
        .map(|m| m.as_str().to_string())
        .collect();

    let mut i = 0usize;
    loop {
        if i >= left_parts.len() && i >= right_parts.len() {
            return string_compare_ordinal(&left, &right);
        }
        let l_val = left_parts.get(i).map(|s| s.as_str()).unwrap_or("-1");
        let r_val = right_parts.get(i).map(|s| s.as_str()).unwrap_or("-1");
        if l_val == r_val {
            i += 1;
            continue;
        }
        let l_val = convert_special_label(l_val);
        let r_val = convert_special_label(r_val);
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
            _ => return string_compare_ordinal(l_val, r_val),
        }
    }
}

/// 特殊版本标签转换（源 `ConvertSpecialLabel`，逐字）：pre/snapshot → "-3"、
/// rc → "-2"、experimental → "-4"、其余原样。
fn convert_special_label(label: &str) -> &str {
    match label {
        "pre" | "snapshot" => "-3",
        "rc" => "-2",
        "experimental" => "-4",
        _ => label,
    }
}

/// 模拟 C# `string.Compare(a, b, StringComparison.Ordinal)` 的符号语义（-1/0/1）。
fn string_compare_ordinal(a: &str, b: &str) -> i32 {
    match a.cmp(b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// 规范化 Minecraft 版本（源 `NormalizeMinecraftVersion`，逐字）：空白 → 空；
/// 以 "1." 开头 → 原样；取 `-` 前为基础版本，按 `.` 分段（去空段）少于 2 → 原样；
/// 首段整数 >= 22 → `1.{baseVersion}`，否则原样。
fn normalize_minecraft_version(version: &str) -> String {
    if version.trim().is_empty() {
        return String::new();
    }
    let version = version.trim();
    if version.starts_with("1.") {
        return version.to_string();
    }
    let base_version = match version.find('-') {
        Some(idx) => &version[..idx],
        None => version,
    };
    let parts: Vec<&str> = base_version.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return version.to_string();
    }
    match parts[0].parse::<i32>() {
        Ok(major) if major >= 22 => format!("1.{base_version}"),
        _ => version.to_string(),
    }
}

/// 生成 MC 版本别名集（源 `GetMinecraftVersionAliases`，逐字）：原始版本 +
/// 规范化版本；规范化版本以 "1." 开头 → 追加去掉 "1." 前缀的形态，否则追加
/// `1.{normalized}`。C# 用 `HashSet(StringComparer.OrdinalIgnoreCase)` 去重 →
/// 本实现按忽略大小写去重。
fn get_minecraft_version_aliases(version: &str) -> Vec<String> {
    let mut aliases: Vec<String> = Vec::new();
    if version.trim().is_empty() {
        return aliases;
    }
    let version = version.trim();

    if !aliases.iter().any(|a| a.eq_ignore_ascii_case(version)) {
        aliases.push(version.to_string());
    }
    let normalized = normalize_minecraft_version(version);
    if !aliases.iter().any(|a| a.eq_ignore_ascii_case(&normalized)) {
        aliases.push(normalized.clone());
    }
    if normalized.starts_with("1.") {
        let stripped = &normalized[2..];
        if !aliases.iter().any(|a| a.eq_ignore_ascii_case(stripped)) {
            aliases.push(stripped.to_string());
        }
    } else {
        let with_prefix = format!("1.{normalized}");
        if !aliases.iter().any(|a| a.eq_ignore_ascii_case(&with_prefix)) {
            aliases.push(with_prefix);
        }
    }
    aliases
}

/// MC 版本匹配（源 `MatchesMinecraftVersion`，逐字）：两侧任一空白 → false；
/// 两侧别名集交集非空（C# `Intersect(..., OrdinalIgnoreCase)`）。
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

/// 去重 + VersionComparer 降序（源 `SortAndDeduplicate`，逐字）：按 `Version`
/// 分组保留首个（GroupBy 默认序数比较），再 `OrderByDescending(v => v.Version,
/// new VersionComparer())`（稳定排序）。
fn sort_and_deduplicate(versions: Vec<ModLoaderResult>) -> Vec<ModLoaderResult> {
    let mut seen: Vec<String> = Vec::new();
    let mut dedup: Vec<ModLoaderResult> = Vec::new();
    for v in versions {
        if !seen.iter().any(|s| *s == v.version) {
            seen.push(v.version.clone());
            dedup.push(v);
        }
    }
    dedup.sort_by(|a, b| version_sort_integer(&b.version, &a.version).cmp(&0));
    dedup
}

/// 版本分词正则（源 `Regex.Matches(left, "[a-z]+|[0-9]+")`）。
fn version_token_regex() -> &'static Regex {
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    TOKEN_RE.get_or_init(|| Regex::new(r"[a-z]+|[0-9]+").expect("静态正则编译失败"))
}

/// Forge 版本表格正则（源 `<table[^>]+class="[^"]*download-list[^"]*"[^>]*>.*?</table>`，
/// RegexOptions.Singleline → (?s)）。
fn download_table_regex() -> &'static Regex {
    static TABLE_RE: OnceLock<Regex> = OnceLock::new();
    TABLE_RE.get_or_init(|| {
        Regex::new(r#"(?s)<table[^>]+class="[^"]*download-list[^"]*"[^>]*>.*?</table>"#)
            .expect("静态正则编译失败")
    })
}

/// 版本行正则（源 `<tr[^>]*>.*?<td[^>]+class="[^"]*download-version[^"]*"[^>]*>.*?</tr>`，
/// Singleline → (?s)）。
fn download_row_regex() -> &'static Regex {
    static ROW_RE: OnceLock<Regex> = OnceLock::new();
    ROW_RE.get_or_init(|| {
        Regex::new(r#"(?s)<tr[^>]*>.*?<td[^>]+class="[^"]*download-version[^"]*"[^>]*>.*?</tr>"#)
            .expect("静态正则编译失败")
    })
}

/// 版本单元格正则（源 lookbehind `(?<=<td[^>]+class="[^"]*download-version[^"]*"[^>]*>\s*)`
/// + `[\d.]+(?:-[a-zA-Z0-9_]+)?` + lookahead `(?=\s*<)`，IgnoreCase → (?i)）。
/// Rust regex 不支持环视：改写为消费式前缀 + 捕获组，lookahead 由调用方
/// 手工断言（等价性论证见日志 U1）。
fn version_cell_regex() -> &'static Regex {
    static VERSION_RE: OnceLock<Regex> = OnceLock::new();
    VERSION_RE.get_or_init(|| {
        Regex::new(
            r#"(?i)<td[^>]+class="[^"]*download-version[^"]*"[^>]*>\s*([\d.]+(?:-[a-zA-Z0-9_]+)?)"#,
        )
        .expect("静态正则编译失败")
    })
}

/// 文件分类正则（源 `classifier-(installer|universal|client)`，IgnoreCase → (?i)）。
fn classifier_regex() -> &'static Regex {
    static CLASSIFIER_RE: OnceLock<Regex> = OnceLock::new();
    CLASSIFIER_RE.get_or_init(|| {
        Regex::new(r"(?i)classifier-(installer|universal|client)").expect("静态正则编译失败")
    })
}

/// 下载 URL 回退正则（源 `href="([^"]*?forge-.*?\.jar[^"]*)"`，IgnoreCase → (?i)）。
fn fallback_url_regex() -> &'static Regex {
    static FALLBACK_URL_RE: OnceLock<Regex> = OnceLock::new();
    FALLBACK_URL_RE.get_or_init(|| {
        Regex::new(r#"(?i)href="([^"]*?forge-.*?\.jar[^"]*)""#).expect("静态正则编译失败")
    })
}

/// SHA1 正则（源 `(?i)sha1[:=]\s*([a-f0-9]{40})`）。
fn sha1_regex() -> &'static Regex {
    static SHA1_RE: OnceLock<Regex> = OnceLock::new();
    SHA1_RE.get_or_init(|| Regex::new(r"(?i)sha1[:=]\s*([a-f0-9]{40})").expect("静态正则编译失败"))
}

/// Maven 直链正则（源 `https://maven\.minecraftforge\.net/.*?\.jar`，CleanDownloadUrl 内）。
fn maven_jar_regex() -> &'static Regex {
    static MAVEN_JAR_RE: OnceLock<Regex> = OnceLock::new();
    MAVEN_JAR_RE.get_or_init(|| {
        Regex::new(r"https://maven\.minecraftforge\.net/.*?\.jar").expect("静态正则编译失败")
    })
}

/// `maven-metadata.xml` 的 `<version>` 条目正则（issue #176）。
///
/// 只取 `<versions>` 列表里的直接文本；`<latest>` / `<release>` 等兄弟节点不匹配
/// （它们的标签名不是 `version`），故不会污染结果。`(?s)` 容忍节点间换行缩进。
fn metadata_version_regex() -> &'static Regex {
    static METADATA_VERSION_RE: OnceLock<Regex> = OnceLock::new();
    METADATA_VERSION_RE.get_or_init(|| {
        Regex::new(r"(?s)<version>\s*([^<\s][^<]*?)\s*</version>").expect("静态正则编译失败")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 `maven-metadata.xml` 节选（含 `<latest>`/`<release>` 干扰节点与多 MC 前缀）。
    const METADATA_SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>net.minecraftforge</groupId>
  <artifactId>forge</artifactId>
  <versioning>
    <latest>1.20.1-47.4.26</latest>
    <release>1.20.1-47.4.26</release>
    <versions>
      <version>1.20.1-47.4.26</version>
      <version>1.12.2-14.23.5.2864</version>
      <version>1.12.2-14.23.5.2860</version>
      <version>1.12.2-14.23.5.2859</version>
      <version>1.7.10-10.13.4.1614-1.7.10</version>
      <version>1.7.10_pre4-10.12.2.1149-prerelease</version>
      <version>1.12.20-99.9.9</version>
    </versions>
  </versioning>
</metadata>"#;

    fn version_of<'a>(results: &'a [ModLoaderResult], v: &str) -> Option<&'a ModLoaderResult> {
        results.iter().find(|r| r.version == v)
    }

    /// issue #176 主用例：模组包所需的 14.23.5.2860 必须能定位到可下载的安装器直链。
    #[test]
    fn metadata_parses_issue_176_version_with_installer_url() {
        let results = parse_forge_metadata_versions(METADATA_SAMPLE, "1.12.2");

        let hit = version_of(&results, "14.23.5.2860")
            .expect("14.23.5.2860 必须从 maven-metadata 中解析出来");
        assert_eq!(hit.r#type, ModLoaderType::Forge);
        assert_eq!(hit.game_version, "1.12.2");
        assert_eq!(
            hit.url,
            "https://maven.minecraftforge.net/net/minecraftforge/forge/\
             1.12.2-14.23.5.2860/forge-1.12.2-14.23.5.2860-installer.jar"
        );
    }

    /// 严格「{mc}-」前缀：1.12.2 不得吃掉 1.12.20，也不得带上 1.7.10_pre4。
    #[test]
    fn metadata_prefix_filter_is_exact() {
        let results = parse_forge_metadata_versions(METADATA_SAMPLE, "1.12.2");
        let versions: Vec<&str> = results.iter().map(|r| r.version.as_str()).collect();

        assert!(
            !versions.contains(&"99.9.9"),
            "1.12.2 不得匹配 1.12.20-99.9.9"
        );
        assert!(
            !versions.contains(&"10.12.2.1149-prerelease"),
            "1.12.2 不得匹配 1.7.10_pre4-*"
        );
        assert_eq!(results.len(), 3, "样例中 1.12.2 恰有 3 个版本");
    }

    /// `<latest>`/`<release>` 是兄弟节点，不能混进版本列表。
    #[test]
    fn metadata_ignores_latest_and_release_nodes() {
        let results = parse_forge_metadata_versions(METADATA_SAMPLE, "1.20.1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].version, "47.4.26");
        assert!(!version_of(&results, "47.4.26").unwrap().is_recommand);
    }

    /// 与 HTML 路径同序：降序，前端「最新版」取首项。
    #[test]
    fn metadata_sorted_descending() {
        let results = parse_forge_metadata_versions(METADATA_SAMPLE, "1.12.2");
        assert_eq!(results[0].version, "14.23.5.2864");
        assert_eq!(results[2].version, "14.23.5.2859");
    }

    /// 推荐标记：latest + recommended 都要打上（#176 记录的 1.12.2 = 2864/2859）。
    #[test]
    fn promotions_marks_latest_and_recommended() {
        let json = r#"{"homepage":"https://files.minecraftforge.net/","promos":{
            "1.12.2-latest":"14.23.5.2864",
            "1.12.2-recommended":"14.23.5.2859",
            "1.20.1-latest":"47.4.26"}}"#;

        let mut promos = parse_forge_promotions(json, "1.12.2").expect("应解析出推荐清单");
        promos.sort();
        assert_eq!(promos, vec!["14.23.5.2859", "14.23.5.2864"]);
    }

    /// 推荐清单异常不得阻断版本列表（宁可不标推荐，也不能整链失败）。
    #[test]
    fn promotions_tolerates_malformed_payload() {
        assert!(parse_forge_promotions("not json", "1.12.2").is_none());
        assert!(parse_forge_promotions(r#"{"promos":{}}"#, "1.12.2").is_none());
        assert!(parse_forge_promotions(r#"{"promos":{"1.12.2-latest":123}}"#, "1.12.2").is_none());
    }

    /// 坏缓存不得被采用（issue #176 回归守卫）。
    ///
    /// 修复前是「命中缓存 → 无条件采信解析结果」，因此本用例在修复前必然失败：
    /// 空 / 残缺缓存会被当成有效结果返回，导致 24h 内持续「无可用版本」。
    #[test]
    fn cache_with_unparseable_content_is_rejected() {
        let dir = std::env::temp_dir().join(format!("qmx-forge-cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1.12.2_forge_metadata.xml");
        let path_str = path.to_string_lossy().to_string();

        // 坏缓存：HTTP 错误页 / 残缺内容
        std::fs::write(&path, "<html>502 Bad Gateway</html>").unwrap();
        assert!(
            read_usable_cached_versions(&path_str, FORGE_CACHE_EXPIRY_HOURS, |t| {
                parse_forge_metadata_versions(t, "1.12.2")
            })
            .is_none(),
            "解析为空的缓存必须被拒绝（否则坏缓存续命 24h）"
        );

        // 完好缓存：同一函数必须采信
        std::fs::write(&path, METADATA_SAMPLE).unwrap();
        let parsed = read_usable_cached_versions(&path_str, FORGE_CACHE_EXPIRY_HOURS, |t| {
            parse_forge_metadata_versions(t, "1.12.2")
        })
        .expect("可解析的缓存必须被采用");
        assert_eq!(parsed.len(), 3);

        // 超期缓存：即便内容完好也不采用
        assert!(
            read_usable_cached_versions(&path_str, 0, |t| parse_forge_metadata_versions(
                t, "1.12.2"
            ))
            .is_none(),
            "过期缓存必须被拒绝"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 有下载源兜底：官方全挂时 BMCLAPI 分支仍能给出可下载直链（结构断言）。
    #[test]
    fn bmclapi_forge_download_url_shape() {
        let url = get_forge_download_url(DownloadMirror::Bmclapi, "1.12.2", "2860");
        assert_eq!(url, "https://bmclapi2.bangbang93.com/forge/download/2860");
        assert!(get_forge_download_url(DownloadMirror::Bmclapi, "1.12.2", "").is_empty());
    }
}
