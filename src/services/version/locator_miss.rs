//! 缺失文件检查（B6，对应 DefaultVersionLocator.cs 的缺失文件检查部分）
//!
//! 拆分说明：源文件 Services/DefaultVersionLocator.cs（765 行）按职责拆为两个文件：
//! - locator.rs：`DefaultVersionLocator` struct 定义 + 本地版本扫描（缓存刷新 / 元数据读取 /
//!   GetLibraries / GetMetaFromJson / GetJsonData / GetVanillaVersion / GetModloaderType 等）
//! - locator_miss.rs（本文件）：缺失文件检查 8 个方法（GetMissLibrariesAsync ×2、
//!   GetMissMainJarAsync ×2、GetMissAssetsAsync ×2、GetMissFilesAsync ×2）
//!   及私有辅助（DownloadAssetIndexAsync / ReplaceMainJarUrl / GetLibraryCheckItems /
//!   ReplaceLibraryUrl）
//!
//! ⚠️ 协同契约（2026-08-06 实测 locator.rs 已由并行 Translator 写入，以实际为准）：
//! 1. locator.rs 已定义 `pub(crate) struct DefaultVersionLocator`，字段（pub(crate)）：
//!    `versions_root_path`（源 _versionsRootPath）/ `game_root_path`（源 _gameRootPath）/
//!    `cache: Mutex<ScanState>`（源 _versionCache/_metadataCache 合入）/ `download_source` /
//!    `http_client`（源 _httpClient）——本文件按此字段名访问。
//! 2. `pub(crate) struct DownloadSource` 已由 locator.rs 定义（含 Default / bmclapi 辅助），
//!    **本文件不再定义**，仅读取字段（libraries_source / main_jar_source /
//!    assets_index_source / assets_source）。
//! 3. locator.rs 已提供固有方法 `get_libraries`（→ Vec<Library>）、`get_meta_from_json`
//!    （→ Result<_, Error>）、`get_json_data`（→ Option<String>），**本文件不重复定义**，
//!    直接调用。注意 get_json_data 的读取失败在 locator.rs 以 `.ok()` 吞掉（p28a 决策），
//!    与源 IOException 传播语义有偏差，属 locator.rs 侧已定案行为。
//! 4. locator.rs 的 `impl VersionLocator for DefaultVersionLocator` 仅含 5 个同步方法，
//!    注释指明 8 个缺失检查方法在本文件。trait 实现唯一性约束下（Rust 不允许同一 trait
//!    impl 拆到多个 impl 块，且重复 impl 为 E0119），本文件实现为**固有方法**
//!    （`pub(crate) async fn`，原生 async fn 无需 async_trait 宏）。
//!    **整合步骤（主 Translator）**：将本文件 8 个方法体并入 locator.rs 的
//!    `impl VersionLocator`（加 `#[async_trait]`），删除本文件的固有 impl 壳即可；
//!    辅助方法已全部 pub(crate)（get_library_check_items / replace_library_url /
//!    replace_main_jar_url / download_asset_index），跨模块可调用。
//!
//! 错误语义（本文件定案）：
//! - 传输层（请求 / 状态码 / 读响应流）→ `Error::Http`
//! - 文件 IO / 目录创建 / 资源索引读与解析 → `Error::DownloadFailed`
//! - 版本 JSON 反序列化失败或结果为 null → `Error::Params`（源 ArgumentException，
//!   locator.rs 的 get_meta_from_json 同语义）
//! - 资源索引镜像全部失败 → 直接抛出最后一次镜像错误（源 `throw lastEx`）；
//!   镜像列表为空 → `Error::DownloadFailed`("所有镜像均无法下载资源索引")

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::services::version::locator::DefaultVersionLocator;
use crate::error::Error;
use crate::models::installer::MissFileInfo;
use crate::models::version_metadata::{AssetIndex, Library};
use crate::util::lib_helper::maven_to_path;
use crate::util::platform::{get_current_arch, get_current_os_name};

/// 资产索引数据（源：JsonContext/AssetIndexDataJsonContext.cs 的 AssetIndexData 记录；
/// 与 completer.rs 内同名私有结构等价，仅本文件 GetMissAssetsAsync 反序列化使用）
#[derive(Deserialize)]
pub(crate) struct AssetIndexData {
    /// 对象集合：哈希 → 资源对象（源：Dictionary<string, AssetObject>；
    /// 源 record 非可空，但缺键/显式 null 时 `indexData?.Objects == null` 提前返回 → Option）
    #[serde(default)]
    pub(crate) objects: Option<HashMap<String, AssetObject>>,
}

/// 单个资源对象（源：AssetObject 记录）
#[derive(Deserialize)]
pub(crate) struct AssetObject {
    /// 文件哈希（SHA1）
    pub(crate) hash: String,
    /// 文件大小（源逻辑同样不使用 → 保留字段仅保形，避免 dead_code 告警）
    #[allow(dead_code)]
    size: i64,
}

/// 固有方法：缺失文件检查的辅助函数（源：DownloadAssetIndexAsync /
/// ReplaceMainJarUrl / GetLibraryCheckItems / ReplaceLibraryUrl）。
/// 8 个缺失检查方法已并入 locator.rs 的 `impl VersionLocator`（B6 合并，见 CHECKPOINT_BATCH_6）。
impl DefaultVersionLocator {
    /// 下载资源索引（源：DownloadAssetIndexAsync，第 236-272 行）。
    /// URL 列表：官方 URL + 镜像 URL（assetsIndexSource 非空时按 3 个前缀替换 + http→https，
    /// 与原始 URL 不同才插入首位）；逐 URL 尝试：2xx → 写文件返回；其余记录最后一次错误；
    /// 全部失败 → 抛最后一次错误（源 `throw lastEx ?? new Exception("所有镜像均无法下载资源索引")`）。
    /// pub(crate)：整合进 locator.rs trait impl 时跨模块调用（协同契约见文件头）。
    pub(crate) async fn download_asset_index(
        &self,
        asset_index: &AssetIndex,
        index_path: &str,
    ) -> Result<(), Error> {
        let mut urls: Vec<String> = vec![asset_index.url.clone()];
        if !self.download_source.assets_index_source.is_empty() {
            // 源 3 个前缀替换 + http:// → https://（String.Replace 全局替换）
            let mirror_url = asset_index
                .url
                .replace(
                    "https://piston-meta.mojang.com/",
                    &self.download_source.assets_index_source,
                )
                .replace(
                    "https://launchermeta.mojang.com/",
                    &self.download_source.assets_index_source,
                )
                .replace(
                    "https://launcher.mojang.com/",
                    &self.download_source.assets_index_source,
                )
                .replace("http://", "https://");
            // 源：mirrorUrl != assetIndex.Url 才 Insert(0, ...)（http→https 修正后可能完全相同）
            if mirror_url != asset_index.url {
                urls.insert(0, mirror_url);
            }
        }

        // 源：foreach + try/catch，成功即返回，失败记录最后一次异常
        let mut last_error: Option<Error> = None;
        for url in &urls {
            match self.http_client.get(url).send().await {
                Ok(response) if response.status().is_success() => {
                    // 源：response.Content.ReadAsStringAsync()（失败落入 catch）
                    let content = match response.text().await {
                        Ok(c) => c,
                        Err(e) => {
                            last_error = Some(Error::Http {
                                message: format!("下载资源索引失败 ({url}): {e}"),
                                status: None,
                                source: Some(Box::new(e)),
                            });
                            continue;
                        }
                    };
                    // 源：Directory.CreateDirectory(Path.GetDirectoryName(indexPath)!)
                    if let Some(parent) = Path::new(index_path).parent() {
                        if !parent.as_os_str().is_empty() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                last_error = Some(Error::DownloadFailed {
                                    message: format!("下载资源索引失败 ({url}): {e}"),
                                    source: Some(Box::new(e)),
                                });
                                continue;
                            }
                        }
                    }
                    // 源：File.WriteAllTextAsync(indexPath, content) 后 return
                    if let Err(e) = std::fs::write(index_path, content) {
                        last_error = Some(Error::DownloadFailed {
                            message: format!("下载资源索引失败 ({url}): {e}"),
                            source: Some(Box::new(e)),
                        });
                        continue;
                    }
                    return Ok(());
                }
                Ok(response) => {
                    // 源：!IsSuccessStatusCode → 记录 ReasonPhrase（reqwest 无等价物，用状态码文本）
                    last_error = Some(Error::Http {
                        message: format!("下载资源索引失败 ({url}): {}", response.status()),
                        status: None,
                        source: None,
                    });
                }
                Err(e) => {
                    last_error = Some(Error::Http {
                        message: format!("下载资源索引失败 ({url}): {e}"),
                        status: None,
                        source: Some(Box::new(e)),
                    });
                }
            }
        }

        // 源：throw lastEx ?? new Exception("所有镜像均无法下载资源索引")
        Err(last_error.unwrap_or_else(|| Error::DownloadFailed {
            message: "所有镜像均无法下载资源索引".to_string(),
            source: None,
        }))
    }

    /// 主 Jar URL 镜像替换（源：ReplaceMainJarUrl，第 287-295 行，规则逐字保留）。
    /// mainJarSource 为空 → 原样返回；否则依次替换 4 个官方前缀为 mainJarSource。
    pub(crate) fn replace_main_jar_url(&self, url: &str) -> String {
        if self.download_source.main_jar_source.is_empty() {
            return url.to_string();
        }
        url.replace(
            "https://piston-meta.mojang.com/",
            &self.download_source.main_jar_source,
        )
        .replace(
            "https://launchermeta.mojang.com/",
            &self.download_source.main_jar_source,
        )
        .replace(
            "https://launcher.mojang.com/",
            &self.download_source.main_jar_source,
        )
        .replace(
            "https://piston-data.mojang.com/",
            &self.download_source.main_jar_source,
        )
    }

    /// 生成单个库文件的检查项（源：GetLibraryCheckItems，第 297-328 行，判定逻辑逐字保留）：
    /// 1. 主工件（Downloads.Artifact）：路径为空时按 Maven 坐标生成路径；
    ///    路径仍为空 → **直接返回已收集项**（源 `return items`，跳过后续 natives/裸坐标分支）；
    /// 2. natives 分类器（Natives 非空 且 Downloads.Classifiers 非空）：按当前 OS 取 classifier 键
    ///    （${arch} 替换为当前架构位数），命中则加入 `{name}:{classifierKey}` 项；
    /// 3. Downloads == null（Rust 模型 downloads 必填 → 以 artifact/classifiers 均为空近似，
    ///    ⚠️ 见 lib_helper.rs is_class_path 的偏差注记）且名称非空：Maven 坐标转路径，
    ///    非空则加入 `{librariesSource}{path}` 项。
    /// 说明：natives/classifiers 判定按源逐字条件实现（与 lib_helper 的 is_class_path/is_natives
    /// 语义不完全一致，为忠实保留源行为不复用，见翻译日志 p28b）。
    pub(crate) fn get_library_check_items(&self, lib: &Library) -> Vec<MissFileInfo> {
        let mut items = Vec::new();

        // 分支1：Downloads?.Artifact（源：artifact != null）
        if let Some(artifact) = &lib.downloads.artifact {
            let lib_path = if !artifact.path.is_empty() {
                artifact.path.clone()
            } else {
                maven_to_path(&lib.name)
            };
            // 源：if (string.IsNullOrEmpty(libPath)) return items;
            if lib_path.is_empty() {
                return items;
            }
            items.push(MissFileInfo {
                name: lib.name.clone(),
                url: self.replace_library_url(&artifact.url, &lib_path),
                sha1: artifact.sha1.clone(),
                path: lib_path,
            });
        }

        // 分支2：Natives != null && Downloads?.Classifiers != null
        if lib.natives.is_some() && lib.downloads.classifiers.is_some() {
            if let (Some(natives), Some(classifiers)) =
                (&lib.natives, &lib.downloads.classifiers)
            {
                let os_name = get_current_os_name();
                if let Some(native_classifier) = natives.get(os_name) {
                    // 源：nativeClassifier.Replace("${arch}", SystemHelper.GetCurrentArch())
                    let classifier_key =
                        native_classifier.replace("${arch}", get_current_arch());
                    if let Some(native_artifact) = classifiers.get(&classifier_key) {
                        items.push(MissFileInfo {
                            name: format!("{}:{}", lib.name, classifier_key),
                            url: self
                                .replace_library_url(&native_artifact.url, &native_artifact.path),
                            sha1: native_artifact.sha1.clone(),
                            path: native_artifact.path.clone(),
                        });
                    }
                }
            }
        }

        // 分支3：Downloads == null（近似）&& Name 非空
        if lib.downloads.artifact.is_none()
            && lib.downloads.classifiers.is_none()
            && !lib.name.is_empty()
        {
            let path = maven_to_path(&lib.name);
            if !path.is_empty() {
                items.push(MissFileInfo {
                    name: lib.name.clone(),
                    url: format!("{}{}", self.download_source.libraries_source, path),
                    sha1: String::new(),
                    path,
                });
            }
        }

        items
    }

    /// 库 URL 镜像替换（源：ReplaceLibraryUrl，第 330-335 行，规则逐字保留）。
    /// C# 参数为 `string?`，调用处均传非空 URL → Rust `&str`；
    /// URL 为空（C# null/空串同判）→ `{librariesSource}{path}`；否则替换
    /// "https://libraries.minecraft.net/" 前缀为 librariesSource。
    pub(crate) fn replace_library_url(&self, url: &str, path: &str) -> String {
        if url.is_empty() {
            return format!("{}{}", self.download_source.libraries_source, path);
        }
        url.replace(
            "https://libraries.minecraft.net/",
            &self.download_source.libraries_source,
        )
    }
}












