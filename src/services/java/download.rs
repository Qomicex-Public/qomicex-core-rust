//! Java 在线下载：Adoptium / Zulu / BMCLAPI（B7）
//!
//! 对应源文件：Services/JavaProvider.cs（Qomicex.Core.AOT.Services.JavaProvider）
//! 本文件只移植"在线下载"部分（扫描/推荐逻辑在 scanner.rs / recommend.rs）：
//! - GetPackages → JavaDownloader::get_packages（source 枚举分派）
//! - Adoptium：BuildAdoptiumUrl / GetLatestFromAdoptiumAsync / IsMatchingAdoptiumBinary /
//!   ToAdoptiumPackageInfo / MapAdoptiumOs / MapAdoptiumArchitecture / MapAdoptiumImageType
//! - 共用：IsPortablePackage
//! - Zulu：BuildZuluMetadataUrl / GetLatestFromZuluAsync / ParseZuluResponse /
//!   IsMatchingZuluPackage / HasZuluOrderingFields / ToComparableVersion
//! - BMCLAPI：GetLatestFromBmclapiAsync
//!
//! 端点清单（URL 模板/查询参数/JSON 字段路径逐字保留，改一个字符请求即失败）：
//! - GET https://api.adoptium.net/v3/assets/latest/{majorVersion}/hotspot
//! - GET https://api.azul.com/metadata/v1/zulu/packages/?java_version={major}&os={os}&arch={arch}&archive_type={archive_type}&java_package_type={java_package_type}&release_status=ga&availability_types=CA&latest=true&page=1&page_size=20
//! - GET https://bmclapi2.bangbang93.com/java/list
//!
//! 错误映射：网络/JSON/非 2xx → Error::Http（源 HttpRequestException/JsonException，
//! B6 先例 services/version/manifest.rs）；源各 GetLatestFrom*Async 的 catch-all → null
//! → get_packages 返回 Ok(空 Vec)。本文件不实现 api/java.rs 的 JavaProvider trait
//! （trait 整合由主控完成）。

use serde_json::Value;

use crate::error::Error;
use crate::models::java::{
    JavaArchitecture, JavaDownloadSource, JavaPackageInfo, JavaPackageType, JavaPlatform,
};

/// Java 在线下载器（源：`JavaProvider` 类的下载部分 + 构造注入的 `_http` HttpClient）。
///
/// 提供按大版本/平台/架构/包类型从指定源获取"最新包"列表的入口
/// `get_packages`；api/java.rs 的 `JavaProvider::get_packages` trait 整合由主控完成。
pub(crate) struct JavaDownloader {
    http: reqwest::Client,
}

impl JavaDownloader {
    /// 创建下载器（源：JavaProvider 构造函数 `JavaProvider(HttpClient http)` 注入 HttpClient）。
    pub(crate) fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// 获取指定大版本/平台/架构/包类型的 Java 包列表（源：`GetPackages`）。
    ///
    /// 语义逐条保留：
    /// 1. `major_version <= 0` → Err(Error::Params)（源
    ///    `ArgumentException("Java major version must be greater than 0.")`）；
    /// 2. 按 source 分派到对应源的"最新包"获取（Adoptium/Zulu/Bmclapi）；
    /// 3. 源分派结果的 catch-all → null 语义：任何失败（网络/解析/无匹配）均
    ///    吞掉 → 返回 `Ok(空 Vec)`（C# `result is not null ? new List { result }
    ///    : new List()`）。
    ///
    /// 差异说明：C# switch 的 `_ => throw ArgumentException("Unsupported Java
    /// download source")` 分支在 Rust 不可达——JavaDownloadSource 枚举穷尽
    /// （仅 Bmclapi/Adoptium/Zulu 三变体，见 models/java.rs）。
    pub(crate) async fn get_packages(
        &self,
        major_version: i32,
        platform: JavaPlatform,
        architecture: JavaArchitecture,
        package_type: JavaPackageType,
        source: JavaDownloadSource,
    ) -> Result<Vec<JavaPackageInfo>, Error> {
        if major_version <= 0 {
            return Err(Error::Params {
                message: "Java major version must be greater than 0.".to_string(),
                source: None,
            });
        }

        let latest = match source {
            JavaDownloadSource::Adoptium => {
                self.get_latest_from_adoptium(major_version, platform, architecture, package_type)
                    .await
            }
            JavaDownloadSource::Zulu => {
                self.get_latest_from_zulu(major_version, platform, architecture, package_type)
                    .await
            }
            JavaDownloadSource::Bmclapi => {
                self.get_latest_from_bmclapi(major_version, platform, architecture, package_type)
                    .await
            }
        }
        .ok()
        .flatten();

        Ok(match latest {
            Some(info) => vec![info],
            None => Vec::new(),
        })
    }

    // ==================== Adoptium ====================

    /// Adoptium 最新包端点（源：`BuildAdoptiumUrl`，URL 模板逐字保留）。
    fn build_adoptium_url(major_version: i32) -> String {
        format!("https://api.adoptium.net/v3/assets/latest/{major_version}/hotspot")
    }

    /// 从 Adoptium 获取匹配的最新包（源：`GetLatestFromAdoptiumAsync`）。
    ///
    /// 源 catch-all → null：任何失败（网络/JSON 解析/无匹配资产）→ None。
    async fn get_latest_from_adoptium(
        &self,
        major_version: i32,
        platform: JavaPlatform,
        architecture: JavaArchitecture,
        package_type: JavaPackageType,
    ) -> Result<Option<JavaPackageInfo>, Error> {
        let body = self.get_string(&Self::build_adoptium_url(major_version)).await?;
        let root: Value = serde_json::from_str(&body).map_err(|e| Error::Http {
            message: "解析 Adoptium 响应失败".to_string(),
            source: Some(Box::new(e)),
        })?;
        let assets = root.as_array().ok_or_else(|| Error::Http {
            message: "Adoptium 响应不是 JSON 数组".to_string(),
            source: None,
        })?;

        // 源：assets.FirstOrDefault(asset => IsMatchingAdoptiumBinary(...))——按数组顺序首个匹配
        for asset in assets {
            if is_matching_adoptium_binary(asset, platform, architecture, package_type) {
                return Ok(Some(to_adoptium_package_info(
                    asset,
                    major_version,
                    platform,
                    architecture,
                    package_type,
                )));
            }
        }
        Ok(None)
    }

    // ==================== Zulu ====================

    /// Zulu 元数据端点（源：`BuildZuluMetadataUrl`，查询参数逐字保留）。
    ///
    /// 参数映射：`archive_type` Windows/MacOS→zip、Linux→tar.gz；`os`
    /// Windows→windows、Linux→linux、MacOS→macos；`arch` X64→x86_64、
    /// Arm64→arm64；`java_package_type` JRE→jre、JDK→jdk。
    /// 源对 archive_type 用 `Uri.EscapeDataString` 转义；"zip"/"tar.gz" 仅含
    /// 非保留字符（字母数字与 `-_.~`），转义后不变 → 字面量插值字节一致。
    fn build_zulu_metadata_url(
        major_version: i32,
        platform: JavaPlatform,
        architecture: JavaArchitecture,
        package_type: JavaPackageType,
    ) -> String {
        let archive_type = match platform {
            JavaPlatform::Windows => "zip",
            JavaPlatform::Linux => "tar.gz",
            JavaPlatform::MacOS => "zip",
        };
        let os = match platform {
            JavaPlatform::Windows => "windows",
            JavaPlatform::Linux => "linux",
            JavaPlatform::MacOS => "macos",
        };
        let arch = match architecture {
            JavaArchitecture::X64 => "x86_64",
            JavaArchitecture::Arm64 => "arm64",
        };
        let java_package_type = match package_type {
            JavaPackageType::JRE => "jre",
            JavaPackageType::JDK => "jdk",
        };
        format!(
            "https://api.azul.com/metadata/v1/zulu/packages/?java_version={major_version}&os={os}&arch={arch}&archive_type={archive_type}&java_package_type={java_package_type}&release_status=ga&availability_types=CA&latest=true&page=1&page_size=20"
        )
    }

    /// 从 Zulu 获取匹配的最新包（源：`GetLatestFromZuluAsync`）。
    ///
    /// 源 catch-all → null：任何失败（网络/JSON 解析/排序键非法）→ None。
    async fn get_latest_from_zulu(
        &self,
        major_version: i32,
        platform: JavaPlatform,
        architecture: JavaArchitecture,
        package_type: JavaPackageType,
    ) -> Result<Option<JavaPackageInfo>, Error> {
        let url = Self::build_zulu_metadata_url(major_version, platform, architecture, package_type);
        let body = self.get_string(&url).await?;
        Ok(parse_zulu_response(
            &body,
            major_version,
            platform,
            architecture,
            package_type,
        ))
    }

    // ==================== BMCLAPI ====================

    /// 从 BMCLAPI 获取匹配的最新包（源：`GetLatestFromBmclapiAsync`）。
    ///
    /// 源行为逐条保留：
    /// 1. GET `https://bmclapi2.bangbang93.com/java/list`；
    /// 2. 403 Forbidden → null；其余非成功状态 → null（源对状态码显式判断，
    ///    不走 GetStringAsync 的抛异常路径）；
    /// 3. 响应顶层为数组 → 取数组；为对象 → 取 `body` 字段（须为数组）；
    ///    否则 → null；
    /// 4. 数组须存在"文档形状"元素（`title` 与 `file` 均非空白）→ 否则 null；
    /// 5. 最后恒返回 null（源 bug-for-bug：形状校验通过后仍 `return null`，
    ///    BMCLAPI 源实际永远取不到包——逐字保留，不做修正）。
    async fn get_latest_from_bmclapi(
        &self,
        _major_version: i32,
        _platform: JavaPlatform,
        _architecture: JavaArchitecture,
        _package_type: JavaPackageType,
    ) -> Result<Option<JavaPackageInfo>, Error> {
        let url = "https://bmclapi2.bangbang93.com/java/list";
        let response = self.http.get(url).send().await.map_err(|e| Error::Http {
            message: format!("请求失败: {url}"),
            source: Some(Box::new(e)),
        })?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Ok(None);
        }
        let body = response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应失败: {url}"),
            source: Some(Box::new(e)),
        })?;
        let token: Value = serde_json::from_str(&body).map_err(|e| Error::Http {
            message: "解析 BMCLAPI 响应失败".to_string(),
            source: Some(Box::new(e)),
        })?;

        // 源：token switch { JsonArray arr => arr, JsonObject obj => obj["body"] as JsonArray, _ => null }
        let packages: Option<&Value> = match &token {
            Value::Array(_) => Some(&token),
            Value::Object(obj) => obj.get("body").filter(|v| v.is_array()),
            _ => None,
        };
        let Some(packages) = packages else {
            return Ok(None);
        };
        let Some(packages) = packages.as_array() else {
            return Ok(None);
        };

        // 源：packages.Any(pkg => 非空白 title && 非空白 file)——"文档形状"校验
        let has_documented_shape = packages.iter().any(|pkg| {
            !node_string(pkg.get("title")).trim().is_empty()
                && !node_string(pkg.get("file")).trim().is_empty()
        });
        if !has_documented_shape {
            return Ok(None);
        }

        // ⚠️ 源此处恒 `return null`（见方法头说明）——BMCLAPI 分支永不产出包
        Ok(None)
    }

    /// GET 并读取响应文本（源：`_http.GetStringAsync(url)`）。
    ///
    /// .NET GetStringAsync 对非 2xx 状态抛 HttpRequestException；网络/读取/
    /// 状态错误一律 → Error::Http（源 catch-all 由调用方吞掉）。
    async fn get_string(&self, url: &str) -> Result<String, Error> {
        let response = self.http.get(url).send().await.map_err(|e| Error::Http {
            message: format!("请求失败: {url}"),
            source: Some(Box::new(e)),
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http {
                message: format!("请求失败 (HTTP {status}): {url}"),
                source: None,
            });
        }
        response.text().await.map_err(|e| Error::Http {
            message: format!("读取响应失败: {url}"),
            source: Some(Box::new(e)),
        })
    }
}

/// 节点 → 字符串（源：`JsonValue.ToString()` 语义）。
///
/// 字符串节点 → 内容（去引号）；数字 → 原始文本；bool → "true"/"false"；
/// 数组/对象 → JSON 文本；缺失或 JSON null → ""（源 `?.ToString() ?? string.Empty`
/// 的 null-conditional 短路语义）。
fn node_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

/// Adoptium 资产匹配（源：`IsMatchingAdoptiumBinary`）。
///
/// `binary.os` / `binary.architecture` / `binary.image_type` 与映射值逐字比较
/// （源 OrdinalIgnoreCase → eq_ignore_ascii_case），且 `binary.package.name`
/// 为可移植压缩包（IsPortablePackage）。
fn is_matching_adoptium_binary(
    asset: &Value,
    platform: JavaPlatform,
    architecture: JavaArchitecture,
    package_type: JavaPackageType,
) -> bool {
    node_string(asset.pointer("/binary/os")).eq_ignore_ascii_case(map_adoptium_os(platform))
        && node_string(asset.pointer("/binary/architecture"))
            .eq_ignore_ascii_case(map_adoptium_architecture(architecture))
        && node_string(asset.pointer("/binary/image_type"))
            .eq_ignore_ascii_case(map_adoptium_image_type(package_type))
        && is_portable_package(
            asset.pointer("/binary/package/name").and_then(Value::as_str),
            platform,
        )
}

/// Adoptium 资产 → 包信息（源：`ToAdoptiumPackageInfo`）。
///
/// JSON 字段路径逐字保留：`version.openjdk_version` → full_version、
/// `version.build` → build、`binary.package.name` → file_name、
/// `binary.package.link` → download_url、`binary.package.checksum` → sha256、
/// `binary.package.size` → size。
fn to_adoptium_package_info(
    asset: &Value,
    major_version: i32,
    platform: JavaPlatform,
    architecture: JavaArchitecture,
    package_type: JavaPackageType,
) -> JavaPackageInfo {
    JavaPackageInfo {
        major_version,
        full_version: node_string(asset.pointer("/version/openjdk_version")),
        build: node_string(asset.pointer("/version/build")),
        platform,
        architecture,
        package_type,
        source: JavaDownloadSource::Adoptium,
        file_name: node_string(asset.pointer("/binary/package/name")),
        download_url: node_string(asset.pointer("/binary/package/link")),
        sha256: node_string(asset.pointer("/binary/package/checksum")),
        size: asset.pointer("/binary/package/size").and_then(Value::as_i64),
    }
}

/// Adoptium os 参数（源：`MapAdoptiumOs`，逐字保留）。
fn map_adoptium_os(platform: JavaPlatform) -> &'static str {
    match platform {
        JavaPlatform::Windows => "windows",
        JavaPlatform::Linux => "linux",
        JavaPlatform::MacOS => "mac",
    }
}

/// Adoptium architecture 参数（源：`MapAdoptiumArchitecture`，逐字保留）。
fn map_adoptium_architecture(architecture: JavaArchitecture) -> &'static str {
    match architecture {
        JavaArchitecture::X64 => "x64",
        JavaArchitecture::Arm64 => "aarch64",
    }
}

/// Adoptium image_type 参数（源：`MapAdoptiumImageType`，逐字保留）。
fn map_adoptium_image_type(package_type: JavaPackageType) -> &'static str {
    match package_type {
        JavaPackageType::JRE => "jre",
        JavaPackageType::JDK => "jdk",
    }
}

/// 是否为可移植压缩包（源：`IsPortablePackage`）。
///
/// null/空白 → false；`Trim().ToLowerInvariant()` 后按平台判后缀（源
/// EndsWith(Ordinal) 大小写无关在降序后等价 ends_with）：
/// Windows → `.zip`；Linux → `.tar.gz`；MacOS → `.tar.gz` 或 `.zip`。
fn is_portable_package(file_name: Option<&str>, platform: JavaPlatform) -> bool {
    let Some(name) = file_name else {
        return false;
    };
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    match platform {
        JavaPlatform::Windows => lower.ends_with(".zip"),
        JavaPlatform::Linux => lower.ends_with(".tar.gz"),
        JavaPlatform::MacOS => lower.ends_with(".tar.gz") || lower.ends_with(".zip"),
    }
}

/// 解析 Zulu 响应并选"最新"包（源：`ParseZuluResponse`，静态）。
///
/// 源排序链：
/// `Where(IsMatchingZuluPackage && HasZuluOrderingFields)`
/// `.OrderByDescending(java_version)`（ToComparableVersion）
/// `.ThenByDescending(distro_version)`（ToComparableVersion）
/// `.ThenByDescending(openjdk_build_number ?? int.MinValue)`
/// `.FirstOrDefault()`。
/// 排序键任一步解析失败（源 GetValue<int> 抛 JsonException，惰性求值发生在
/// FirstOrDefault 处）→ 整个调用失败 → 源 catch → null → None。
/// 同键取原始顺序首个：源 OrderBy 稳定排序 + FirstOrDefault →
/// Rust 遍历"严格大于才替换"，平键保留首个。
fn parse_zulu_response(
    json: &str,
    major_version: i32,
    platform: JavaPlatform,
    architecture: JavaArchitecture,
    package_type: JavaPackageType,
) -> Option<JavaPackageInfo> {
    let root: Value = serde_json::from_str(json).ok()?;
    let packages = root.as_array()?;

    let mut best: Option<&Value> = None;
    let mut best_key: Option<(String, String, i32)> = None;
    for pkg in packages {
        if !is_matching_zulu_package(pkg, platform) || !has_zulu_ordering_fields(pkg) {
            continue;
        }
        let key = zulu_sort_key(pkg).ok()?;
        if best_key.as_ref().is_none_or(|k| key > *k) {
            best_key = Some(key);
            best = Some(pkg);
        }
    }

    let matched = best?;

    // 源：matched["java_version"] is JsonArray versionParts
    //   ? string.Join('.', versionParts.Select(part => part!.ToString())) : string.Empty
    let java_version = match matched.get("java_version") {
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| node_string(Some(part)))
            .collect::<Vec<_>>()
            .join("."),
        _ => String::new(),
    };

    Some(JavaPackageInfo {
        major_version,
        full_version: java_version,
        build: node_string(matched.get("openjdk_build_number")),
        platform,
        architecture,
        package_type,
        source: JavaDownloadSource::Zulu,
        file_name: node_string(matched.get("name")),
        download_url: node_string(matched.get("download_url")),
        sha256: String::new(),
        size: None,
    })
}

/// Zulu 包匹配（源：`IsMatchingZuluPackage`）。
///
/// 包名须为可移植压缩包（IsPortablePackage），且小写文件名不含
/// `-fx-`（Fx 版）、`-crac-`（CRaC 版）、`_musl_`（musl 构建）——
/// 源 `Contains(Ordinal)` 后对小写串判包含，逐字保留（注意源此处未 Trim，
/// 与 IsPortablePackage 内部的 Trim 不同）。
fn is_matching_zulu_package(pkg: &Value, platform: JavaPlatform) -> bool {
    let file_name = node_string(pkg.get("name"));
    if !is_portable_package(Some(file_name.as_str()), platform) {
        return false;
    }
    let lower = file_name.to_lowercase();
    !lower.contains("-fx-") && !lower.contains("-crac-") && !lower.contains("_musl_")
}

/// Zulu 排序前置字段校验（源：`HasZuluOrderingFields`）。
///
/// `java_version` / `distro_version` 须为非空 JSON 数组，且 `download_url`
/// 非空白（`??` 与 IsNullOrWhiteSpace 语义 → 缺失/JSON null → 空白）。
fn has_zulu_ordering_fields(pkg: &Value) -> bool {
    matches!(pkg.get("java_version"), Some(Value::Array(a)) if !a.is_empty())
        && matches!(pkg.get("distro_version"), Some(Value::Array(d)) if !d.is_empty())
        && !node_string(pkg.get("download_url")).trim().is_empty()
}

/// Zulu 排序键（源：OrderByDescending + ThenByDescending 的三键元组）。
///
/// `java_version` / `distro_version` → to_comparable_version；
/// `openjdk_build_number` → `?.GetValue<int>() ?? int.MinValue`——缺失或
/// JSON null → int.MinValue；存在但非整数 → 失败（源 GetValue<int> 抛异常）。
fn zulu_sort_key(pkg: &Value) -> Result<(String, String, i32), ()> {
    let java_version = to_comparable_version(pkg.get("java_version"))?;
    let distro_version = to_comparable_version(pkg.get("distro_version"))?;
    let build = match pkg.get("openjdk_build_number") {
        Some(v) if v.is_null() => i32::MIN,
        Some(v) => v.as_i64().map(|n| n as i32).ok_or(())?,
        None => i32::MIN,
    };
    Ok((java_version, distro_version, build))
}

/// 版本分量数组 → 可比较字符串（源：`ToComparableVersion`）。
///
/// 每个分量按 C# `{value:D8}` 十进制 8 位零填充后点号连接（Rust `{:08}` 等价，
/// 负号处理一致）；缺失/非数组/空数组 → 空串（源 `as JsonArray` 为 null 或
/// Count == 0 → string.Empty）；分量非整数 → 失败（源 GetValue<int> 抛
/// JsonException → 整体失败）。
fn to_comparable_version(value: Option<&Value>) -> Result<String, ()> {
    let Some(parts) = value.and_then(Value::as_array) else {
        return Ok(String::new());
    };
    if parts.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        let n = part.as_i64().map(|n| n as i32).ok_or(())?;
        out.push_str(&format!("{n:08}"));
    }
    Ok(out)
}

