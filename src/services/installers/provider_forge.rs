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
/// 按下载源分发：BMCLAPI → `get_forge_versions_from_bmcl_api`；Official →
/// `get_forge_versions_from_official_html`（源 `_mirror == DownloadMirror.BMCLAPI ? ... : ...`）。
pub(crate) async fn get_forge_versions(
    http: &reqwest::Client,
    mirror: DownloadMirror,
    mc_version: &str,
) -> Result<Vec<ModLoaderResult>, Error> {
    match mirror {
        DownloadMirror::Bmclapi => get_forge_versions_from_bmcl_api(http, mirror, mc_version).await,
        DownloadMirror::Official => {
            get_forge_versions_from_official_html(http, mirror, mc_version).await
        }
    }
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
    const CACHE_EXPIRY_HOURS: u64 = 24;

    // 源：`File.Exists(cacheFilePath) && (DateTime.Now - File.GetLastWriteTime(...)).TotalHours < 24`
    if Path::new(&cache_file_path).is_file() {
        let modified = std::fs::metadata(&cache_file_path)
            .and_then(|m| m.modified())
            .map_err(|e| Error::Params {
                message: format!("读取 Forge 版本缓存文件时间失败: {e}"),
                source: Some(Box::new(e)),
            })?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age < Duration::from_secs(CACHE_EXPIRY_HOURS * 3600) {
            match std::fs::read_to_string(&cache_file_path) {
                Ok(cached_html) => {
                    return Ok(parse_forge_versions(
                        mc_version,
                        &forge_mc_version,
                        &cached_html,
                    ));
                }
                Err(e) => eprintln!("使用缓存失败: {e}，将重新获取"),
            }
        }
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
        let cache_write = (|| -> std::io::Result<()> {
            if let Some(cache_dir) = Path::new(&cache_file_path).parent() {
                std::fs::create_dir_all(cache_dir)?;
            }
            std::fs::write(&cache_file_path, &html_content)
        })();
        match cache_write {
            Ok(()) => eprintln!("已缓存html到{cache_file_path}"),
            Err(e) => eprintln!("缓存写入失败: {e}"),
        }

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
    std::env::temp_dir()
        .join("ForgeVersionCache")
        .join(format!("{minecraft_version}_forge.html"))
        .to_string_lossy()
        .to_string()
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
