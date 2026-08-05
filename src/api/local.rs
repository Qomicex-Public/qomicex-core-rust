//! LocalResourcesFactory trait：本地内容管理器工厂（B3）
//!
//! 对应源文件：Services/Expansion/Local/ILocalModsFactory.cs
//! （接口名 `ILocalResourcesFactory`，声明于该文件内，
//! namespace Qomicex.Core.AOT.Services.Expansion.Local）
//!
//! 方法映射表：
//! - `CreateMods(string version, bool versionSegmented, string apiKey) -> Mods`
//!   → `create_mods(&self, version: &str, version_segmented: bool, api_key: &str) -> Box<dyn ModsManager + Send + Sync>`
//! - `CreateSaves(...) -> Saves` → `create_saves(...) -> Box<dyn SavesManager + Send + Sync>`
//! - `CreateResourcepack(...) -> Resourcepack` → `create_resourcepack(...) -> Box<dyn ResourcepackManager + Send + Sync>`
//! - `CreateShaders(...) -> Shaders` → `create_shaders(...) -> Box<dyn ShadersManager + Send + Sync>`
//! - `CreateScreenshots(...) -> Screenshots` → `create_screenshots(...) -> Box<dyn ScreenshotsManager + Send + Sync>`
//! - `CreateDataPacks(...) -> DataPacks` → `create_data_packs(...) -> Box<dyn DataPacksManager + Send + Sync>`
//!   （6 个方法参数形态一致，均为同步方法，无 Task → 普通 fn）
//!
//! ⚠️ 占位标注：源方法返回的是 concrete class（Mods/Saves/Resourcepack/Shaders/
//! Screenshots/DataPacks，均继承 LocalResourceBase，见 Services/Expansion/Local/），
//! 本批按任务规则 3 仅翻译工厂 trait，返回类型以占位 trait 名承接，
//! 管理器实现待后续批次翻译（B10）。
//! 决策（见翻译日志 p18）：C# 工厂每次调用 `new` 新实例（DefaultLocalResourcesFactory
//! 逐次 new）→ Rust 所有权转移 `Box<dyn ...>`；不用 `&dyn`（借用无法持有新创建对象）。

/// 本地内容资源工厂（源：ILocalResourcesFactory）。
/// 按版本号、版本分段目录、API Key 创建各类本地内容管理器
/// （Mods/存档/资源包/光影/截图/数据包）。
pub trait LocalResourcesFactory: Send + Sync {
    /// 创建 Mod 管理器（源：CreateMods）
    fn create_mods(
        &self,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ModsManager + Send + Sync>;

    /// 创建存档管理器（源：CreateSaves）
    fn create_saves(
        &self,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn SavesManager + Send + Sync>;

    /// 创建资源包管理器（源：CreateResourcepack）
    fn create_resourcepack(
        &self,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ResourcepackManager + Send + Sync>;

    /// 创建光影管理器（源：CreateShaders）
    fn create_shaders(
        &self,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ShadersManager + Send + Sync>;

    /// 创建截图管理器（源：CreateScreenshots）
    fn create_screenshots(
        &self,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ScreenshotsManager + Send + Sync>;

    /// 创建数据包管理器（源：CreateDataPacks）
    fn create_data_packs(
        &self,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn DataPacksManager + Send + Sync>;
}

/// ⚠️ 占位 trait：源为 concrete class `Mods`（Services/Expansion/Local/Mods.cs，
/// 继承 LocalResourceBase，含 GetModList/DisableMod/EnableMod 等）。
/// 管理器实现待 B10 翻译，届时替换为本文件内占位定义。
pub trait ModsManager: Send + Sync {}

/// ⚠️ 占位 trait：源为 concrete class `Saves`（Services/Expansion/Local/Saves.cs）。
/// 管理器实现待 B10 翻译。
pub trait SavesManager: Send + Sync {}

/// ⚠️ 占位 trait：源为 concrete class `Resourcepack`（Services/Expansion/Local/Resourcepacks.cs）。
/// 管理器实现待 B10 翻译。
pub trait ResourcepackManager: Send + Sync {}

/// ⚠️ 占位 trait：源为 concrete class `Shaders`（Services/Expansion/Local/Shaders.cs）。
/// 管理器实现待 B10 翻译。
pub trait ShadersManager: Send + Sync {}

/// ⚠️ 占位 trait：源为 concrete class `Screenshots`（Services/Expansion/Local/Screenshots.cs）。
/// 管理器实现待 B10 翻译。
pub trait ScreenshotsManager: Send + Sync {}

/// ⚠️ 占位 trait：源为 concrete class `DataPacks`（Services/Expansion/Local/DataPacks.cs）。
/// 管理器实现待 B10 翻译。
pub trait DataPacksManager: Send + Sync {}
