//! Java 模型（B1）：对应源文件 Public/Models/JavaResult.cs（全部内容）
//! - JavaResult / JavaSearchOptions 记录、JavaPackageInfo 记录类
//! - JavaSearchMode / JavaState / JavaType / JavaDownloadSource / JavaPlatform /
//!   JavaArchitecture / JavaPackageType 枚举
//! 源上下文无 UseStringEnumConverter → 枚举用 serde_repr 数字序列化。

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// Java 搜索模式（源：JavaResult.cs 的 JavaSearchMode 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaSearchMode {
    Quick = 0,
    Deep = 1,
    Custom = 2,
}

/// Java 状态（源：JavaResult.cs 的 JavaState 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaState {
    Valid = 0,
    InvalidPath = 1,
    MissingReleaseFile = 2,
    CorruptedReleaseFile = 3,
    UnknownError = 4,
}

/// Java 类型（源：JavaResult.cs 的 JavaType 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaType {
    Unknown = 0,
    JDK = 1,
    JRE = 2,
}

/// Java 下载源（源：JavaResult.cs 的 JavaDownloadSource 枚举，BMCLAPI 按 DownloadMirror 映射先例命名 Bmclapi）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaDownloadSource {
    Bmclapi = 0,
    Adoptium = 1,
    Zulu = 2,
}

/// Java 平台（源：JavaResult.cs 的 JavaPlatform 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaPlatform {
    Windows = 0,
    Linux = 1,
    MacOS = 2,
}

/// Java 架构（源：JavaResult.cs 的 JavaArchitecture 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaArchitecture {
    X64 = 0,
    Arm64 = 1,
}

/// Java 包类型（源：JavaResult.cs 的 JavaPackageType 枚举）。
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum JavaPackageType {
    JRE = 0,
    JDK = 1,
}

/// Java 探测结果（源：JavaResult.cs 的 JavaResult 记录）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JavaResult {
    pub path: String,
    pub major_version: i32,
    pub version: String,
    pub state: JavaState,
    pub arch: String,
    pub r#type: JavaType,
    pub discovered_by: String,
    pub name: String,
}

/// Java 搜索选项（源：JavaResult.cs 的 JavaSearchOptions 记录，含全部源默认值）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JavaSearchOptions {
    pub custom_exclude_paths: Vec<String>,
    pub custom_root_path: Option<String>,
    pub game_dir: Option<String>,
    pub mode: JavaSearchMode,
    pub include_jre: bool,
    pub include_jdk: bool,
    pub max_depth: i32,
    pub max_results: i32,
    pub scan_hidden_folders: bool,
    pub include_network_drives: bool,
}

impl Default for JavaSearchOptions {
    fn default() -> Self {
        Self {
            custom_exclude_paths: Vec::new(),
            custom_root_path: None,
            game_dir: None,
            mode: JavaSearchMode::Quick,
            include_jre: true,
            include_jdk: true,
            max_depth: 5,
            max_results: 100,
            scan_hidden_folders: false,
            include_network_drives: false,
        }
    }
}

/// Java 包信息（源：JavaResult.cs 的 JavaPackageInfo 记录类）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JavaPackageInfo {
    pub major_version: i32,
    pub full_version: String,
    pub build: String,
    pub platform: JavaPlatform,
    pub architecture: JavaArchitecture,
    pub package_type: JavaPackageType,
    pub source: JavaDownloadSource,
    pub file_name: String,
    pub download_url: String,
    pub sha256: String,
    pub size: Option<i64>,
}

impl Default for JavaPackageInfo {
    fn default() -> Self {
        Self {
            major_version: 0,
            full_version: String::new(),
            build: String::new(),
            platform: JavaPlatform::Windows,
            architecture: JavaArchitecture::X64,
            package_type: JavaPackageType::JRE,
            source: JavaDownloadSource::Bmclapi,
            file_name: String::new(),
            download_url: String::new(),
            sha256: String::new(),
            size: None,
        }
    }
}
