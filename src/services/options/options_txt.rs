//! options.txt 读写 + 版本可用性过滤（B12，对应 OptionsProvider.cs）
//!
//! 对应源文件：Services/Options/OptionsProvider.cs（namespace Qomicex.Core.AOT.Services.Options，
//! 316 行）；契约 trait：crate::api::options::OptionsProvider（13 方法，全同步 → 无 async_trait，
//! B4 定案"options.rs 纯同步无需"）。
//!
//! 语义要点：
//! - options.txt 逐字格式：空行/纯空白行跳过、`#` 注释行跳过；分隔符取行内首个 `=`，
//!   否则 `:`（`.Contains('=') ? '=' : ':'`），只按首个分隔符分割（.NET Split(sep, 2)），
//!   键值两侧 Trim；写回**恒为 `key:value` 冒号 + `\n`**（源 WriteLine），UTF-8 无 BOM。
//! - 写回顺序保真：源 Dictionary 迭代序 == 插入序 → 内部以 `Vec<(String, String)>` 有序快照
//!   （已存在 key 原位替换、新 key 追加，同 .NET Dictionary 语义），SetOption→Save 往返
//!   保持文件行序；契约 `load()` 返回的 GameOptionsSnapshot 再转为 HashMap（模型已定案）。
//! - 版本可用性（IsOptionAvailableInVersion）：源 `GameVersion` 为 readonly record struct，
//!   FirstOrDefault 未命中返回 default（Version=null）→ IsNullOrEmpty → 可用；introducedVersion
//!   空白 → 恒可用；日期比较为 DateTime → B1 定案 String 保真，比较时经
//!   util::json_helper::parse_minecraft_datetime 解析，解析失败 ≡ DateTime.MinValue
//!   （源 TryParse 失败分支），days_from_civil 换算 UTC 总序（详见方法注释）。
//! - 多语言描述：descriptions.json 为 `HashMap<String, HashMap<String, String>>`（外层语言键、
//!   内层选项名键，外层键**大小写敏感**——源 `?? new Dictionary(OrdinalIgnoreCase)` 仅作用于
//!   反序列化得 null 时的回退空字典，行为上空字典无差异）；回退链 language → "en-US" →
//!   "(无描述)"，每级需命中且非空白（IsNullOrWhiteSpace）。
//! - ValueKind 推断顺序逐字：Boolean（"true,false"/"false,true"，忽略大小写）→ Range
//!   （正则逐字）→ Enum（含逗号）→ Text（Range 检查在逗号之前）。
//! - 无效 JSON：源 JsonDocument.Parse/JsonSerializer.Deserialize 抛异常 → panic!；
//!   文件 IO 错误 → panic!（同 servers_dat.rs P50 / saves.rs P48 约定）；JSON 字面 `null`
//!   源回退空字典/空列表（`?? new ...`）→ 反序列化为 Option + unwrap_or_default。
//!
//! ⚠️ 差异/UNMAPPED 详见翻译日志（b12-logs/p54-options.md）：
//! - parse_minecraft_datetime 仅覆盖 ISO-8601 子集（json_helper.rs 已登记），
//!   .NET TryParse 全格式矩阵不接受；真实 manifest 全为 ISO → 不触发。
//! - Rust `str::lines()` 不识别裸 `\r` 分隔行（.NET ReadAllLines 识别）；实际 options.txt
//!   为 \n / \r\n，不触发。
//! - 非字符串 JSON 值：源 GetString() 抛 InvalidOperationException → 本实现宽容取空串。
//! - SetOption 不可用：源 InvalidOperationException → panic!（契约无 Result，消息文本逐字保留）。

use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::api::options::OptionsProvider as OptionsProviderApi;
use crate::models::local::{
    GameOption, GameOptionsSnapshot, GameVersion, MinecraftOption, OptionDefinition, OptionViewItem,
};
use crate::util::json_helper::{MinecraftDateTime, parse_minecraft_datetime};

/// 选项提供方（源：OptionsProvider 具体类，OptionsProvider.cs）。
/// 持有选项定义（options.json）、多语言描述（descriptions.json）与版本清单（manifest），
/// 负责 options.txt 的读取/快照/写入与版本可用性过滤。
/// 契约 trait：crate::api::options::OptionsProvider（13 方法）。
pub(crate) struct OptionsProvider {
    /// 版本清单（源字段 `_versions`）
    versions: Vec<GameVersion>,
    /// 游戏根目录（源字段 `_gameDirectory`）
    game_directory: String,
    /// 游戏版本（源字段 `_version`，用于版本分段路径与可用性判断）
    version: String,
    /// 是否使用版本分段目录（源字段 `_versionSpecific`）
    version_specific: bool,
    /// 全部选项定义（源字段 `_options`，options.json）
    options: Vec<MinecraftOption>,
    /// 多语言描述：外层语言键 → 内层选项名键 → 描述文本（源字段 `_descriptions`）
    descriptions: HashMap<String, HashMap<String, String>>,
}

/// 默认描述（源常量 DefaultDescription）
const DEFAULT_DESCRIPTION: &str = "(无描述)";
/// 回退语言（源常量 FallbackLanguage）
const FALLBACK_LANGUAGE: &str = "en-US";

impl OptionsProvider {
    /// 创建选项提供方（源构造函数
    /// `OptionsProvider(string optionsJsonPath, string descriptionsJsonPath,
    /// string minecraftManifest, string gameDirectory, string gameVersion,
    /// bool versionSpecific)`；`minecraftManifest` 为版本清单 JSON 文本，非路径）。
    /// 源对无效 JSON 抛 JsonException / 文件缺失抛 FileNotFoundException →
    /// panic!（消息含原因）；JSON 字面 `null` 源以 `?? new ...` 回退空集合 →
    /// `Option` 反序列化 + unwrap_or_default，等价。
    pub(crate) fn new(
        options_json_path: &str,
        descriptions_json_path: &str,
        minecraft_manifest: &str,
        game_directory: String,
        game_version: String,
        version_specific: bool,
    ) -> Self {
        // C#: File.ReadAllText + JsonSerializer.Deserialize(...) ?? new List<MinecraftOption>()
        let options_json = fs::read_to_string(options_json_path)
            .unwrap_or_else(|e| panic!("读取 options.json 失败（{options_json_path}）: {e}"));
        let options: Vec<MinecraftOption> = serde_json::from_str(&options_json)
            .unwrap_or_else(|e| panic!("解析 options.json 失败（源 JsonException）: {e}"));

        // C#: ?? new Dictionary<string, Dictionary<string, string>>(StringComparer.OrdinalIgnoreCase)
        // 注：OrdinalIgnoreCase 仅作用于 null 回退分支的空字典 → 行为等价于默认（大小写敏感）
        let desc_json = fs::read_to_string(descriptions_json_path).unwrap_or_else(|e| {
            panic!("读取 descriptions.json 失败（{descriptions_json_path}）: {e}")
        });
        let descriptions: HashMap<String, HashMap<String, String>> =
            serde_json::from_str(&desc_json)
                .unwrap_or_else(|e| panic!("解析 descriptions.json 失败（源 JsonException）: {e}"));

        Self {
            versions: Self::parse_version_manifest(minecraft_manifest),
            game_directory,
            version: game_version,
            version_specific,
            options,
            descriptions,
        }
    }

    /// 解析版本清单（源：ParseVersionManifest，static）：
    /// - 无效 JSON → 源 JsonDocument.Parse 抛 JsonException → panic!；
    /// - 缺 `versions` 属性 → 空列表（源 TryGetProperty 失败分支）；
    /// - 条目取 `id`/`type`/`releaseTime`，缺失或 null → 空串（源 `?? string.Empty` /
    ///   TryGetProperty 失败分支）；
    /// - `releaseTime` 源经 DateTime.TryParse（失败 → DateTime.MinValue）；B1 定案
    ///   DateTime → String 保真 → 此处仅保存原始文本，比较时再解析（失败 ≡ MinValue）。
    fn parse_version_manifest(manifest_json: &str) -> Vec<GameVersion> {
        let document: Value = serde_json::from_str(manifest_json)
            .unwrap_or_else(|e| panic!("解析版本清单 JSON 失败（源 JsonException）: {e}"));

        let Some(versions_element) = document.get("versions") else {
            return Vec::new();
        };
        // 源 EnumerateArray 对非数组抛 InvalidOperationException
        let array = versions_element
            .as_array()
            .expect("版本清单 'versions' 非数组（源 EnumerateArray 抛 InvalidOperationException）");

        array
            .iter()
            .map(|version| {
                let id = version
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let release_type = version
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let release_time_str = version.get("releaseTime").and_then(Value::as_str);
                // 源：缺失/null → 跳过 TryParse → DateTime.MinValue；原始文本保真
                let release_date = release_time_str.unwrap_or("").to_string();
                GameVersion {
                    version: id,
                    release_type,
                    release_date,
                }
            })
            .collect()
    }

    /// 按名称查找选项定义（源：FindOption；`StringComparison.Ordinal` → 精确大小写敏感）
    fn find_option(&self, name: &str) -> Option<&MinecraftOption> {
        self.options.iter().find(|option| option.name == name)
    }

    /// 生成选项定义（源：ToDefinition(MinecraftOption) 重载——加载当前配置，
    /// 委托 2 参重载 → en-US 语言）
    fn to_definition(&self, option: &MinecraftOption) -> OptionDefinition {
        let config = self.load();
        self.to_definition_with_config(option, &config.values)
    }

    /// 生成选项定义（源：ToDefinition(MinecraftOption, IReadOnlyDictionary) 重载，
    /// 使用调用方提供的配置快照 + en-US 语言）
    fn to_definition_with_config(
        &self,
        option: &MinecraftOption,
        config: &HashMap<String, String>,
    ) -> OptionDefinition {
        self.to_definition_lang(option, config, FALLBACK_LANGUAGE)
    }

    /// 生成选项定义（源：ToDefinition(MinecraftOption, IReadOnlyDictionary, string) 重载；
    /// Rust 无重载 → 逐级改名，同名重命名决策见 api/options.rs）
    fn to_definition_lang(
        &self,
        option: &MinecraftOption,
        config: &HashMap<String, String>,
        language: &str,
    ) -> OptionDefinition {
        // C#: config.TryGetValue(option.Name, out var value) ? value : option.DefaultValue
        let current_value = config
            .get(&option.name)
            .cloned()
            .unwrap_or_else(|| option.default_value.clone());

        OptionDefinition {
            name: option.name.clone(),
            default_value: option.default_value.clone(),
            current_value,
            description: self.get_description_lang(&option.name, language),
            valid_values_raw: option.valid_values.clone(),
            introduced_version: option.introduced_version.clone(),
            is_available_in_current_version: self.is_option_available_in_version(&option.name),
            value_kind: Self::infer_value_kind(&option.valid_values),
        }
    }

    /// 判断选项在当前版本是否可用（源：IsOptionAvailableInVersion）：
    /// - 选项不存在 / introducedVersion 空白 → 存在性本身（不存在 → false，空白 → true）；
    /// - introducedVersion 或当前版本未在清单中 → true
    ///   （源 GameVersion 为 record struct，FirstOrDefault 未命中返回 default，
    ///   Version=null → IsNullOrEmpty → true）；
    /// - 否则比较 `currentVersion.ReleaseDate >= introducedVersion.ReleaseDate`。
    fn is_option_available_in_version(&self, option_name: &str) -> bool {
        let Some(option) = self.find_option(option_name) else {
            return false;
        };
        // C#: string.IsNullOrWhiteSpace(option.IntroducedVersion) → option != null
        if option.introduced_version.trim().is_empty() {
            return true;
        }

        // 源 FirstOrDefault（Ordinal 精确匹配）→ Rust find；未命中 ≡ default(GameVersion)
        let Some(introduced) = self
            .versions
            .iter()
            .find(|v| v.version == option.introduced_version)
        else {
            return true;
        };
        let Some(current) = self.versions.iter().find(|v| v.version == self.version) else {
            return true;
        };
        // 命中但 Version 为空串（JSON id 缺失）→ IsNullOrEmpty → true
        if introduced.version.is_empty() || current.version.is_empty() {
            return true;
        }

        Self::release_date_ge(current, introduced)
    }

    /// 日期比较：`current.ReleaseDate >= introduced.ReleaseDate`（源 DateTime 比较；
    /// B1 定案 String 保真 → 经 parse_minecraft_datetime 解析，解析失败 ≡ DateTime.MinValue，
    /// 源 TryParse 失败分支 → default(DateTime)）：
    /// - 双双失败 → MinValue >= MinValue → true；仅 introduced 失败 → true；
    ///   仅 current 失败 → false；
    /// - 均成功 → 按 UTC 绝对时刻比较（days_from_civil + 时分秒 − 时区偏移）。
    ///
    /// ⚠️ 差异：.NET TryParse 带偏移的串按本机时区换算为 Kind=Local 后比较；同一机器同一
    /// 时区下为常量平移，相对序不变；跨 DST 边界的极端相邻时间（差 <1h）可能差 1 小时，
    /// 实际 manifest 发布时间全为 +00:00，不触发（详见翻译日志）。
    fn release_date_ge(current: &GameVersion, introduced: &GameVersion) -> bool {
        match (
            parse_minecraft_datetime(&current.release_date).ok(),
            parse_minecraft_datetime(&introduced.release_date).ok(),
        ) {
            (None, None) => true,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(current_dt), Some(introduced_dt)) => {
                Self::instant_rank(&current_dt) >= Self::instant_rank(&introduced_dt)
            }
        }
    }

    /// 时刻绝对序值：自纪元的秒数（时区偏移向东为正，减去偏移还原 UTC）
    fn instant_rank(dt: &MinecraftDateTime) -> i64 {
        let days = Self::days_from_civil(dt.year, dt.month, dt.day);
        days * 86_400
            + i64::from(dt.hour) * 3_600
            + i64::from(dt.minute) * 60
            + i64::from(dt.second)
            - i64::from(dt.offset_minutes) * 60
    }

    /// 公历日期 → 自 1970-01-01 的天数（Howard Hinnant days_from_civil 算法；
    /// 支持负年份与任意公历日期，等价于 .NET DateTime 的日期分量语义）
    fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
        let y = i64::from(year) - if month <= 2 { 1 } else { 0 };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (i64::from(month) + 9) % 12;
        let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// 推断值类型（源：InferValueKind，static；判定顺序逐字）：
    /// - "true,false"/"false,true"（OrdinalIgnoreCase → eq_ignore_ascii_case）→ "Boolean"；
    /// - 区间正则（RangePattern）→ "Range"；
    /// - 含 `,` → "Enum"；否则 → "Text"。
    fn infer_value_kind(valid_values: &str) -> String {
        if valid_values.eq_ignore_ascii_case("true,false")
            || valid_values.eq_ignore_ascii_case("false,true")
        {
            return "Boolean".to_string();
        }

        if range_pattern().is_match(valid_values) {
            return "Range".to_string();
        }

        if valid_values.contains(',') {
            return "Enum".to_string();
        }

        "Text".to_string()
    }

    /// 选项文件路径（源：GetOptionFilePath）：版本分段 →
    /// `{gameDirectory}/versions/{version}/options.txt`，否则 `{gameDirectory}/options.txt`
    fn get_option_file_path(&self) -> PathBuf {
        if self.version_specific {
            return Path::new(&self.game_directory)
                .join("versions")
                .join(&self.version)
                .join("options.txt");
        }
        Path::new(&self.game_directory).join("options.txt")
    }

    /// 内部有序加载（源：Load 的字典构建部分；Rust 以 Vec 保持插入序以支持
    /// SetOption→Save 写回顺序保真，upsert 语义同 .NET Dictionary）：
    /// - 文件不存在 → 空快照（源 File.Exists 失败分支）；
    /// - 逐行：空行/纯空白行跳过（IsNullOrWhiteSpace）、`#` 开头注释行跳过；
    /// - 分隔符：行内含 `=` 用 `=`，否则 `:`；只按首个分隔符分割
    ///   （.NET Split(sep, 2) == split_once）；无分隔符的行（分割仅 1 段）跳过；
    /// - 键值两侧 Trim；重复键原位替换、新键追加（源 `dict[key] = value`）。
    fn load_internal(&self) -> Vec<(String, String)> {
        let mut dict: Vec<(String, String)> = Vec::new();
        let option_file_path = self.get_option_file_path();
        if !option_file_path.is_file() {
            return dict;
        }

        let content = fs::read_to_string(&option_file_path)
            .unwrap_or_else(|e| panic!("读取 options.txt 失败（{option_file_path:?}）: {e}"));
        // 源 ReadAllLines（UTF-8 默认编码）自动剥离 BOM
        let content = content.strip_prefix('\u{FEFF}').unwrap_or(&content);

        for line in content.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let sep = if line.contains('=') { '=' } else { ':' };
            if let Some((key, value)) = line.split_once(sep) {
                upsert(&mut dict, key.trim().to_string(), value.trim().to_string());
            }
        }
        dict
    }

    /// 写回 options.txt（源：Save(Dictionary)）：
    /// 每行 `{key}:{value}`（冒号分隔 + `\n`，源 WriteLine），UTF-8 无 BOM（源 StreamWriter
    /// 默认编码）；StreamWriter 截断语义 == File::create；写失败 → panic! 带路径与 io 错误。
    fn save_internal(&self, config: &[(String, String)]) {
        let option_file_path = self.get_option_file_path();
        let file = fs::File::create(&option_file_path)
            .unwrap_or_else(|e| panic!("创建 options.txt 失败（{option_file_path:?}）: {e}"));
        let mut writer = BufWriter::new(file);
        for (key, value) in config {
            writeln!(writer, "{key}:{value}")
                .unwrap_or_else(|e| panic!("写入 options.txt 失败（{option_file_path:?}）: {e}"));
        }
        writer
            .flush()
            .unwrap_or_else(|e| panic!("刷写 options.txt 失败（{option_file_path:?}）: {e}"));
    }
}

/// 字典语义 upsert（同 .NET `dict[key] = value`）：
/// 已存在 key → 原位替换值（.NET Dictionary 保留插入位）；否则追加末尾
fn upsert(dict: &mut Vec<(String, String)>, key: String, value: String) {
    if let Some(entry) = dict.iter_mut().find(|(k, _)| *k == key) {
        entry.1 = value;
    } else {
        dict.push((key, value));
    }
}

/// 区间正则（源：GeneratedRegex RangePattern，逐字）：
/// `^\s*-?\d+(?:\.\d+)?\s*[-–]\s*-?\d+(?:\.\d+)?\s*$`（`[-–]` 含连字符与 en-dash）
fn range_pattern() -> &'static Regex {
    static RANGE_PATTERN: OnceLock<Regex> = OnceLock::new();
    RANGE_PATTERN.get_or_init(|| {
        Regex::new(r"^\s*-?\d+(?:\.\d+)?\s*[-–]\s*-?\d+(?:\.\d+)?\s*$")
            .expect("RangePattern 正则编译失败")
    })
}

impl OptionsProviderApi for OptionsProvider {
    /// 加载选项快照（源：Load）：options.txt 不存在 → 空快照；
    /// 行格式见 load_internal（`#` 注释/空行跳过、`=`/`:` 分隔、Trim）。
    /// 契约返回 GameOptionsSnapshot（HashMap 视图；内部有序 Vec 仅供写回顺序保真）
    fn load(&self) -> GameOptionsSnapshot {
        GameOptionsSnapshot::new(self.load_internal().into_iter().collect())
    }

    /// 设置选项值（源：SetOption(string name, string value)）：
    /// 版本不可用 → 源抛 InvalidOperationException（消息逐字保留）→ panic!；
    /// 写入顺序：加载现有配置（有序）→ upsert 新值 → 整体写回
    fn set_option(&self, name: &str, value: &str) {
        if !self.is_option_available_in_version(name) {
            panic!(
                "Option '{name}' is not available in version '{}'.",
                self.version
            );
        }

        let mut config = self.load_internal();
        upsert(&mut config, name.to_string(), value.to_string());
        self.save_internal(&config);
    }

    /// 设置选项值（源：SetOption(GameOption option) 重载，改名 set_option_from）
    fn set_option_from(&self, option: &GameOption) {
        self.set_option(&option.option_name, &option.option_value);
    }

    /// 获取指定选项的当前值（源：GetCurrentValue）：不存在 → 空串（源 string.Empty）
    fn get_current_value(&self, name: &str) -> String {
        self.load_internal()
            .into_iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
            .unwrap_or_default()
    }

    /// 获取指定选项的值（源：GetOption；与 GetCurrentValue 均返回 string，语义依源实现）
    fn get_option(&self, option_name: &str) -> String {
        self.get_current_value(option_name)
    }

    /// 获取全部选项定义（源：GetDefinitions；逐选项加载配置 + en-US 语言）
    fn get_definitions(&self) -> Vec<OptionDefinition> {
        self.options
            .iter()
            .map(|option| self.to_definition(option))
            .collect()
    }

    /// 按名称获取选项定义（源：GetDefinition，`OptionDefinition?` → Option）
    fn get_definition(&self, name: &str) -> Option<OptionDefinition> {
        self.find_option(name)
            .map(|option| self.to_definition(option))
    }

    /// 获取指定语言的选项展示条目（源：GetOptionViewItems(string language)）：
    /// 单次加载配置，逐选项生成 OptionDefinition 后按字段拷贝为 OptionViewItem
    fn get_option_view_items(&self, language: &str) -> Vec<OptionViewItem> {
        let config = self.load();
        self.options
            .iter()
            .map(|option| {
                let definition = self.to_definition_lang(option, &config.values, language);
                OptionViewItem {
                    name: definition.name,
                    default_value: definition.default_value,
                    current_value: definition.current_value,
                    description: definition.description,
                    valid_values_raw: definition.valid_values_raw,
                    introduced_version: definition.introduced_version,
                    is_available_in_current_version: definition.is_available_in_current_version,
                    value_kind: definition.value_kind,
                }
            })
            .collect()
    }

    /// 获取 Minecraft 原生选项列表（源：GetOptions）：仅返回当前版本可用的选项定义
    fn get_options(&self) -> Vec<MinecraftOption> {
        self.options
            .iter()
            .filter(|option| self.is_option_available_in_version(&option.name))
            .cloned()
            .collect()
    }

    /// 获取当前选项列表（源：GetCurrentOptions）
    fn get_current_options(&self) -> Vec<GameOption> {
        self.load_internal()
            .into_iter()
            .map(|(option_name, option_value)| GameOption {
                option_name,
                option_value,
            })
            .collect()
    }

    /// 获取全部选项列表（源：GetAllOptions）
    fn get_all_options(&self) -> Vec<GameOption> {
        self.load_internal()
            .into_iter()
            .map(|(option_name, option_value)| GameOption {
                option_name,
                option_value,
            })
            .collect()
    }

    /// 获取选项描述（源：GetDescription(string name)）：en-US 回退语言
    fn get_description(&self, name: &str) -> String {
        self.get_description_lang(name, FALLBACK_LANGUAGE)
    }

    /// 获取指定语言的选项描述（源：GetDescription(string name, string language) 重载，
    /// 改名 get_description_lang）：
    /// 回退链：`language` 命中且非空白 → 返回；否则 `en-US` 命中且非空白 → 返回；
    /// 否则默认 "(无描述)"（每级 `IsNullOrWhiteSpace` → `trim().is_empty()`）
    fn get_description_lang(&self, name: &str, language: &str) -> String {
        if let Some(language_descriptions) = self.descriptions.get(language) {
            if let Some(description) = language_descriptions.get(name) {
                if !description.trim().is_empty() {
                    return description.clone();
                }
            }
        }

        if let Some(fallback_descriptions) = self.descriptions.get(FALLBACK_LANGUAGE) {
            if let Some(fallback_description) = fallback_descriptions.get(name) {
                if !fallback_description.trim().is_empty() {
                    return fallback_description.clone();
                }
            }
        }

        DEFAULT_DESCRIPTION.to_string()
    }
}
