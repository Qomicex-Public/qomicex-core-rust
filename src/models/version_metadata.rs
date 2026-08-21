//! 版本元数据模型（B1）
//! 对应源：Models/VersionMetadata/*.cs（7 个文件）+ JsonConverters/VersionArgumentsConverter.cs
//! 特殊兼容点：VersionArguments 的解析/序列化由手写 impl 完整保留源 converter 语义——
//! - 旧形态：arguments 直接为字符串（minecraftArguments / VersionArgumentsOld 形态）
//! - 新形态：{"game": [...], "jvm": [...]}，数组元素可为纯字符串或带 rules 的对象
//! 与源的偏差（详见 p4 翻译日志）：
//! - 源 converter 对"非对象 / 无 game 键"返回 null；Rust 侧字符串映射为 Old，
//!   其余非法形态（null/数字/数组/无 game 的对象）报错而非静默置 null

use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// 表示资源文件索引信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndex {
    pub id: String,
    pub sha1: String,
    pub size: i64,
    pub total_size: i64,
    pub url: String,
}

/// 表示单个游戏版本的完整元数据
/// 对应：https://piston-meta.mojang.com/v1/packages/.../version.json
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompleteVersionMetadata {
    pub id: String,
    pub r#type: String,
    pub main_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<VersionArguments>,
    pub libraries: Vec<Library>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_index: Option<AssetIndex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<VersionDownloads>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_version: Option<JavaVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_launcher_version: Option<i32>,
    /// 发布时间（源为 DateTimeOffset + MinecraftDateTimeConverter，
    /// 暂用原始字符串保真，类型决策见日志 ⚠️ UNMAPPED）
    pub release_time: String,
    /// 发布时间（同上 ⚠️ UNMAPPED）
    pub time: String,
}

/// 表示该版本所需的 Java 版本信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: i32,
}

/// 表示启动参数（源：VersionArguments + VersionArgumentsConverter）
/// 兼容两种形态：
/// - 旧格式：arguments 直接为字符串（对应源 minecraftArguments / VersionArgumentsOld 形态）
/// - 新格式：{"game": [...], "jvm": [...]}（VersionArgumentsNew）
#[derive(Debug, Clone, PartialEq)]
pub enum VersionArguments {
    /// 旧格式启动参数（arguments 为纯字符串）
    Old(String),
    /// 新格式启动参数（1.13+）
    New {
        /// 游戏参数
        game: Vec<ArgumentItem>,
        /// JVM 参数
        jvm: Vec<ArgumentItem>,
    },
}

impl Serialize for VersionArguments {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            // 源 Write 对非 New 形态抛出 NotSupportedException
            VersionArguments::Old(_) => Err(S::Error::custom(
                "旧格式启动参数不支持序列化（源为 NotSupportedException）",
            )),
            VersionArguments::New { game, jvm } => {
                let mut obj = Map::new();
                obj.insert(
                    "game".into(),
                    serde_json::to_value(game).map_err(S::Error::custom)?,
                );
                obj.insert(
                    "jvm".into(),
                    serde_json::to_value(jvm).map_err(S::Error::custom)?,
                );
                Value::Object(obj).serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for VersionArguments {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            // 旧形态：arguments 直接为字符串
            Value::String(s) => Ok(VersionArguments::Old(s)),
            Value::Object(obj) => {
                let game = obj.get("game").ok_or_else(|| {
                    D::Error::custom("arguments 缺少 game 键（源逻辑：返回 null）")
                })?;
                let game = parse_argument_items(game).map_err(D::Error::custom)?;
                let jvm = match obj.get("jvm") {
                    Some(v) => parse_argument_items(v).map_err(D::Error::custom)?,
                    None => Vec::new(),
                };
                Ok(VersionArguments::New { game, jvm })
            }
            _ => Err(D::Error::custom(
                "arguments 既非字符串也非对象（源逻辑：返回 null；仅畸形数据会命中）",
            )),
        }
    }
}

/// 参数项，可以是字符串或带规则的对象
#[derive(Debug, Clone, PartialEq)]
pub enum ArgumentItem {
    /// 字符串格式的参数
    String(String),
    /// 带规则的对象格式参数
    Object {
        /// 参数值列表
        value: Vec<String>,
        /// 条件规则
        rules: Vec<Rule>,
    },
}

impl Serialize for ArgumentItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ArgumentItem::String(s) => serializer.serialize_str(s),
            ArgumentItem::Object { value, rules } => {
                let mut obj = Map::new();
                // 源 WriteItems：value 仅 1 项时写字符串，否则写数组
                if value.len() == 1 {
                    obj.insert("value".into(), Value::String(value[0].clone()));
                } else {
                    obj.insert(
                        "value".into(),
                        Value::Array(value.iter().cloned().map(Value::String).collect()),
                    );
                }
                // 源 WriteItems：rules 为空时不写该键
                if !rules.is_empty() {
                    obj.insert(
                        "rules".into(),
                        serde_json::to_value(rules).map_err(S::Error::custom)?,
                    );
                }
                Value::Object(obj).serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for ArgumentItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(s) => Ok(ArgumentItem::String(s)),
            Value::Object(obj) => {
                let (value, rules) = parse_argument_object(&obj).map_err(D::Error::custom)?;
                Ok(ArgumentItem::Object { value, rules })
            }
            // 源 ParseItems：非字符串/对象元素被忽略（跳过）；此处仅对直接解析单元素时报错
            _ => Err(D::Error::custom(
                "参数项既非字符串也非对象（源逻辑：忽略该元素）",
            )),
        }
    }
}

/// 源 ParseItems：解析 game/jvm 数组元素。
/// 字符串 → String；对象 → Object{value, rules}；其余类型元素被跳过；
/// 非数组整体返回空列表。
fn parse_argument_items(value: &Value) -> Result<Vec<ArgumentItem>, String> {
    let Value::Array(items) = value else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for item in items {
        match item {
            Value::String(s) => result.push(ArgumentItem::String(s.clone())),
            Value::Object(obj) => {
                let (value, rules) = parse_argument_object(obj)?;
                result.push(ArgumentItem::Object { value, rules });
            }
            _ => {}
        }
    }
    Ok(result)
}

/// 解析参数对象元素（源 ParseItems 的 Object 分支 + DeserializeRules）：
/// - value：缺失/非字符串非数组 → []；字符串 → [s]；数组 → 各元素（非字符串元素 → ""）
/// - rules：缺失/非数组 → []；数组 → 逐元素反序列化为 Rule（失败向上传播，同源异常语义）
fn parse_argument_object(obj: &Map<String, Value>) -> Result<(Vec<String>, Vec<Rule>), String> {
    let value = match obj.get("value") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .map(|e| match e {
                Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let rules = match obj.get("rules") {
        Some(Value::Array(arr)) => {
            let mut rules = Vec::with_capacity(arr.len());
            for r in arr {
                rules.push(
                    serde_json::from_value(r.clone())
                        .map_err(|e| format!("rules 元素反序列化失败: {e}"))?,
                );
            }
            rules
        }
        _ => Vec::new(),
    };
    Ok((value, rules))
}

/// 表示一个条件规则
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRequirement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<HashMap<String, bool>>,
}

/// 表示操作系统要求
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OsRequirement {
    /// Newer versions allow an OS rule with only `arch` (e.g. `{"arch":"x86"}`)
    /// with no `name`; keep it optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// 表示一个库文件（依赖项）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub name: String,
    pub downloads: LibraryDownloads,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<Rule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub natives: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extract: Option<LibraryExtract>,
}

/// 库文件的下载信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDownloads {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

/// 文件信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub path: String,
    pub url: String,
    pub sha1: String,
    pub size: i64,
}

/// 库文件的提取规则（主要用于 natives）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryExtract {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

/// A raw download descriptor. Mojang's `downloads.client` / `downloads.server`
/// only carry `{sha1,size,url}` (no `path`); libraries embed `path`. So the
/// game jar download target is derived by the launcher as
/// `versions/{id}/{id}.jar`, never read from this struct (path stays optional).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub url: String,
    pub sha1: String,
    pub size: i64,
}

/// 表示版本核心Jar文件的下载信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionDownloads {
    pub client: DownloadFile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<DownloadFile>,
}
