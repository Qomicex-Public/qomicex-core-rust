//! DownloadSourceManager trait：镜像切换（B3）
//!
//! 对应源文件：Public/Core/IDownloadSourceManager.cs（Qomicex.Core.AOT.Interfaces.Core）
//!
//! 方法映射表：
//! - `GetAvailableSources(ResourceType) -> IReadOnlyList<DownloadSource>` → `get_available_sources(&self, resource_type) -> Vec<DownloadSource>`
//! - `GenerateMirrorUrls(string, ResourceType) -> IEnumerable<string>` → `generate_mirror_urls(&self, original_url, resource_type) -> Vec<String>`
//! - `TestSourceAsync(DownloadSource) -> Task<bool>` → `test_source(&self, source) -> Result<bool, Error>`
//! - `AddCustomSource(DownloadSource)`（同步 void）→ `add_custom_source(&mut self, source)`
//! - `GetPreferredSource(ResourceType) -> DownloadSource?` → `get_preferred_source(&self, resource_type) -> Result<Option<DownloadSource>, Error>`

use crate::error::Error;
use crate::models::download::{DownloadSource, ResourceType};
use async_trait::async_trait;

/// 下载源管理器（源：IDownloadSourceManager 接口）。
///
/// 负责按资源类型提供可用下载源、生成镜像 URL、测试源可用性、
/// 注册自定义源并挑选首选源。
#[async_trait]
pub trait DownloadSourceManager: Send + Sync {
    /// 获取指定资源类型的全部可用下载源（源：`GetAvailableSources`）。
    ///
    /// 同步方法，返回 Vec 所有权（C# `IReadOnlyList<DownloadSource>` 只读集合，
    /// 返回后调用方可自由持有，无需借用）。
    fn get_available_sources(&self, resource_type: ResourceType) -> Vec<DownloadSource>;

    /// 基于原始 URL 与资源类型生成镜像 URL 列表（源：`GenerateMirrorUrls`）。
    ///
    /// `original_url` 取 `&str` 借用（只读输入，不持有）；返回 `Vec<String>`
    /// 所有权（C# `IEnumerable<string>` 惰性枚举 → Rust 一次性收集）。
    fn generate_mirror_urls(&self, original_url: &str, resource_type: ResourceType) -> Vec<String>;

    /// 测试下载源是否可用（源：`TestSourceAsync`）。
    ///
    /// `source` 取 `&DownloadSource` 借用（C# 记录为引用语义的只读参数）。
    async fn test_source(&self, source: &DownloadSource) -> Result<bool, Error>;

    /// 注册自定义下载源（源：`AddCustomSource`，同步 void）。
    ///
    /// 语义上修改内部源集合，故取 `&mut self` 可变借用；`DownloadSource` 按值
    /// 移入（C# 引用类型入参 → Rust 所有权转移）。
    fn add_custom_source(&mut self, source: DownloadSource);

    /// 获取指定资源类型的首选下载源（源：`GetPreferredSource`，返回 `DownloadSource?`）。
    ///
    /// C# 可空返回映射为 `Option<DownloadSource>`；按 B3 签名规则包装为
    /// `Result<Option<DownloadSource>, Error>` 以统一错误通道。
    fn get_preferred_source(
        &self,
        resource_type: ResourceType,
    ) -> Result<Option<DownloadSource>, Error>;
}
