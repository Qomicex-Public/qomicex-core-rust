//! 下载重试辅助（B6）
//!
//! 对应源实现：Services/DefaultResourceCompleter.cs 的私有方法
//! `DownloadFileWithRetryAsync`（源项目无独立重试工具类，重试逻辑内嵌于
//! 资源补全器；MAPPING_TABLE utils 已把「下载重试 + 校验」指向本模块与
//! checksum.rs，本批次按任务指令将其抽出为轻量重试包装）。
//!
//! 源语义逐条保留：
//! - 循环 `maxRetries` 次（默认 3），每次 GET 流式下载到本地文件；
//! - 非 2xx（`EnsureSuccessStatusCode`）视为失败进入重试；
//! - 下载完成后若期望 SHA1 非空且文件校验不通过 → 删除文件、抛错进入重试；
//! - 第 retry 次失败（retry < maxRetries - 1）：退避等待 `1000 * (retry + 1)` ms；
//! - 最后一次失败：异常向上传播（`catch when (retry < maxRetries - 1)` 不过滤）。
//!
//! 差异说明（B6 定案）：
//! - 进度报告（源 IProgress<DownloadProgress>，MAPPING_TABLE 定为
//!   `mpsc::channel<CoreEvent>`）不在本模块实现，保留给后续 ResourceCompleter
//!   批次；本模块仅返回 `Result<(), Error>`。
//! - 错误类型：HTTP 传输/状态码错误 → `Error::Http`；文件写入/校验失败 →
//!   `Error::DownloadFailed`（对应源 DownloadFailedException 语义）。

use std::path::Path;
use std::time::Duration;

use crate::error::Error;
use crate::services::download::checksum;

/// 默认最大重试次数（源：`DownloadFileWithRetryAsync(..., int maxRetries = 3)`）。
const DEFAULT_MAX_RETRIES: usize = 3;

/// 重试退避基数毫秒数（源：`Task.Delay(1000 * (retry + 1))`）。
const RETRY_DELAY_BASE_MS: u64 = 1000;

/// 带重试的下载（源：`DownloadFileWithRetryAsync`）。
///
/// 将 `url` 下载到 `local_path`；`expected_sha1` 非空时下载完成后校验 SHA1，
/// 不匹配则删除文件并进入重试；失败按 `1000 * (attempt + 1)` 毫秒递增退避
/// （第 0 次失败等 1s、第 1 次等 2s……），重试满 `max_retries` 次后返回
/// 最后一次错误。
///
/// 边界（源语义）：`max_retries == 0` 时循环不执行 → 直接 `Ok(())`。
pub(crate) async fn download_with_retry(
    client: &reqwest::Client,
    url: &str,
    local_path: &Path,
    expected_sha1: Option<&str>,
    max_retries: usize,
) -> Result<(), Error> {
    for attempt in 0..max_retries {
        match download_once(client, url, local_path, expected_sha1).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if attempt + 1 < max_retries {
                    let delay_ms = RETRY_DELAY_BASE_MS * (attempt as u64 + 1);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                } else {
                    return Err(err);
                }
            }
        }
    }
    Ok(())
}

/// 单次下载（源：`DownloadFileWithRetryAsync` 循环体）。
///
/// - GET 流式下载：先请求成功（对应 `SendAsync`），再校验状态码
///   （对应 `EnsureSuccessStatusCode`，非 2xx → Err 且不创建文件）；
/// - 按 8KB 语义以 chunk 方式写入磁盘（源 8192 字节缓冲 + 流拷贝，
///   不整体缓冲内存）；
/// - `expected_sha1` 非空且校验失败 → 删除已写文件并返回 Err
///   （源消息：`文件哈希不匹配: {文件名}`）。
async fn download_once(
    client: &reqwest::Client,
    url: &str,
    local_path: &Path,
    expected_sha1: Option<&str>,
) -> Result<(), Error> {
    let mut response = client.get(url).send().await.map_err(|e| Error::Http {
        message: format!("GET {url} 失败"),
        source: Some(Box::new(e)),
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::Http {
            message: format!("GET {url} 返回非成功状态码 {status}"),
            source: None,
        });
    }

    let mut file = tokio::fs::File::create(local_path).await.map_err(|e| Error::DownloadFailed {
        message: format!("创建文件失败: {}", local_path.display()),
        source: Some(Box::new(e)),
    })?;

    while let Some(chunk) = response.chunk().await.map_err(|e| Error::Http {
        message: format!("读取响应体失败: {url}"),
        source: Some(Box::new(e)),
    })? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| Error::DownloadFailed {
                message: format!("写入文件失败: {}", local_path.display()),
                source: Some(Box::new(e)),
            })?;
    }

    if let Some(expected) = expected_sha1 {
        if !expected.is_empty() && !checksum::validate_file_sha1(local_path, expected)? {
            let _ = tokio::fs::remove_file(local_path).await;
            return Err(Error::DownloadFailed {
                message: format!("文件哈希不匹配: {}", local_path.display()),
                source: None,
            });
        }
    }

    Ok(())
}
