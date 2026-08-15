//! CurseForge API 客户端（B13，对应源文件：Services/Expansion/CurseForge/CurseForgeBase.cs）
//!
//! 简单派生类标注（仅继承 + ClassId 常量，无额外逻辑 → 不单列 Rust 类型）：
//! - `Mods.cs`（ClassId=6）、`ResourcePacks.cs`（ClassId=12）、`Worlds.cs`（ClassId=17）、
//!   `Modpacks.cs`（ClassId=4471）、`DataPacks.cs`（ClassId=6945）、`Shaders.cs`（ClassId=6552）
//!   —— 均仅声明 GameId=432/ClassId 常量并继承 CurseForgeBase；基类搜索 URL 已硬编码
//!   `gameId=432`（源亦如此，派生类常量未被基类逻辑使用），Rust 侧调用 `search` 时按
//!   `class_id=Some(...)` 传入对应 ClassId 即可。
//!
//! 端点（base = https://api.curseforge.com）：
//! - GET  {base}/v1/mods/search（SearchAsync，gameId=432 + classId/searchFilter/gameVersions/
//!   pageSize/index/categoryIds/modLoaderTypes/sortField 查询参数，逐字保留源拼接，未做 URL 编码）
//! - GET  {base}/v1/mods/{id}（GetModInfoAsync）
//! - GET  {base}/v1/mods/{modId}/files/{fileId}（GetFileInfoAsync）
//! - GET  {base}/v1/mods/{id}/files/{fileId}/download-url（GetDownloadUrlAsync）
//! - POST {base}/v1/mods/files（GetFilesBatchAsync，类级方法不在接口内；每批 ≤ 100 个 fileId，
//!   body `{"fileIds":[...]}`，整批 400 时跳过继续，其余错误传播）
//! - POST {base}/v1/fingerprints（GetInfoFromHashesAsync/DictAsync，body FingerprintsRequest
//!   `{"fingerprints":[...]}`）
//!
//! 请求头：GET/POST 均带 `x-api-key` 与 `Accept: application/json`（源 GetData/PostData）；
//! POST 额外带 `User-Agent: QomicexCore/1.0` 与 30 秒超时（源 PostData 的
//! CancellationTokenSource(TimeSpan.FromSeconds(30))）。
//! 错误映射：网络/非 2xx → Error::Http（源 HttpRequestException）；JSON 解析/反序列化 →
//! Error::Http（源 JsonException，B6 语义）；参数非法 → Error::Params（源 ArgumentException）；
//! 响应缺失 → Error::Http（源 InvalidOperationException，消息注明源异常名）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::api::expansion::CurseForgeSource;
use crate::error::Error;
use crate::models::expansion::curseforge::{
    AuthorMeta, CategoryMeta, CurseForgeBatchFileInfo, CurseForgeFileInfo,
    CurseForgeFilePageItem, CurseForgeFilePageResponse, CurseForgeFingerprintFile,
    CurseForgeFingerprintMatch, CurseForgeInfo, CurseForgeSearchResponse, CurseForgeSearchResult,
    FingerprintsFilesMeta, FingerprintsRequest, ScreenshotsMeta,
};
use serde_json::Value;

/// 默认 API 基址（源：`DefaultBaseUrl`，逐字保留）
const DEFAULT_BASE_URL: &str = "https://api.curseforge.com";

/// 批量文件查询每批上限（源：`MaxBatchFileIds`）
const MAX_BATCH_FILE_IDS: usize = 100;

/// POST 请求超时（源：PostData 内 `CancellationTokenSource(TimeSpan.FromSeconds(30))`）
const POST_TIMEOUT: Duration = Duration::from_secs(30);

/// CurseForge 数据源实现（源：`internal class CurseForgeBase : ICurseForgeSource`）。
pub(crate) struct CurseForgeBase {
    /// 共享 HTTP 客户端（源：`_http` HttpClient，B4 定案 reqwest 共享 client）
    http: reqwest::Client,
    /// 请求基址（源：`_baseUrl`，`(baseUrl ?? DefaultBaseUrl).TrimEnd('/')`）
    base_url: String,
    /// API 密钥（源：`_apiKey`，作为 `x-api-key` 请求头）
    api_key: String,
}

impl CurseForgeBase {
    /// 创建 CurseForge 数据源（源：构造函数 `CurseForgeBase(HttpClient http, string apiKey,
    /// string? baseUrl = null)`；`base_url` 为 `None` 时使用默认基址）。
    pub(crate) fn new(http: reqwest::Client, api_key: String, base_url: Option<&str>) -> Self {
        Self {
            http,
            api_key,
            base_url: base_url
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    /// GET 请求并返回响应体文本（源：`GetData(string url, string key)`）。
    /// 相对 URL 自动拼接基址（源 `url.StartsWith("http") ? url : _baseUrl + url`）；
    /// 请求头 `x-api-key` + `Accept: application/json`；非 2xx 按 .NET
    /// EnsureSuccessStatusCode 的 HttpRequestException 语义报错（Error::Http）。
    ///
    /// 显式超时：与 POST 一致 30s（源 GetData 走 .NET HttpClient.Timeout 默认 100s，
    /// 此处统一用 POST_TIMEOUT 兜底，避免上游连接挂死拖垮资源中心请求）。
    async fn get_data(&self, url: &str) -> Result<String, Error> {
        let full_url = full_url(&self.base_url, url);
        let send = self
            .http
            .get(&full_url)
            .header("x-api-key", self.api_key.as_str())
            .header("Accept", "application/json")
            .header("User-Agent", "QomicexCore/1.0")
            .send();
        let response = match tokio::time::timeout(POST_TIMEOUT, send).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => return Err(http_err(e)),
            Err(_) => {
                return Err(Error::Http {
                    message: format!("[CurseForge] GET {full_url} 请求超时（30s）"),
                    status: None,
                    source: None,
                })
            }
        };
        let status = response.status();
        let body = response.text().await.map_err(http_err)?;
        if !status.is_success() {
            return Err(Error::Http {
                message: ensure_success_message(status),
                status: Some(status.as_u16()),
                source: None,
            });
        }
        Ok(body)
    }

    /// POST JSON 请求并返回响应体文本（源：`PostData(string url, string key, string jsonData)`）。
    /// 请求头在 GET 基础上额外带 `User-Agent: QomicexCore/1.0` 与 JSON 内容类型；
    /// 带 30 秒超时（源 CancellationTokenSource）；`Trace.WriteLine` 日志 → `eprintln!`
    /// （B6 约定，同 file_helper.rs）；失败日志后原样传播（源 catch → rethrow）。
    async fn post_data(&self, url: &str, json_data: &str) -> Result<String, Error> {
        let full_url = full_url(&self.base_url, url);
        eprintln!("[CurseForge] POST {full_url}");
        let sw = Instant::now();
        let send = self
            .http
            .post(&full_url)
            .header("x-api-key", self.api_key.as_str())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", "QomicexCore/1.0")
            .body(json_data.to_string())
            .send();
        let response = match tokio::time::timeout(POST_TIMEOUT, send).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                eprintln!(
                    "[CurseForge] POST {full_url} => 失败 耗时 {}ms: {e}",
                    sw.elapsed().as_millis()
                );
                return Err(http_err(e));
            }
            // 源：30s 取消令牌超时 → TaskCanceledException → catch 后 rethrow
            Err(_) => {
                eprintln!(
                    "[CurseForge] POST {full_url} => 失败 耗时 {}ms: 请求超时（30s）",
                    sw.elapsed().as_millis()
                );
                return Err(Error::Http {
                    message: format!("[CurseForge] POST {full_url} 请求超时（30s）"),
                    status: None,
                    source: None,
                });
            }
        };

        let status = response.status();
        eprintln!(
            "[CurseForge] POST {full_url} => {} 耗时 {}ms",
            status.as_u16(),
            sw.elapsed().as_millis()
        );
        // 源：sw.Stop() 后先打印成功行，再 EnsureSuccessStatusCode（非 2xx 时成功行与失败行都输出）
        if !status.is_success() {
            let message = ensure_success_message(status);
            eprintln!(
                "[CurseForge] POST {full_url} => 失败 耗时 {}ms: {message}",
                sw.elapsed().as_millis()
            );
            return Err(Error::Http {
                message,
                status: Some(status.as_u16()),
                source: None,
            });
        }

        response.text().await.map_err(|e| {
            eprintln!(
                "[CurseForge] POST {full_url} => 失败 耗时 {}ms: {e}",
                sw.elapsed().as_millis()
            );
            http_err(e)
        })
    }

    /// 批量获取文件信息（源：`GetFilesBatchAsync`，类级公开方法，不属于 ICurseForgeSource 接口）。
    ///
    /// 每批最多 100 个 fileId（`MaxBatchFileIds`）；过滤非正整数/超出 int32 范围的无效 id；
    /// 整批返回 400 时打印日志并跳过继续（源仅吞掉 `HttpRequestException` 且
    /// `StatusCode == BadRequest`，其余错误传播）。
    pub(crate) async fn get_files_batch(
        &self,
        file_ids: &[i64],
    ) -> Result<HashMap<i64, CurseForgeBatchFileInfo>, Error> {
        // 源：ArgumentNullException.ThrowIfNull(fileIds) —— Rust &[i64] 不可能为 null，跳过
        if file_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut result = HashMap::new();
        let mut i = 0usize;
        let mut batch_num = 1usize;
        while i < file_ids.len() {
            // 源：fileIds.Skip(i).Take(MaxBatchFileIds)
            let end = (i + MAX_BATCH_FILE_IDS).min(file_ids.len());
            let batch = &file_ids[i..end];

            // 过滤无效 fileId：CF API 要求正整数 int32（源注释原文）
            let valid_batch: Vec<i64> = batch
                .iter()
                .copied()
                .filter(|&id| id > 0 && id <= i32::MAX as i64)
                .collect();
            let skipped = batch.len() - valid_batch.len();
            if skipped > 0 {
                println!("[CurseForge] 批次 {batch_num} 过滤掉 {skipped} 个无效 fileId（≤0 或超出 int32 范围）");
            }
            if valid_batch.is_empty() {
                println!("[CurseForge] 批次 {batch_num} 无有效 fileId，跳过");
                i = end;
                batch_num += 1;
                continue;
            }

            println!(
                "[CurseForge] 批次 {batch_num} 有效 fileId 前5个: [{}] 总数={}",
                valid_batch.iter().take(5).map(|id| id.to_string()).collect::<Vec<_>>().join(","),
                valid_batch.len()
            );

            // 源：$"{{\"fileIds\":[{string.Join(",", validBatch)}]}}"
            let json_data = format!(
                "{{\"fileIds\":[{}]}}",
                valid_batch.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
            );

            let response_text = match self.post_data("/v1/mods/files", &json_data).await {
                Ok(text) => text,
                Err(e) if is_bad_request(&e) => {
                    println!("[CurseForge] 批次 {batch_num} 返回 400 Bad Request，跳过此批（{} 个 fileId）", valid_batch.len());
                    println!("[CurseForge] 失败批次 fileIds: {}", valid_batch.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","));
                    i = end;
                    batch_num += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };

            let root = parse_json(&response_text, "批量文件响应")?;
            let Some(array) = root.get("data").and_then(|v| v.as_array()) else {
                // 源：if (array == null) continue;
                i = end;
                batch_num += 1;
                continue;
            };

            for node in array {
                // 源：obj["id"]?.GetValue<long>() ?? 0
                let id = to_i64(node.get("id"));
                if id == 0 {
                    continue;
                }

                result.insert(
                    id,
                    CurseForgeBatchFileInfo {
                        id,
                        mod_id: to_i64(node.get("modId")),
                        file_name: opt_str_field(node.get("fileName")),
                        download_url: opt_str_field(node.get("downloadUrl")),
                        file_length: match node.get("fileLength") {
                            Some(v) if !v.is_null() => Some(to_i64(Some(v))),
                            _ => None,
                        },
                        sha1: find_sha1_from_hashes(node.get("hashes").and_then(|v| v.as_array())),
                    },
                );
            }
            i = end;
            batch_num += 1;
        }
        Ok(result)
    }

    /// 分页获取模组文件列表（对齐桌面 `ResourceCenterEndpoints.cs` 的
    /// `GET /v1/mods/{id}/files?pageSize=&index=[&gameVersion=]`）。
    ///
    /// `index` 为偏移（源 FetchPage 按 pageSize 递进）；`game_version` 非空时附加
    /// `&gameVersion=` 查询参数（源 `Uri.EscapeDataString` 编码，此处版本号无特殊字符，直拼）。
    /// 返回 `CurseForgeFilePageResponse`（files + totalCount）。
    pub(crate) async fn get_file_page(
        &self,
        mod_id: &str,
        index: i32,
        page_size: i32,
        game_version: Option<&str>,
    ) -> Result<CurseForgeFilePageResponse, Error> {
        let mut url = format!(
            "{}/v1/mods/{}/files?pageSize={}&index={}",
            self.base_url, mod_id, page_size, index
        );
        if let Some(gv) = game_version.filter(|g| !g.is_empty()) {
            url.push_str(&format!("&gameVersion={gv}"));
        }
        let data = self.get_data(&url).await?;
        let root = parse_json(&data, "CurseForge 文件列表响应")?;
        let total_count = to_i64(root.get("pagination").and_then(|p| p.get("totalCount"))) as i32;
        let files = root
            .get("data")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| serde_json::from_value(n.clone()).ok())
                    .collect::<Vec<CurseForgeFilePageItem>>()
            })
            .unwrap_or_default();
        Ok(CurseForgeFilePageResponse { files, total_count })
    }

    /// 指纹反查内部实现：返回 (指纹, 文件信息) 有序列表。
    /// 保序原因：源 `GetInfoFromHashesAsync` 取 `dict.Values.ToList()`（Dictionary 插入序 =
    /// exactMatches 数组序），Rust HashMap 无序，故保留 Vec 供列表版使用。
    async fn get_info_from_hashes_inner(
        &self,
        hashes: &[i64],
    ) -> Result<Vec<(i64, FingerprintsFilesMeta)>, Error> {
        // 源：ArgumentNullException.ThrowIfNull(hashes) —— Rust &[i64] 不可能为 null，跳过
        if hashes.is_empty() {
            return Ok(Vec::new());
        }

        // 源：JsonSerializer.Serialize(new FingerprintsRequest(hashes), JsonCtx.FingerprintsRequest)
        // （CamelCase → {"fingerprints":[...]}，B1 模型 FingerprintsRequest）
        let json_data = serde_json::to_string(&FingerprintsRequest {
            fingerprints: hashes.to_vec(),
        })
        .map_err(|e| Error::Http {
            message: "序列化 FingerprintsRequest 失败（源 JsonException）".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let data = self.post_data("/v1/fingerprints", &json_data).await?;
        let root = parse_json(&data, "指纹反查响应")?;
        // 源：data?["data"]?["exactMatches"] as JsonArray
        let Some(exact_matches) = root
            .get("data")
            .and_then(|d| d.get("exactMatches"))
            .and_then(|v| v.as_array())
        else {
            // 源：if (exactMatches == null) return [];
            return Ok(Vec::new());
        };

        let mut result = Vec::with_capacity(exact_matches.len());
        for match_node in exact_matches {
            // 源：match?["file"]，缺失或 JSON null → continue
            let Some(file_data) = match_node.get("file").filter(|v| !v.is_null()) else {
                continue;
            };
            // 源：fileData.ToObject(JsonCtx.FingerprintsFilesMeta) —— 结构非法时抛
            // JsonException 并传播（源未捕获）
            let meta: FingerprintsFilesMeta =
                serde_json::from_value(file_data.clone()).map_err(|e| Error::Http {
                    message: "解析 FingerprintsFilesMeta 失败（源 ToObject → JsonException）".to_string(),
                    status: None,
                    source: Some(Box::new(e)),
                })?;
            // 源：fileData["fileFingerprint"]?.GetValue<long>() ?? fileData["id"]?.GetValue<long>() ?? 0
            // （fileFingerprint 非空即优先，不因值为 0 而回退到 id）
            let fingerprint = match file_data.get("fileFingerprint") {
                Some(v) if !v.is_null() => to_i64(Some(v)),
                _ => to_i64(file_data.get("id")),
            };
            if fingerprint != 0 {
                result.push((fingerprint, meta));
            }
        }
        Ok(result)
    }
}

#[async_trait]
impl CurseForgeSource for CurseForgeBase {
    /// 搜索模组（源：`SearchAsync`）。
    ///
    /// 查询参数按源逐字拼接（未做 URL 编码，与源一致）：
    /// `gameId=432` 固定；`classId` / `categoryIds=[...]` 仅在指定时附加；
    /// `gameVersions=["v1","v2"]` 每个版本加引号；`modLoaderTypes=[...]` 原始拼接；
    /// `index = (page - 1) * pageSize`；`sortField` 为 `None` 时输出空值（源 int?
    /// 插值为空串）。`(page-1)*pageSize + pageSize > 10000` 时返回参数错误
    /// （源 ArgumentOutOfRangeException("PageSize cannot exceed 10,000 items.")）。
    async fn search(
        &self,
        search_filter: &str,
        game_versions: Option<&[String]>,
        categories: Option<&[Option<i32>]>,
        mod_loader_types: Option<&[String]>,
        sort_field: Option<i32>,
        page: Option<i32>,
        page_size: Option<i32>,
        class_id: Option<i32>,
    ) -> Result<CurseForgeSearchResponse, Error> {
        // 源：var p = page ?? 1; var ps = pageSize ?? 25;
        // index = ((p - 1) * ps).ToString()；用 i64 运算避免 Rust 调试构建整数溢出 panic
        // （源默认 unchecked）
        let p = page.unwrap_or(1);
        let ps = page_size.unwrap_or(25);
        let index = (p as i64 - 1) * ps as i64;
        if index + ps as i64 > 10000 {
            return Err(Error::Params {
                message: "PageSize cannot exceed 10,000 items.".to_string(),
                source: None,
            });
        }

        // 源：modLoaders = string.Join(",", modLoaderTypes ?? [])
        let mod_loaders = mod_loader_types
            .into_iter()
            .flatten()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",");
        // 源：categories is { Length: > 0 } ? $"&categoryIds=[{string.Join(",", categories)}]" : ""；
        // int? 元素为 null 时插值为空串（C# 可空值类型插值语义）
        let cats = categories
            .filter(|c| !c.is_empty())
            .map(|c| {
                format!(
                    "&categoryIds=[{}]",
                    c.iter()
                        .map(|v| v.map(|x| x.to_string()).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .unwrap_or_default();
        // 源：versions = string.Join(",", (gameVersions ?? []).Select(v => $"\"{v}\""))
        let versions = game_versions
            .into_iter()
            .flatten()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(",");
        // 源：cls = classId.HasValue ? $"&classId={classId.Value}" : ""
        let cls = class_id.map(|v| format!("&classId={v}")).unwrap_or_default();
        // 源：$"{sortField}" —— int? 为 null 时插值为空串
        let sort = sort_field.map(|v| v.to_string()).unwrap_or_default();

        let url = format!(
            "{}/v1/mods/search?gameId=432{}&searchFilter={}&sortOrder=desc&gameVersions=[{}]&pageSize={}&index={}{}&modLoaderTypes=[{}]&sortField={}",
            self.base_url, cls, search_filter, versions, ps, index, cats, mod_loaders, sort
        );

        let data = self.get_data(&url).await?;
        let root = parse_json(&data, "CurseForge 搜索响应")?;
        // 源：root?["pagination"]?["totalCount"]?.GetValue<int>() ?? 0
        let total_count = to_i64(root.get("pagination").and_then(|p| p.get("totalCount"))) as i32;
        let Some(array) = root.get("data").and_then(|v| v.as_array()) else {
            // 源：if (result == null) return new CurseForgeSearchResponse([], 0);
            return Ok(CurseForgeSearchResponse {
                results: Vec::new(),
                total_count: 0,
            });
        };

        let mut results = Vec::with_capacity(array.len());
        for mod_data in array {
            // 源：mod!.AsObject() —— 非对象时 InvalidOperationException → Error::Http
            let Some(mod_obj) = mod_data.as_object() else {
                return Err(Error::Http {
                    message: "搜索结果为非对象（源 AsObject() → InvalidOperationException）".to_string(),
                    status: None,
                    source: None,
                });
            };

            // 源：latestFilesIndexes? .Select(n => n?["gameVersion"]?.ToString())
            //      .OfType<string>().Distinct().OrderBy(v => v).ToList() ?? []
            // （缺失/JSON null 过滤；空字符串保留；Distinct+OrderBy → sort + dedup 等价）
            let mut game_versions_list: Vec<String> = mod_obj
                .get("latestFilesIndexes")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|n| match n.get("gameVersion") {
                    Some(v) if !v.is_null() => Some(node_to_string(v)),
                    _ => None,
                })
                .collect();
            game_versions_list.sort();
            game_versions_list.dedup();

            results.push(CurseForgeSearchResult {
                id: str_field(mod_obj.get("id")),
                name: str_field(mod_obj.get("name")),
                slug: str_field(mod_obj.get("slug")),
                summary: str_field(mod_obj.get("summary")),
                status: str_field(mod_obj.get("status")),
                // 源：GameVersion = string.Join(", ", gameVersionsList)
                game_version: game_versions_list.join(", "),
                download_count: str_field(mod_obj.get("downloadCount")),
                // 源：modData["logo"]?["url"]?.ToString() ?? ""
                icon_url: mod_obj
                    .get("logo")
                    .and_then(|l| l.as_object())
                    .map(|logo| str_field(logo.get("url")))
                    .unwrap_or_default(),
                // 源：modData["isFeatured"]?.GetValue<bool>() ?? false
                is_featured: mod_obj.get("isFeatured").and_then(|v| v.as_bool()).unwrap_or(false),
                // 源：ParseJsonArray(authors, n => new AuthorMeta(n?["id"]?.GetValue<int>() ?? 0,
                //      n?["name"]?.ToString() ?? "", n?["url"]?.ToString()))
                authors: mod_obj
                    .get("authors")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .map(|n| AuthorMeta {
                        id: to_i64(n.get("id")) as i32,
                        name: str_field(n.get("name")),
                        url: opt_str_field(n.get("url")),
                    })
                    .collect(),
                // 源：ParseJsonArray(categories, n => new CategoryMeta((int)(n?["id"] ?? 0),
                //      n?["name"]?.ToString() ?? "", n?["slug"]?.ToString(), n?["url"]?.ToString()))
                categories: mod_obj
                    .get("categories")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .map(|n| CategoryMeta {
                        id: to_i64(n.get("id")) as i32,
                        name: str_field(n.get("name")),
                        slug: opt_str_field(n.get("slug")),
                        url: opt_str_field(n.get("url")),
                    })
                    .collect(),
                // 源：ParseJsonArray(screenshots, n => new ScreenshotsMeta((int)(n?["id"] ?? 0),
                //      (int)(n?["modId"] ?? 0), n?["title"]?.ToString(), n?["description"]?.ToString(),
                //      n?["thumbnailUrl"]?.ToString(), n?["url"]?.ToString()))
                screenshots: mod_obj
                    .get("screenshots")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .map(|n| ScreenshotsMeta {
                        id: to_i64(n.get("id")) as i32,
                        mod_id: to_i64(n.get("modId")) as i32,
                        title: opt_str_field(n.get("title")),
                        description: opt_str_field(n.get("description")),
                        thumbnail_url: opt_str_field(n.get("thumbnailUrl")),
                        url: opt_str_field(n.get("url")),
                    })
                    .collect(),
            });
        }
        Ok(CurseForgeSearchResponse {
            results,
            total_count,
        })
    }

    /// 获取模组详情（源：`GetModInfoAsync`）。
    /// 空 id → Error::Params（源 ArgumentException.ThrowIfNullOrEmpty）；响应无 `data` →
    /// Error::Http（源 InvalidOperationException("CurseForge 响应为空")）；
    /// `data` 反序列化为 B1 模型 CurseForgeInfo（源 ToObject，非法 → JsonException）。
    async fn get_mod_info(&self, id: &str) -> Result<CurseForgeInfo, Error> {
        if id.is_empty() {
            return Err(Error::Params {
                message: "模组 id 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）".to_string(),
                source: None,
            });
        }
        let data = self
            .get_data(&format!("{}/v1/mods/{}", self.base_url, id))
            .await?;
        let root = parse_json(&data, "模组详情响应")?;
        let Some(result) = root.get("data").filter(|v| !v.is_null()) else {
            return Err(Error::Http {
                message: "CurseForge 响应为空（源 InvalidOperationException）".to_string(),
                status: None,
                source: None,
            });
        };
        serde_json::from_value(result.clone()).map_err(|e| Error::Http {
            message: "解析 CurseForgeInfo 失败（源 ToObject → JsonException）".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 获取模组文件信息（源：`GetFileInfoAsync`）。
    /// 空参数 → Error::Params（源 ArgumentException.ThrowIfNullOrEmpty）；响应无 `data` →
    /// Error::Http（源 InvalidOperationException("CurseForge 文件信息响应为空")）；
    /// `data` 反序列化为 B1 模型 CurseForgeFileInfo。
    async fn get_file_info(&self, mod_id: &str, file_id: &str) -> Result<CurseForgeFileInfo, Error> {
        if mod_id.is_empty() {
            return Err(Error::Params {
                message: "modId 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）".to_string(),
                source: None,
            });
        }
        if file_id.is_empty() {
            return Err(Error::Params {
                message: "fileId 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）".to_string(),
                source: None,
            });
        }
        let data = self
            .get_data(&format!("{}/v1/mods/{}/files/{}", self.base_url, mod_id, file_id))
            .await?;
        let root = parse_json(&data, "文件信息响应")?;
        let Some(result) = root.get("data").filter(|v| !v.is_null()) else {
            return Err(Error::Http {
                message: "CurseForge 文件信息响应为空（源 InvalidOperationException）".to_string(),
                status: None,
                source: None,
            });
        };
        serde_json::from_value(result.clone()).map_err(|e| Error::Http {
            message: "解析 CurseForgeFileInfo 失败（源 ToObject → JsonException）".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })
    }

    /// 获取文件下载地址（源：`GetDownloadUrlAsync`）。
    /// 空参数 → Error::Params（源 ArgumentException）；响应 `data` 缺失/为 null →
    /// Error::Http（源 InvalidOperationException("提取下载链接失败")）。
    async fn get_download_url(&self, id: &str, file_id: &str) -> Result<String, Error> {
        if id.is_empty() {
            return Err(Error::Params {
                message: "id 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）".to_string(),
                source: None,
            });
        }
        if file_id.is_empty() {
            return Err(Error::Params {
                message: "fileId 不能为空（源 ArgumentException.ThrowIfNullOrEmpty）".to_string(),
                source: None,
            });
        }
        let data = self
            .get_data(&format!(
                "{}/v1/mods/{}/files/{}/download-url",
                self.base_url, id, file_id
            ))
            .await?;
        let root = parse_json(&data, "下载链接响应")?;
        // 源：JsonNode.Parse(data)?["data"]?.ToString()
        //      ?? throw new InvalidOperationException("提取下载链接失败")
        root.get("data")
            .filter(|v| !v.is_null())
            .map(node_to_string)
            .ok_or_else(|| Error::Http {
                message: "提取下载链接失败（源 InvalidOperationException）".to_string(),
                status: None,
                source: None,
            })
    }

    /// 通过指纹反查文件信息（源：`GetInfoFromHashesAsync`，内部委托
    /// GetInfoFromHashesDictAsync 后取 `dict.Values.ToList()`）。
    /// 顺序保持 exactMatches 数组序（C# Dictionary 插入序 → .Values 序；HashMap 无序，
    /// 故内部以保序 Vec 承载，见 get_info_from_hashes_inner）。
    async fn get_info_from_hashes(&self, hashes: &[i64]) -> Result<Vec<FingerprintsFilesMeta>, Error> {
        Ok(self
            .get_info_from_hashes_inner(hashes)
            .await?
            .into_iter()
            .map(|(_, meta)| meta)
            .collect())
    }

    /// 通过指纹反查文件信息，返回 指纹 → 文件信息 映射
    /// （源：`GetInfoFromHashesDictAsync` → Dictionary<long, FingerprintsFilesMeta>）。
    /// body 为 FingerprintsRequest（`{"fingerprints":[...]}`）；键取
    /// `file.fileFingerprint ?? file.id`，为 0 时跳过。
    async fn get_info_from_hashes_dict(
        &self,
        hashes: &[i64],
    ) -> Result<HashMap<i64, FingerprintsFilesMeta>, Error> {
        Ok(self.get_info_from_hashes_inner(hashes).await?.into_iter().collect())
    }

    /// 通过指纹反查完整命中（file + latestFiles，用于批次更新检查）。
    /// body 同 FingerprintsRequest（`{"fingerprints":[...]}`）；键取
    /// `file.fileFingerprint ?? file.id`，为 0 时跳过；file 缺失 → None，
    /// latestFiles 缺失 → 空列表。
    async fn get_fingerprint_matches(
        &self,
        hashes: &[i64],
    ) -> Result<Vec<CurseForgeFingerprintMatch>, Error> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }

        let json_data = serde_json::to_string(&FingerprintsRequest {
            fingerprints: hashes.to_vec(),
        })
        .map_err(|e| Error::Http {
            message: "序列化 FingerprintsRequest 失败（源 JsonException）".to_string(),
            status: None,
            source: Some(Box::new(e)),
        })?;
        let data = self.post_data("/v1/fingerprints", &json_data).await?;
        let root = parse_json(&data, "指纹反查响应")?;
        let Some(exact_matches) = root
            .get("data")
            .and_then(|d| d.get("exactMatches"))
            .and_then(|v| v.as_array())
        else {
            return Ok(Vec::new());
        };

        let mut result = Vec::with_capacity(exact_matches.len());
        for match_node in exact_matches {
            let file_raw = match_node.get("file").filter(|v| !v.is_null());
            let file: Option<CurseForgeFingerprintFile> = match file_raw {
                Some(v) => Some(
                    serde_json::from_value(v.clone()).map_err(|e| Error::Http {
                        message: "解析 CurseForgeFingerprintFile 失败".to_string(),
                        status: None,
                        source: Some(Box::new(e)),
                    })?,
                ),
                None => None,
            };
            let fingerprint = match file_raw {
                Some(v) => match v.get("fileFingerprint") {
                    Some(fp) if !fp.is_null() => to_i64(Some(fp)),
                    _ => to_i64(v.get("id")),
                },
                None => to_i64(match_node.get("fingerprint")),
            };
            let latest_files: Vec<CurseForgeFingerprintFile> = match_node
                .get("latestFiles")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|n| serde_json::from_value(n.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            if fingerprint != 0 {
                result.push(CurseForgeFingerprintMatch {
                    fingerprint,
                    file,
                    latest_files,
                });
            }
        }
        Ok(result)
    }
}

// ── 源 private static 辅助函数 ─────────────────────────

/// 源：`url.StartsWith("http") ? url : _baseUrl + url`（相对路径拼接基址）
fn full_url(base: &str, url: &str) -> String {
    if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{base}{url}")
    }
}

/// 网络/发送错误映射（源：HttpRequestException → Error::Http）
fn http_err(e: reqwest::Error) -> Error {
    Error::Http {
        message: format!("HTTP 请求失败: {e}"),
        status: None,
        source: Some(Box::new(e)),
    }
}

/// .NET EnsureSuccessStatusCode 消息格式（源：`Response status code does not indicate
/// success: 400 (Bad Request).`，与 services/version/manifest.rs 既有惯例一致）
fn ensure_success_message(status: reqwest::StatusCode) -> String {
    format!(
        "Response status code does not indicate success: {} ({}).",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
}

/// 判断错误是否为 400 Bad Request（源：`catch (HttpRequestException ex) when
/// (ex.StatusCode == System.Net.HttpStatusCode.BadRequest)`）。
/// ⚠️ 源依赖异常上的 StatusCode；Rust 侧 Error::Http（B6 增补）无状态码字段，
/// 通过本模块 ensure_success_message 生成的固定消息文本匹配（见 p56 翻译日志）。
fn is_bad_request(e: &Error) -> bool {
    // TD-1：Error::Http 已结构化承载状态码（源 HttpRequestException.StatusCode 语义）
    matches!(e, Error::Http { status: Some(400), .. })
}

/// 解析 JSON 文本（源：`JsonNode.Parse` —— 非法 JSON → JsonException → Error::Http，B6 语义）
fn parse_json(data: &str, what: &str) -> Result<Value, Error> {
    serde_json::from_str(data).map_err(|e| Error::Http {
        message: format!("解析 {what} 失败（源 JsonException）"),
        status: None,
        source: Some(Box::new(e)),
    })
}

/// 源 `JsonNode.ToString()`：字符串 → 原文；数字/布尔 → JSON 字面量文本
fn node_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 源 `obj[key]?.ToString() ?? ""`（缺失或 JSON null → 空串）
fn str_field(v: Option<&Value>) -> String {
    match v {
        Some(v) if !v.is_null() => node_to_string(v),
        _ => String::new(),
    }
}

/// 源 `obj[key]?.ToString()`（缺失或 JSON null → None）
fn opt_str_field(v: Option<&Value>) -> Option<String> {
    match v {
        Some(v) if !v.is_null() => Some(node_to_string(v)),
        _ => None,
    }
}

/// 源 `GetValue<long>()`：数字或可解析为数字的字符串 → 值，否则 0
/// （缺失/JSON null 经索引语义（`?.`）已为 None → 0；类型不匹配时源抛
/// JsonException，此处按容错取 0 —— ⚠️ 微差，见 p56 翻译日志）
fn to_i64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// 从 hashes 数组中取第一个 `algo == 1`（SHA1）的 value（源：`FindSha1FromHashes`）。
/// 命中项 value 缺失/为 null 时返回 None（与源 return 行为一致，不再继续找）。
fn find_sha1_from_hashes(hashes: Option<&Vec<Value>>) -> Option<String> {
    for hash in hashes? {
        if to_i64(hash.get("algo")) == 1 {
            return opt_str_field(hash.get("value"));
        }
    }
    None
}






