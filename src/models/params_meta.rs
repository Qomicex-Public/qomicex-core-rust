//! 安装器参数元数据模型（B1）
//! 对应源：Models/ParamsMeta/ParamEntry.cs + JsonContext/ParamsJsonContent.cs
//! 依赖：crate::models::version_metadata::{Rule, AssetIndex}（VersionMetadata 批次，另行移植）

use crate::models::version_metadata::{AssetIndex, Rule};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 表示一个参数条目（带条件规则，如 JVM/游戏参数中的规则对象）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParamEntry {
    /// 条件规则列表（源为 List<VersionMetadata.Rule>?，可为 null）
    pub rules: Option<Vec<Rule>>,
    /// 原始 JSON 值（源为 JsonElement，可对应 serde_json::Value）
    pub value: Value,
}

/// 表示安装器配置（版本 JSON 顶层解析结果）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// 参数列表
    pub arguments: Arguments,
    /// 继承的父版本 ID
    pub inherits_from: String,
    /// 主类名
    pub main_class: String,
    /// 传统 minecraftArguments（旧格式参数）
    pub minecraft_arguments: String,
    /// 资源文件索引信息（来自 Models/VersionMetadata/AssetIndex.cs）
    pub asset_index: AssetIndex,
}

/// 表示启动参数列表（新版 arguments 结构）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Arguments {
    /// JVM 参数列表（JsonElement，可为纯字符串或带规则的对象）
    pub jvm: Vec<Value>,
    /// 游戏参数列表（同上）
    pub game: Vec<Value>,
}
