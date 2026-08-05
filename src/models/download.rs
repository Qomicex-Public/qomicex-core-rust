//! 下载模型（B1）：对应源文件
//! - Models/Download/DownloadProgress.cs（DownloadProgress 记录 + DownloadStatus 枚举）
//! - Models/Download/DownloadSource.cs（DownloadSource 记录 + DownloadSourceType 枚举）
//! - Models/ResourceType.cs（ResourceType 枚举）
//! - Builder/CoreOptions.cs 内的 DownloadMirror 枚举
//! 以上均未注册任何 JsonContext（内存/配置用途），枚举默认数字序列化 → serde_repr。

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// 下载状态（源：DownloadProgress.cs 内 DownloadStatus 枚举）
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum DownloadStatus {
    Pending = 0,
    Downloading = 1,
    Completed = 2,
    Failed = 3,
    Retrying = 4,
    Cancelled = 5,
}

/// 下载进度（源：DownloadProgress 记录）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub file_name: String,
    pub downloaded_bytes: i64,
    pub total_bytes: i64,
    pub percentage: f64,
    pub speed_bytes_per_second: i64,
    pub retry_count: i32,
    pub status: DownloadStatus,
}

/// 下载源类型（源：DownloadSource.cs 内 DownloadSourceType 枚举；BMCLAPI → Bmclapi，遵循 DownloadMirror 映射先例）
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum DownloadSourceType {
    Official = 0,
    Bmclapi = 1,
    Custom = 2,
}

/// 下载源（源：DownloadSource 记录，Description 默认 null）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSource {
    pub r#type: DownloadSourceType,
    pub name: String,
    pub base_url: String,
    pub is_enabled: bool,
    pub priority: i32,
    pub description: Option<String>,
}

/// 下载镜像（源：CoreOptions.cs 内 DownloadMirror 枚举；BMCLAPI → Bmclapi）
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum DownloadMirror {
    Official = 0,
    Bmclapi = 1,
}

/// 资源类型（源：ResourceType.cs 内 ResourceType 枚举）
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum ResourceType {
    Library = 0,
    Asset = 1,
    Client = 2,
    Server = 3,
    AssetIndex = 4,
}
