//! Java 版本推荐器（B7，对应源：Services/JavaProvider.cs 的推荐/校验部分）
//!
//! 拆分说明：JavaProvider.cs（1034 行）按职责拆为三个文件（B7）：
//! - scanner.rs：Search（扫描部分，P30，由另一 Translator 实现）
//! - 本文件（recommend.rs）：Recommand / Check / GetRequireMajroVersion / JavaDiff
//! - download.rs：GetPackages（Adoptium / Zulu / BMCLAPI 在线包下载）
//!
//! trait 协调方案：`JavaProvider` trait（api/java.rs）的 4 个方法分属多个文件，
//! 各处分别 `impl JavaProvider` 会 E0119（同一 crate 重复实现同一 trait）；
//! 本文件定义 `JavaRecommender`，提供独立 `recommend` / `check` 方法，
//! trait 整合由 B7 主控收尾统一完成（先例：B6 locator.rs / locator_miss.rs
//! 跨文件拆分契约，见 src/services/version/locator.rs 文件头）。

use crate::error::Error;
use crate::models::java::{JavaResult, JavaState};
use crate::models::version_metadata::CompleteVersionMetadata;

/// Java 版本推荐器（源：JavaProvider 类的 Recommand / Check / GetRequireMajroVersion 部分）。
///
/// 源 JavaProvider 构造函数持有 HttpClient（仅供 GetPackages 使用），
/// 推荐/校验逻辑不依赖 `_http`，故为无字段单元结构体；
/// 若 trait 整合需要统一承载（与 JavaScanner 合并或补字段），由 B7 主控收尾决策。
pub(crate) struct JavaRecommender;

/// 排序用临时结构（源：JavaProvider 内嵌 `private struct JavaDiff`）。
struct JavaDiff {
    /// 与所需大版本的差值（源：`diff = javaResult.MajorVersion - require`）。
    diff: i32,
    /// 候选 Java（源：`java`）。
    java: JavaResult,
}

impl JavaRecommender {
    /// 从候选 Java 列表中推荐适配指定版本元数据的 Java（源：`Recommand`）。
    ///
    /// `java_results` 取 `&[JavaResult]` 切片借用（C# `List<JavaResult>` 只读参数，
    /// 返回后调用方仍可持有），`metadata` 取借用；返回推荐结果所有权。
    ///
    /// 逻辑逐字保留：
    /// - 列表为空 → `Error::Params("Java列表为空")`（源 `ArgumentException`）；
    /// - 按 `MajorVersion - require` 差值升序排序（JavaDiff），取最小非负差值：
    ///   diff < 0 跳过；diff == 0 返回（精确匹配）；diff > 0 时若 require == 8
    ///   抛错（Java 8 必须精确匹配），否则返回该候选；
    /// - 全部 diff < 0（无可满足候选）→ 抛错；
    /// - 找不到的错误消息文本逐字保留：`找不到合适的Java运行时 (需要 Java >= {require})`。
    pub(crate) fn recommend(
        &self,
        java_results: &[JavaResult],
        metadata: &CompleteVersionMetadata,
    ) -> Result<JavaResult, Error> {
        if java_results.is_empty() {
            return Err(Error::Params {
                message: "Java列表为空".to_string(),
                source: None,
            });
        }

        let require = self.get_require_major_version(metadata);
        let mut diff: Vec<JavaDiff> = java_results
            .iter()
            .map(|java| JavaDiff {
                java: java.clone(),
                diff: java.major_version - require,
            })
            .collect();

        diff.sort_by(|a, b| a.diff.cmp(&b.diff));

        for diff_item in &diff {
            if diff_item.diff < 0 {
                continue;
            } else if diff_item.diff == 0 {
                return Ok(diff_item.java.clone());
            } else {
                if require == 8 {
                    return Err(Error::VersionNotFound {
                        message: format!("找不到合适的Java运行时 (需要 Java >= {require})"),
                        source: None,
                    });
                }
                return Ok(diff_item.java.clone());
            }
        }

        Err(Error::VersionNotFound {
            message: format!("找不到合适的Java运行时 (需要 Java >= {require})"),
            source: None,
        })
    }

    /// 校验 Java 与版本元数据是否匹配（源：`Check`，同步 bool 方法）。
    ///
    /// 逐字保留：State != Valid → false；`MajorVersion >= require`；
    /// require == 8 时大版本必须精确等于 8（否则 false）。
    /// 源对 `GetRequireMajroVersion(metadata)` 调用两次（纯函数），
    /// 此处缓存到局部变量 `require`，语义一致。
    pub(crate) fn check(&self, java: &JavaResult, metadata: &CompleteVersionMetadata) -> bool {
        if java.state != JavaState::Valid {
            return false;
        }
        let require = self.get_require_major_version(metadata);
        let useful = java.major_version >= require;
        if require == 8 && useful && java.major_version != 8 {
            return false;
        }
        useful
    }

    /// 获取版本元数据要求的 Java 大版本（源：`GetRequireMajroVersion`；
    /// Rust 侧修正源方法名 Majro 拼写笔误）。
    ///
    /// ⚠️ 源偏差：AOT 源为 `JavaVersion?.MajorVersion ?? throw new
    /// InvalidOperationException("JavaVersion metadata missing")`；按任务规则 3
    /// 映射为缺失时回退默认值 8（与源项目非 AOT 版 JavaHelper.GetRequiredJavaMajor
    /// 的默认 8 行为一致），不抛错。如需逐字保留抛错语义，改用
    /// `ok_or_else(|| Error::VersionMetadata { .. })`。
    fn get_require_major_version(&self, metadata: &CompleteVersionMetadata) -> i32 {
        metadata
            .java_version
            .as_ref()
            .map(|java_version| java_version.major_version)
            .unwrap_or(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::java::{JavaResult, JavaState, JavaType};
    use crate::models::version_metadata::{CompleteVersionMetadata, JavaVersion};

    fn java_result(major: i32) -> JavaResult {
        JavaResult {
            path: format!("C:/java{}", major),
            major_version: major,
            version: format!("{major}.0"),
            state: JavaState::Valid,
            arch: "x64".to_string(),
            r#type: JavaType::JDK,
            discovered_by: "test".to_string(),
            name: format!("Java {major}"),
        }
    }

    fn metadata_with_required(major: i32) -> CompleteVersionMetadata {
        CompleteVersionMetadata {
            id: "test".to_string(),
            main_class: "net.minecraft.client.Main".to_string(),
            inherits_from: None,
            jar: None,
            arguments: None,

            minimum_launcher_version: None,
            java_version: Some(JavaVersion {
                component: "jre-legacy".to_string(),
                major_version: major,
            }),

            asset_index: None,
            downloads: None,
            libraries: vec![],
            release_time: String::new(),
            time: String::new(),
            r#type: String::new(),
        }
    }

    #[test]
    fn recommend_exact_match_wins() {
        let recommender = JavaRecommender;
        let list = vec![java_result(8), java_result(17), java_result(21)];
        let picked = recommender
            .recommend(&list, &metadata_with_required(17))
            .unwrap();
        assert_eq!(picked.major_version, 17);
    }

    #[test]
    fn recommend_higher_version_fallback() {
        let recommender = JavaRecommender;
        let list = vec![java_result(8), java_result(17)];
        // require=21：候选全部 diff<0（源逻辑：循环结束抛错）
        assert!(
            recommender
                .recommend(&list, &metadata_with_required(21))
                .is_err()
        );
        // require=17：17 精确匹配
        let picked = recommender
            .recommend(&list, &metadata_with_required(17))
            .unwrap();
        assert_eq!(picked.major_version, 17);
    }

    #[test]
    fn recommend_java8_requires_exact() {
        let recommender = JavaRecommender;
        // require=8：diff>0 直接抛错（8 必须精确匹配）
        let list = vec![java_result(17)];
        assert!(
            recommender
                .recommend(&list, &metadata_with_required(8))
                .is_err()
        );
        let ok = recommender
            .recommend(&vec![java_result(8)], &metadata_with_required(8))
            .unwrap();
        assert_eq!(ok.major_version, 8);
    }

    #[test]
    fn recommend_empty_list_errors() {
        let recommender = JavaRecommender;
        assert!(
            recommender
                .recommend(&vec![], &metadata_with_required(17))
                .is_err()
        );
    }

    #[test]
    fn check_validates_state_and_major() {
        let recommender = JavaRecommender;
        let meta = metadata_with_required(17);
        assert!(recommender.check(&java_result(17), &meta));
        assert!(!recommender.check(&java_result(8), &meta));
        let mut bad = java_result(17);
        bad.state = JavaState::InvalidPath;
        assert!(!recommender.check(&bad, &meta));
        // require=8 且 major!=8 → false
        assert!(!recommender.check(&java_result(17), &metadata_with_required(8)));
    }
}
