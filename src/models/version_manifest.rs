//! 版本清单模型（B1）
//! 对应源：Models/VersionManifest/*.cs，由 VersionManifestJsonContext（CamelCase 策略）序列化

use serde::{Deserialize, Serialize};

/// 表示从 Mojang API 获取的版本清单根对象
/// 对应：https://launchermeta.mojang.com/mc/game/version_manifest.json
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionManifestRoot {
    /// 最新版本信息
    pub latest: LatestVersionInfo,
    /// 全部版本条目列表
    pub versions: Vec<ManifestVersionInfo>,
}

/// 表示最新版本信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LatestVersionInfo {
    /// 最新正式版版本号
    pub release: String,
    /// 最新快照版版本号
    pub snapshot: String,
}

/// 表示版本清单中的单个版本条目
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestVersionInfo {
    /// 版本 ID
    pub id: String,
    /// 版本类型（release / snapshot / old_alpha / old_beta）
    pub r#type: String,
    /// 版本 JSON 下载地址
    pub url: String,
    /// 发布时间（源为 DateTimeOffset + MinecraftDateTimeConverter，
    /// 暂用原始字符串保真，类型决策见日志 ⚠️ UNMAPPED）
    pub time: String,
    /// 正式发布时间（同上 ⚠️ UNMAPPED）
    pub release_time: String,
}
