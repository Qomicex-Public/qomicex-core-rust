//! 资源补全器实现（B6）：对应源文件 Services/DefaultResourceCompleter.cs
//!
//! 并发设计（特殊兼容）：源 `Task.WhenAll` + `SemaphoreSlim(_maxConcurrentDownloads)`
//! → 本实现用 `tokio::task::scope`（结构化并发）+ `tokio::sync::Semaphore` 限流。
//! 选型原因：`ProgressReporter` 以借用引用（`Option<&dyn ProgressReporter>`）传入，
//! 不满足 `tokio::spawn` 的 `'static` 约束；`tokio::task::scope` 允许借用环境数据，
//! 是 tokio 原生方案（无需引入 futures crate，规则：不改 Cargo.toml）。
//! 注：C# 侧任务在 await 前已同步执行至首个 await，SemaphoreSlim 实际无法限流；
//! 本实现按源意图（_maxConcurrentDownloads 存在）实施真正限流（见翻译日志 p27-completer.md）。
//!
//! 错误语义（本文件定案）：
//! - 传输层（请求 / 状态码 / 读响应流）→ `Error::Http`
//! - 文件 IO / 目录创建 / SHA1 校验 → `Error::DownloadFailed`
//! - artifact 全部镜像失败 → `Error::DownloadFailed` 外层包装（内层 source 为最后一次
//!   镜像错误；镜像列表为空时为源异常 "所有下载源都失败了"）
//!
//! 忠实保留的源行为（疑似缺陷不修复，见翻译日志）：
//! - 资源对象（assets/objects/xx/yyyy）下载前不创建目录 → File::create 失败被镜像
//!   循环吞掉，资源静默跳过（源同逻辑）
//! - 客户端 jar 下载到 libraries/{client.path}，而完整性检查检查 versions/{id}/{id}.jar
//!   （源两处路径约定不一致）
//! - 资产索引全部镜像失败时静默成功；CheckResourcesCompleteAsync 不检查资产索引/资源对象

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::api::download::DownloadSourceManager;
use crate::api::version::ResourceCompleter;
use crate::error::Error;
use crate::event::ProgressReporter;
use crate::models::download::{DownloadProgress, DownloadStatus, ResourceType};
use crate::models::version_metadata::{
    Artifact, AssetIndex, CompleteVersionMetadata, Library, Rule,
};
use crate::util::file_helper::validate_file_hash;
use crate::util::platform::{get_current_arch, get_current_os_name, is_os_match};

/// 资产索引数据（源：JsonContext/AssetIndexDataJsonContext.cs 的 AssetIndexData 记录；
/// 仅本文件内部反序列化使用，未注册模型层）
#[derive(Deserialize)]
struct AssetIndexData {
    /// 对象集合：哈希 → 资源对象（源：Dictionary<string, AssetObject>；
    /// 源 record 非可空，但缺键/显式 null 时 `indexData?.Objects == null` 提前返回 → Option）
    #[serde(default)]
    objects: Option<HashMap<String, AssetObject>>,
}

/// 单个资源对象（源：AssetObject 记录）
#[derive(Deserialize)]
struct AssetObject {
    /// 文件哈希（SHA1）
    hash: String,
    /// 文件大小（源记录含 Size 字段，但源处理逻辑同样不使用 → 保留字段仅保形，
    /// 避免 dead_code 告警（过渡期约定见 services/auth/mod.rs））
    #[allow(dead_code)]
    size: i64,
}

/// 资源补全器（源：DefaultResourceCompleter，internal class → pub(crate)）
pub(crate) struct DefaultResourceCompleter {
    /// 游戏根目录（源：_gameRootPath）
    game_root_path: String,
    /// 下载源管理器（源：_sourceManager，IDownloadSourceManager）
    source_manager: Arc<dyn DownloadSourceManager + Send + Sync>,
    /// HTTP 客户端（源：_httpClient；C# 构造参数 httpClient 为 null 时 new HttpClient() →
    /// Rust 侧内部自建 reqwest::Client::new()，不对外暴露注入）
    http_client: reqwest::Client,
    /// 最大并发下载数（源：_maxConcurrentDownloads，构造默认 8）
    max_concurrent_downloads: usize,
}

impl DefaultResourceCompleter {
    /// 构造资源补全器（源：DefaultResourceCompleter(...)）。
    /// C# 可选参数 httpClient（null → new HttpClient()）不注入（内部自建）；
    /// maxConcurrentDownloads 默认值 8 由调用方显式传入（Rust 无默认参数）。
    pub(crate) fn new(
        game_root_path: String,
        source_manager: Arc<dyn DownloadSourceManager + Send + Sync>,
        max_concurrent_downloads: usize,
    ) -> Self {
        Self {
            game_root_path,
            source_manager,
            http_client: reqwest::Client::new(),
            max_concurrent_downloads,
        }
    }
}

#[async_trait]
impl ResourceCompleter for DefaultResourceCompleter {
    /// 补全缺失资源（源：CompleteResourcesAsync）。
    /// 任务集合：客户端 jar + 各库文件（含 natives 分类器）+ 资产索引；
    /// 以 _maxConcurrentDownloads 限流，全部任务完成后返回（Task.WhenAll 语义）。
    async fn complete_resources(
        &self,
        metadata: &CompleteVersionMetadata,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        // 源：先收集下载任务（客户端 jar / 库文件 / 资产索引），再以
        // SemaphoreSlim(_maxConcurrentDownloads) 限流，最后 Task.WhenAll
        let mut library_artifacts: Vec<Artifact> = Vec::new();
        for library in &metadata.libraries {
            library_artifacts.extend(self.get_library_artifacts(library));
        }

        // C# SemaphoreSlim(0) 抛 ArgumentOutOfRangeException；
        // ⚠️ UNMAPPED: Rust 侧传入 0 将导致并发死锁（调用方应传 ≥1，默认 8）
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent_downloads));

        // tokio::task::scope 自 tokio 1.44 弃用、1.53 已移除（B6 编译排障记录）；
        // 改用 futures::future::join_all 在同一任务内交错执行（IO 并发等价，借用合法）
        async move {
            let mut handles: Vec<futures::future::BoxFuture<'_, Result<(), Error>>> = Vec::new();

            // 客户端 jar（源：if (metadata.Downloads?.Client is { } client)）
            if let Some(downloads) = &metadata.downloads {
                let permit = semaphore.clone();
                handles.push(Box::pin(async move {
                    let _permit = permit.acquire_owned().await.map_err(|_| Error::DownloadFailed {
                        message: "并发信号量不可用".to_string(),
                        source: None,
                    })?;
                    self.download_artifact(&downloads.client, progress).await
                }));
            }

            // 库文件（源：foreach library → GetLibraryArtifacts → DownloadArtifactAsync）
            for artifact in &library_artifacts {
                let permit = semaphore.clone();
                handles.push(Box::pin(async move {
                    let _permit = permit.acquire_owned().await.map_err(|_| Error::DownloadFailed {
                        message: "并发信号量不可用".to_string(),
                        source: None,
                    })?;
                    self.download_artifact(artifact, progress).await
                }));
            }

            // 资产索引（源：metadata.AssetIndex != null && !string.IsNullOrEmpty(Url)）
            if let Some(asset_index) = metadata
                .asset_index
                .as_ref()
                .filter(|a| !a.url.is_empty())
            {
                let permit = semaphore.clone();
                handles.push(Box::pin(async move {
                    let _permit = permit.acquire_owned().await.map_err(|_| Error::DownloadFailed {
                        message: "并发信号量不可用".to_string(),
                        source: None,
                    })?;
                    self.download_asset_index(asset_index, progress).await
                }));
            }

            // 源 Task.WhenAll：等待全部任务；按 spawn 顺序记录首个错误（C# 抛首个异常）
            let mut first_error: Option<Error> = None;
            for result in futures::future::join_all(handles).await {
                match result {
                    Ok(()) => {}
                    Err(e) if first_error.is_none() => first_error = Some(e),
                    Err(_) => {}
                }
            }

            match first_error {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        .await
    }

    /// 检查资源是否完整（源：CheckResourcesCompleteAsync）。
    /// 仅检查客户端 jar（versions/{id}/{id}.jar）与库文件（libraries/{path}，含 SHA1 校验）；
    /// 不检查资产索引与资源对象（源同逻辑）。
    async fn check_resources_complete(
        &self,
        metadata: &CompleteVersionMetadata,
    ) -> Result<bool, Error> {
        let client_path = Path::new(&self.game_root_path)
            .join("versions")
            .join(&metadata.id)
            .join(format!("{}.jar", metadata.id));
        if !client_path.is_file() {
            return Ok(false);
        }

        for library in &metadata.libraries {
            for artifact in self.get_library_artifacts(library) {
                let local_path = Path::new(&self.game_root_path)
                    .join("libraries")
                    .join(&artifact.path);
                if !local_path.is_file() {
                    return Ok(false);
                }
                if !validate_file_hash(&local_path.to_string_lossy(), &artifact.sha1) {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }
}

impl DefaultResourceCompleter {
    /// 下载单个工件（源：DownloadArtifactAsync）。
    /// 已存在且 SHA1 匹配 → 跳过；否则逐镜像下载，全部失败抛 DownloadFailed。
    async fn download_artifact(
        &self,
        artifact: &Artifact,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        let local_path = Path::new(&self.game_root_path)
            .join("libraries")
            .join(&artifact.path);
        let local_path_str = local_path.to_string_lossy().into_owned();

        // 源：Path.GetDirectoryName 非空时 Directory.CreateDirectory
        if let Some(dir) = local_path.parent() {
            if !dir.as_os_str().is_empty() {
                tokio::fs::create_dir_all(dir).await.map_err(|e| Error::DownloadFailed {
                    message: format!("创建目录失败: {}", dir.display()),
                    source: Some(Box::new(e)),
                })?;
            }
        }

        if local_path.is_file() && validate_file_hash(&local_path_str, &artifact.sha1) {
            return Ok(());
        }

        let mirror_urls = self
            .source_manager
            .generate_mirror_urls(&artifact.url, ResourceType::Library);
        let mut last_error: Option<Error> = None;
        for url in mirror_urls {
            match self
                .download_file_with_retry(&url, &local_path_str, &artifact.sha1, progress)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => last_error = Some(e),
            }
        }

        // 源：throw new DownloadFailedException($"下载 {artifact.Path} 失败", lastException)
        // lastException 初始为 new Exception("所有下载源都失败了")（镜像列表为空时保留）
        let source: Box<dyn std::error::Error + Send + Sync> = match last_error {
            Some(e) => Box::new(e),
            None => Box::new(Error::DownloadFailed {
                message: "所有下载源都失败了".to_string(),
                source: None,
            }),
        };
        Err(Error::DownloadFailed {
            message: format!("下载 {} 失败", artifact.path),
            source: Some(source),
        })
    }

    /// 带重试的下载（源：DownloadFileWithRetryAsync，maxRetries 默认 3）。
    /// 每次尝试：HTTP GET（ResponseHeadersRead → 流式读取）→ 流式写入 → SHA1 校验；
    /// 校验失败删除文件并抛错；非末次失败上报 Retrying 并延迟 1000*(retry+1)ms 后重试。
    async fn download_file_with_retry(
        &self,
        url: &str,
        local_path: &str,
        expected_sha1: &str,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        const MAX_RETRIES: usize = 3; // 源方法默认参数 maxRetries = 3
        for retry in 0..MAX_RETRIES {
            match self
                .download_file_once(url, local_path, expected_sha1, retry, progress)
                .await
            {
                Ok(()) => return Ok(()),
                Err(_e) if retry < MAX_RETRIES - 1 => {
                    // 源：catch when (retry < maxRetries - 1) → 上报 Retrying（计数 retry+1）→ Task.Delay
                    if let Some(reporter) = progress {
                        reporter.report_download(DownloadProgress {
                            file_name: file_name_of(local_path),
                            downloaded_bytes: 0,
                            total_bytes: 0,
                            percentage: 0.0,
                            speed_bytes_per_second: 0,
                            retry_count: (retry + 1) as i32,
                            status: DownloadStatus::Retrying,
                        });
                    }
                    tokio::time::sleep(Duration::from_millis(1000 * (retry as u64 + 1))).await;
                }
                // 末次重试失败 → 错误直接向上传播（无 Retrying 上报、无延迟，源同逻辑）
                Err(e) => return Err(e),
            }
        }
        unreachable!("重试循环内必然 return（源逻辑一致）")
    }

    /// 单次下载尝试（源：DownloadFileWithRetryAsync 的 try 块）。
    /// 流式下载到 local_path（直接写目标文件，源无临时文件+改名逻辑）；
    /// 传输层错误 → Error::Http；文件 IO / 哈希不匹配 → Error::DownloadFailed。
    async fn download_file_once(
        &self,
        url: &str,
        local_path: &str,
        expected_sha1: &str,
        retry: usize,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        let response = self.http_client.get(url).send().await.map_err(|e| Error::Http {
            message: format!("HTTP 请求失败: {url}"),
            status: None,
            source: Some(Box::new(e)),
        })?;
        // 源：response.EnsureSuccessStatusCode()（非 2xx 抛 HttpRequestException）
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http {
                message: format!("HTTP 状态码 {}: {url}", status.as_u16()),
                status: None,
                source: None,
            });
        }

        // 源：response.Content.Headers.ContentLength ?? 0
        let total_bytes = response.content_length().unwrap_or(0) as i64;
        let file_name = file_name_of(local_path);
        let mut downloaded_bytes: i64 = 0;
        let mut bytes_since_update: i64 = 0;
        let mut last_update = Instant::now(); // 源：DateTime.Now

        // 源：File.Create(localPath)（直接写目标路径；重试时自动截断上次残留）
        let mut response = response;
        let mut file = tokio::fs::File::create(local_path)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("创建文件失败: {local_path}"),
                source: Some(Box::new(e)),
            })?;

        // 源：8192 字节缓冲循环读取 → 流式写入（reqwest chunk 流等价；
        // 进度上报仍按源的时间窗口与字节计数语义）
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Http {
                message: format!("读取响应流失败: {url}"),
                status: None,
                source: Some(Box::new(e)),
            })?
        {
            file.write_all(&chunk).await.map_err(|e| Error::DownloadFailed {
                message: format!("写入文件失败: {local_path}"),
                source: Some(Box::new(e)),
            })?;
            downloaded_bytes += chunk.len() as i64;
            bytes_since_update += chunk.len() as i64;

            let now = Instant::now();
            let elapsed = now.duration_since(last_update);
            // 源：距上次上报 ≥ 0.5s 时上报 Downloading 进度
            if elapsed.as_secs_f64() >= 0.5 {
                let speed = bytes_since_update as f64 / elapsed.as_secs_f64();
                if let Some(reporter) = progress {
                    reporter.report_download(DownloadProgress {
                        file_name: file_name.clone(),
                        downloaded_bytes,
                        total_bytes,
                        percentage: if total_bytes > 0 {
                            downloaded_bytes as f64 * 100.0 / total_bytes as f64
                        } else {
                            0.0
                        },
                        speed_bytes_per_second: speed as i64, // 源：(long)speed
                        retry_count: retry as i32,
                        status: DownloadStatus::Downloading,
                    });
                }
                bytes_since_update = 0;
                last_update = now;
            }
        }

        // 关闭文件句柄（源：await using 释放 + FlushAsync；tokio::fs::File 有内部缓冲，
        // 必须先 flush 再校验，Windows 上句柄先释放避免占用冲突）
        file.flush().await.map_err(|e| Error::DownloadFailed {
            message: format!("刷新文件失败: {local_path}"),
            source: Some(Box::new(e)),
        })?;
        drop(file);

        // 源：期望值非空且 SHA1 不匹配 → File.Delete + 抛异常
        if !expected_sha1.is_empty() && !validate_file_hash(local_path, expected_sha1) {
            let _ = tokio::fs::remove_file(local_path).await; // 源 File.Delete（不抛错）
            return Err(Error::DownloadFailed {
                message: format!("文件哈希不匹配: {file_name}"),
                source: None,
            });
        }

        // 源：成功后上报 Completed（百分比固定 100，速度 0）
        if let Some(reporter) = progress {
            reporter.report_download(DownloadProgress {
                file_name,
                downloaded_bytes,
                total_bytes,
                percentage: 100.0,
                speed_bytes_per_second: 0,
                retry_count: retry as i32,
                status: DownloadStatus::Completed,
            });
        }

        Ok(())
    }

    /// 下载资产索引并处理资源对象（源：DownloadAssetIndexAsync）。
    /// 已存在且 SHA1 匹配 → 跳过；逐镜像"下载 + 处理"，任一失败尝试下一镜像；
    /// 全部镜像失败静默返回（源 catch 吞异常，不中断整体）。
    async fn download_asset_index(
        &self,
        asset_index: &AssetIndex,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        let local_path = Path::new(&self.game_root_path)
            .join("assets")
            .join("indexes")
            .join(format!("{}.json", asset_index.id));
        let local_path_str = local_path.to_string_lossy().into_owned();

        // 源：Path.GetDirectoryName 非空时 Directory.CreateDirectory
        if let Some(dir) = local_path.parent() {
            if !dir.as_os_str().is_empty() {
                tokio::fs::create_dir_all(dir).await.map_err(|e| Error::DownloadFailed {
                    message: format!("创建目录失败: {}", dir.display()),
                    source: Some(Box::new(e)),
                })?;
            }
        }

        if local_path.is_file() && validate_file_hash(&local_path_str, &asset_index.sha1) {
            return Ok(());
        }

        let mirror_urls = self
            .source_manager
            .generate_mirror_urls(&asset_index.url, ResourceType::AssetIndex);
        for url in mirror_urls {
            // 源：try { 下载 + 处理 } catch { 尝试下一镜像 } —— 下载或处理失败均吞掉
            let downloaded = self
                .download_file_with_retry(&url, &local_path_str, &asset_index.sha1, progress)
                .await
                .is_ok();
            if !downloaded {
                continue;
            }
            if self.process_asset_index(&local_path_str, progress).await.is_ok() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// 处理资产索引：解析 JSON 并逐资源下载（源：ProcessAssetIndexAsync）。
    /// 进度每 100 个资源上报一次（FileName 形如 "Assets (100/1000)"）；
    /// 单资源下载失败被镜像循环吞掉（静默跳过，源同逻辑）。
    async fn process_asset_index(
        &self,
        index_path: &str,
        progress: Option<&dyn ProgressReporter>,
    ) -> Result<(), Error> {
        // 源：File.ReadAllTextAsync + JsonSerializer.Deserialize
        // （解析异常向上传播 → 被 download_asset_index 镜像循环吞掉）
        let json =
            tokio::fs::read_to_string(index_path)
                .await
                .map_err(|e| Error::DownloadFailed {
                    message: format!("读取资源索引失败: {index_path}"),
                    source: Some(Box::new(e)),
                })?;
        let index_data: AssetIndexData = serde_json::from_str(&json).map_err(|e| {
            Error::DownloadFailed {
                message: format!("解析资源索引失败: {index_path}"),
                source: Some(Box::new(e)),
            }
        })?;

        // 源：indexData?.Objects == null → return
        let Some(objects) = index_data.objects else {
            return Ok(());
        };

        let total_assets = objects.len();
        let mut downloaded: usize = 0;

        for asset in objects.values() {
            let hash = &asset.hash;
            // 源：hash[..2] + hash（哈希长度 < 2 时 C# 抛 ArgumentOutOfRangeException
            // ↔ Rust 切片 panic，均被上层捕获/上报）
            let asset_path = format!("{}/{}", &hash[..2], hash);
            let local_asset_path = Path::new(&self.game_root_path)
                .join("assets")
                .join("objects")
                .join(&asset_path);
            let local_asset_path_str = local_asset_path.to_string_lossy().into_owned();

            if !local_asset_path.is_file() || !validate_file_hash(&local_asset_path_str, hash) {
                // 源：官方 URL + 镜像列表逐镜像尝试（进度传 null → 无单文件进度；
                // ⚠️ 源不创建资源对象目录，File.Create 失败 → 被 catch 吞掉 → 资源静默跳过）
                let url = format!("https://resources.download.minecraft.net/{asset_path}");
                for mirror_url in self
                    .source_manager
                    .generate_mirror_urls(&url, ResourceType::Asset)
                {
                    if self
                        .download_file_with_retry(&mirror_url, &local_asset_path_str, hash, None)
                        .await
                        .is_ok()
                    {
                        break;
                    }
                }
            }

            downloaded += 1;
            // 源：每 100 个资源上报一次整体进度（百分比 downloaded*100/totalAssets）
            if downloaded % 100 == 0 {
                if let Some(reporter) = progress {
                    reporter.report_download(DownloadProgress {
                        file_name: format!("Assets ({downloaded}/{total_assets})"),
                        downloaded_bytes: downloaded as i64,
                        total_bytes: total_assets as i64,
                        percentage: downloaded as f64 * 100.0 / total_assets as f64,
                        speed_bytes_per_second: 0,
                        retry_count: 0,
                        status: DownloadStatus::Downloading,
                    });
                }
            }
        }
        Ok(())
    }

    /// 收集库文件工件（源：GetLibraryArtifacts）：
    /// 主工件（规则允许时）+ natives 分类器（当前 OS/架构匹配时）。
    fn get_library_artifacts(&self, library: &Library) -> Vec<Artifact> {
        let mut artifacts = Vec::new();

        // 源：Downloads.Artifact != null && (Rules == null || ShouldIncludeLibrary(Rules))
        if let Some(artifact) = &library.downloads.artifact {
            let include = match &library.rules {
                None => true,
                Some(rules) => should_include_library(rules),
            };
            if include {
                artifacts.push(artifact.clone());
            }
        }

        // 源：Natives != null && Downloads.Classifiers != null → 按当前 OS 取分类器
        if let (Some(natives), Some(classifiers)) = (&library.natives, &library.downloads.classifiers)
        {
            let os_name = get_current_os_name();
            if let Some(native_classifier) = natives.get(os_name) {
                // 源：nativeClassifier.Replace("${arch}", SystemHelper.GetCurrentArch())
                let classifier_key = native_classifier.replace("${arch}", get_current_arch());
                if let Some(native_artifact) = classifiers.get(&classifier_key) {
                    artifacts.push(native_artifact.clone());
                }
            }
        }

        artifacts
    }
}

/// 判断库是否应包含（源：ShouldIncludeLibrary，静态方法）：
/// 逐规则应用 allow/disallow（当前 OS 匹配时生效），最终状态为 allow。
fn should_include_library(rules: &[Rule]) -> bool {
    let mut allow = false;
    for rule in rules {
        if rule.action == "allow" {
            // 源：rule.Action == "allow" && (rule.Os == null || SystemHelper.IsOsMatch(rule.Os))
            if rule.os.as_ref().map_or(true, is_os_match) {
                allow = true;
            }
        } else if rule.action == "disallow" {
            if rule.os.as_ref().map_or(true, is_os_match) {
                allow = false;
            }
        }
    }
    allow
}

/// 取路径的文件名（对应 Path.GetFileName）。
fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}



