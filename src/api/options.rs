//! OptionsProvider trait：options.txt 读写（B3）
//!
//! 对应源文件：Public/Services/IOptionsProvider.cs（namespace Qomicex.Core.AOT.Public.Services）
//!
//! 方法映射表：
//! - `GameOptionsSnapshot Load()` → `load(&self) -> GameOptionsSnapshot`
//! - `void SetOption(string name, string value)` → `set_option(&self, name: &str, value: &str)`
//! - `void SetOption(GameOption option)`（重载 → 改名 `set_option_from`，见日志重命名决策）
//!   → `set_option_from(&self, option: &GameOption)`
//! - `string GetCurrentValue(string name)` → `get_current_value(&self, name: &str) -> String`
//! - `string GetOption(string optionName)` → `get_option(&self, option_name: &str) -> String`
//! - `List<OptionDefinition> GetDefinitions()` → `get_definitions(&self) -> Vec<OptionDefinition>`
//! - `OptionDefinition? GetDefinition(string name)` → `get_definition(&self, name: &str) -> Option<OptionDefinition>`
//! - `List<OptionViewItem> GetOptionViewItems(string language)` → `get_option_view_items(&self, language: &str) -> Vec<OptionViewItem>`
//! - `List<MinecraftOption> GetOptions()` → `get_options(&self) -> Vec<MinecraftOption>`
//! - `List<GameOption> GetCurrentOptions()` → `get_current_options(&self) -> Vec<GameOption>`
//! - `List<GameOption> GetAllOptions()` → `get_all_options(&self) -> Vec<GameOption>`
//! - `string GetDescription(string name)` → `get_description(&self, name: &str) -> String`
//! - `string GetDescription(string name, string language)`（重载 → 改名 `get_description_lang`，
//!   见日志重命名决策）→ `get_description_lang(&self, name: &str, language: &str) -> String`
//!
//! 全部为同步方法，无 Task → 普通 fn；`string?` 返回 → `Option<T>`；`List<T>` → `Vec<T>`。
//!
//! ⚠️ 缺失/UNMAPPED 标注：以下模型定义于源 Services/Options/ServiceTypes.cs
//! （"#region 游戏选项类型"），Rust 侧 models 尚未落位，模型批次补齐前不编译，属预期：
//! - `GameOption`：MAPPING_TABLE.yaml models 段已登记 → models::local，仅缺失（待模型批次）。
//! - `GameOptionsSnapshot` / `OptionDefinition` / `OptionViewItem` / `MinecraftOption`：
//!   映射表 models 段未登记 → ⚠️ UNMAPPED，按命名约定建议 `crate::models::local::*`
//!   （建议路径，未定案，登记时确认）。

use crate::models::local::{
    GameOption, GameOptionsSnapshot, MinecraftOption, OptionDefinition, OptionViewItem,
};

/// 游戏选项提供方（源：IOptionsProvider）。
/// 负责 options.txt 的读取/快照/写入，以及 Minecraft 选项定义
/// （OptionDefinition/MinecraftOption）与本地化描述（GetOptionViewItems/GetDescription）。
pub trait OptionsProvider: Send + Sync {
    /// 加载选项快照（源：Load，同步方法）
    fn load(&self) -> GameOptionsSnapshot;

    /// 按名称设置选项值（源：SetOption(string name, string value)，同步方法）
    fn set_option(&self, name: &str, value: &str);

    /// 按 GameOption 条目设置选项（源：SetOption(GameOption option) 重载，改名 `set_option_from`）
    fn set_option_from(&self, option: &GameOption);

    /// 获取指定选项的当前值（源：GetCurrentValue，同步方法）
    fn get_current_value(&self, name: &str) -> String;

    /// 获取指定选项的值（源：GetOption，同步方法；与 GetCurrentValue 均返回 string，语义依源实现区分）
    fn get_option(&self, option_name: &str) -> String;

    /// 获取全部选项定义（源：GetDefinitions，同步方法）
    fn get_definitions(&self) -> Vec<OptionDefinition>;

    /// 按名称获取选项定义（源：GetDefinition，返回 `OptionDefinition?` → `Option<OptionDefinition>`，同步方法）
    fn get_definition(&self, name: &str) -> Option<OptionDefinition>;

    /// 获取指定语言的选项展示条目（源：GetOptionViewItems(string language)，同步方法）
    fn get_option_view_items(&self, language: &str) -> Vec<OptionViewItem>;

    /// 获取 Minecraft 原生选项列表（源：GetOptions，同步方法）
    fn get_options(&self) -> Vec<MinecraftOption>;

    /// 获取当前选项列表（源：GetCurrentOptions，同步方法）
    fn get_current_options(&self) -> Vec<GameOption>;

    /// 获取全部选项列表（源：GetAllOptions，同步方法）
    fn get_all_options(&self) -> Vec<GameOption>;

    /// 获取选项描述（源：GetDescription(string name)，同步方法）
    fn get_description(&self, name: &str) -> String;

    /// 获取指定语言的选项描述（源：GetDescription(string name, string language) 重载，
    /// 改名 `get_description_lang`）
    fn get_description_lang(&self, name: &str, language: &str) -> String;
}
