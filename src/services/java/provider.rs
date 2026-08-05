//! Java 提供商整合（B7 收尾）
//!
//! 源文件：Services/JavaProvider.cs（910 行，一个类三个职责）。
//! 拆分整合（B7 定案）：
//! - scanner.rs：JavaScanner（本地扫描 Search/Quick/Deep/Custom/BFS/注册表/环境变量）
//! - recommend.rs：JavaRecommender（Recommand/Check/GetRequireMajroVersion）
//! - download.rs：JavaDownloader（GetPackages/Adoptium/Zulu/BMCLAPI）
//!
//! 本文件聚合三件套实现 api/java.rs 的 JavaProvider trait
//! （单一 impl 避免 E0119，见 B6 locator 先例）。

use async_trait::async_trait;

use crate::api::java::JavaProvider;
use crate::error::Error;
use crate::models::java::{
    JavaArchitecture, JavaDownloadSource, JavaPackageInfo, JavaPackageType, JavaPlatform,
    JavaResult, JavaSearchOptions,
};
use crate::models::version_metadata::CompleteVersionMetadata;
use crate::services::java::download::JavaDownloader;
use crate::services::java::recommend::JavaRecommender;
use crate::services::java::scanner::JavaScanner;

/// Java 提供商聚合（源：JavaProvider.cs 的公开面）
pub(crate) struct JavaProviderService {
    scanner: JavaScanner,
    recommender: JavaRecommender,
    downloader: JavaDownloader,
}

impl JavaProviderService {
    /// 创建聚合（扫描器需要镜像偏好，下载器共享 http client）
    pub(crate) fn new(
        http_client: reqwest::Client,
        preferred_mirror: crate::models::download::DownloadMirror,
    ) -> Self {
        Self {
            scanner: JavaScanner::new(http_client.clone(), preferred_mirror),
            recommender: JavaRecommender,
            downloader: JavaDownloader::new(http_client),
        }
    }
}

#[async_trait]
impl JavaProvider for JavaProviderService {
    /// 搜索本机 Java（源：Search；源为同步 Task，包装为 async）
    async fn search(&self, options: &JavaSearchOptions) -> Result<Vec<JavaResult>, Error> {
        self.scanner.search(options)
    }

    /// 推荐适配 Java（源：Recommand）
    async fn recommand(
        &self,
        java_results: &[JavaResult],
        metadata: &CompleteVersionMetadata,
    ) -> Result<JavaResult, Error> {
        self.recommender.recommend(java_results, metadata)
    }

    /// 校验 Java 与版本元数据是否匹配（源：Check）
    fn check(&self, java: &JavaResult, metadata: &CompleteVersionMetadata) -> bool {
        self.recommender.check(java, metadata)
    }

    /// 获取 Java 包下载列表（源：GetPackages）
    async fn get_packages(
        &self,
        major_version: i32,
        platform: JavaPlatform,
        architecture: JavaArchitecture,
        package_type: JavaPackageType,
        source: JavaDownloadSource,
    ) -> Result<Vec<JavaPackageInfo>, Error> {
        self.downloader
            .get_packages(major_version, platform, architecture, package_type, source)
            .await
    }
}

