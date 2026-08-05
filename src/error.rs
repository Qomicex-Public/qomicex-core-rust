//! 统一 Error 枚举（thiserror）（B1，对应 Exceptions/）

use thiserror::Error;

/// 统一错误枚举，对应源项目 `Exceptions/` 目录下的 5 个自定义异常。
#[derive(Debug, Error)]
pub enum Error {
    #[error("download failed: {message}")]
    DownloadFailed {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("invalid params: {message}")]
    Params {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("resource completion failed: {message}")]
    ResourceCompletion {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("version metadata error: {message}")]
    VersionMetadata {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("version not found: {message}")]
    VersionNotFound {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}
