//! JavaProvider trait：Java 检测 / 推荐 / 包下载（B3）
//!
//! 对应源文件：Public/Services/IJavaProvider.cs（Qomicex.Core.AOT.Public.Services）
//!
//! 方法映射表：
//! - `Task<List<JavaResult>> Search(JavaSearchOptions options)` → `search(&self, options: &JavaSearchOptions) -> Result<Vec<JavaResult>, Error>`
//! - `Task<JavaResult> Recommand(List<JavaResult> javaResults, CompleteVersionMetadata metadata)` → `recommand(&self, java_results: &[JavaResult], metadata: &CompleteVersionMetadata) -> Result<JavaResult, Error>`
//! - `bool Check(JavaResult java, CompleteVersionMetadata metadata)`（同步）→ `check(&self, java: &JavaResult, metadata: &CompleteVersionMetadata) -> bool`
//! - `Task<List<JavaPackageInfo>> GetPackages(int majorVersion, JavaPlatform platform, JavaArchitecture architecture, JavaPackageType packageType, JavaDownloadSource source = JavaDownloadSource.Adoptium)` → `get_packages(&self, major_version: i32, platform: JavaPlatform, architecture: JavaArchitecture, package_type: JavaPackageType, source: JavaDownloadSource) -> Result<Vec<JavaPackageInfo>, Error>`

use crate::error::Error;
use crate::models::java::{
    JavaArchitecture, JavaDownloadSource, JavaPackageInfo, JavaPackageType, JavaPlatform,
    JavaResult, JavaSearchOptions,
};
use crate::models::version_metadata::CompleteVersionMetadata;
use async_trait::async_trait;

/// Java 提供商（源：IJavaProvider 接口）。
///
/// 负责扫描本机 Java 环境（Search）、在候选列表中推荐适配当前版本的
/// Java（Recommand）、校验 Java 与版本元数据是否匹配（Check），以及
/// 按平台/架构/类型从指定源获取可下载的 Java 包列表（GetPackages）。
#[async_trait]
pub trait JavaProvider: Send + Sync {
    /// 按搜索选项扫描本机 Java 环境（源：`Search`）。
    ///
    /// `options` 取 `&JavaSearchOptions` 借用（C# 记录为引用语义的只读参数）。
    async fn search(&self, options: &JavaSearchOptions) -> Result<Vec<JavaResult>, Error>;

    /// 从候选 Java 列表中推荐适配指定版本元数据的 Java（源：`Recommand`）。
    ///
    /// `java_results` 取 `&[JavaResult]` 切片借用（C# `List<JavaResult>` 只读参数，
    /// 返回后调用方仍可持有），`metadata` 取借用；返回推荐结果所有权。
    async fn recommand(
        &self,
        java_results: &[JavaResult],
        metadata: &CompleteVersionMetadata,
    ) -> Result<JavaResult, Error>;

    /// 校验 Java 与版本元数据是否匹配（源：`Check`，同步 bool 方法）。
    fn check(&self, java: &JavaResult, metadata: &CompleteVersionMetadata) -> bool;

    /// 获取指定大版本/平台/架构/类型的 Java 包下载列表（源：`GetPackages`）。
    ///
    /// C# 参数 `source` 有默认值 `JavaDownloadSource.Adoptium`，Rust 无默认参数，
    /// 调用方需显式传入。
    async fn get_packages(
        &self,
        major_version: i32,
        platform: JavaPlatform,
        architecture: JavaArchitecture,
        package_type: JavaPackageType,
        source: JavaDownloadSource,
    ) -> Result<Vec<JavaPackageInfo>, Error>;
}
