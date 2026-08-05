//! InstallerProvider trait：可用模组加载器查询（B3）
//!
//! 对应源文件：Public/Services/IInstallerProvider.cs（Qomicex.Core.AOT.Public.Services）
//!
//! 方法映射表：
//! - `Task<List<ModLoaderResult>> GetAvailableModLoaders(string gameVersion, ModLoaderType type = ModLoaderType.All)` → `get_available_mod_loaders(&self, game_version: &str, r#type: ModLoaderType) -> Result<Vec<ModLoaderResult>, Error>`
//!
//! 注：InstallerFactory（IInstallerFactory，6 种安装器创建）属另一源文件，后续批次补充。

use crate::error::Error;
use crate::models::installer::{ModLoaderResult, ModLoaderType};

/// 安装器提供商（源：IInstallerProvider 接口）。
///
/// 负责查询指定游戏版本可用的模组加载器列表。
pub trait InstallerProvider: Send + Sync {
    /// 获取指定游戏版本可用的模组加载器列表（源：`GetAvailableModLoaders`）。
    ///
    /// C# 参数 `type` 有默认值 `ModLoaderType.All`，Rust 无默认参数，调用方需显式
    /// 传入；`type` 为 Rust 关键字，参数命名 `r#type`（与 ModLoaderResult 模型
    /// 字段命名一致）。
    async fn get_available_mod_loaders(
        &self,
        game_version: &str,
        r#type: ModLoaderType,
    ) -> Result<Vec<ModLoaderResult>, Error>;
}
