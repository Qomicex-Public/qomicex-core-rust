//! FTB 整合包安装器（B13）
//!
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/Modpacks/FTBModpackInstaller.cs（77 行）
//!
//! 契约（只读，并行批次已落地）：
//! - `Installer` trait / `MissFileData`：src/services/installers/installer.rs（B9）
//! - `InstallerFactory::create_ftb_modpack`：src/api/installer.rs（B9，`HttpClient` → `reqwest::Client` 按值）
//! - FTB 客户端：src/services/expansion/ftb/query.rs（B13，`FtbBase` + `FtbSource::get_version_detail`）
//! - CurseForge 客户端：src/services/expansion/curseforge/query.rs（B13，`CurseForgeBase::get_files_batch`）
//!
//! 流程要点（逐字保留源）：
//! - `InstallAsync`：直接返回 CompletedTask，不写任何文件（源无任何逻辑）→ Ok(())；
//! - `GetMissLibrariesAsync`（para1=versionId、para2=packId、para3=packVersionId）：
//!   1. FTB `GET /modpack/{packId}/{packVersionId}` 版本详情 → 文件清单，跳过
//!      `serverOnly`；路径 `{baseDir}/{path 去 ./ 与前置 /}/{name}`；
//!   2. FTB `GET /modpack/{packId}/{packVersionId}/mods` Mods 详情 → 取全部
//!      `curseFile`（>0 去重）→ CurseForge `POST /v1/mods/files` 批量查下载链接
//!      （源 `new Mods(_httpClient, _cfApiKety)`：Mods 仅继承 CurseForgeBase，
//!      ClassId 常量未被基类逻辑使用，见 curseforge/query.rs 文件头）→
//!      目标路径 `{gameDir}/mods/{filename}`（版本隔离时含 `versions/{versionId}`），
//!      无下载链接/已下架 → Trace 日志跳过；
//! - 日志：源 Console.WriteLine → println!；Trace.WriteLine → eprintln!（B6 约定）；
//! - 进度事件：源无 IProgress → 无需 ProgressReporter。
//!
//! 错误映射：
//! - 源 `new Exception(...)`（业务校验）→ Error::Http（无专属变体，借 Error::Http，
//!   与 InvalidOperationException → Error::Http 惯例一致；⚠️ UNMAPPED 见日志 p58）；
//! - 网络/JSON → Error::Http；批量查询错误传播。
//!
//! Android 兼容性：纯 Rust（std + reqwest + serde），无平台 API 依赖。
//!
//! ⚠️ 微差（正常响应不触发，均已在代码内标注）：
//! - `json?.Files` / `modsInfo?.Mods` 为 null 时源 foreach 抛 NullReferenceException → 按空列表；
//! - `string sha1 = file.Sha1`（string?）源可传 null 进 record → Rust 空串；
//! - Distinct 保序去重 → sort_unstable + dedup（批量查询结果按 fileId 建映射，顺序无关）；
//! - int.TryParse 失败 → 0（同源）。

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::api::expansion::FtbSource;
use crate::error::Error;
use crate::services::expansion::curseforge::query::CurseForgeBase;
use crate::services::expansion::ftb::query::FtbBase;
use crate::services::installers::installer::{Installer, MissFileData};

/// FTB 整合包安装器（源：`internal class FTBModpackInstaller : InstallerBase, IInstaller`）。
pub(crate) struct FtbModpackInstaller {
    /// 游戏根目录（源：`_gameDir`）
    game_dir: String,
    /// 是否版本隔离（源：`_versionIsolation`，true 时路径含 `versions/{versionId}`）
    version_isolation: bool,
    /// 共享 HTTP 客户端（源：`_httpClient`，用于构造 CurseForge Mods 客户端）
    http_client: reqwest::Client,
    /// CurseForge API 密钥（源：`_cfApiKety`）
    cf_api_key: String,
    /// FTB 数据源（源：`_ftb` FTBBase）
    ftb: FtbBase,
}

impl FtbModpackInstaller {
    /// 创建安装器（源：构造函数
    /// `FTBModpackInstaller(string gameDir, bool versionIsolation, HttpClient httpClient, string cfApiKey)`；
    /// `HttpClient` → `reqwest::Client` 按值传递，语义同 api/installer.rs 注释）。
    pub(crate) fn new(
        game_dir: &str,
        version_isolation: bool,
        http_client: reqwest::Client,
        cf_api_key: &str,
    ) -> Self {
        Self {
            game_dir: game_dir.to_string(),
            version_isolation,
            http_client: http_client.clone(),
            cf_api_key: cf_api_key.to_string(),
            // 源：_ftb = new FTBBase(httpClient)（默认 baseUrl/cacheDir）
            ftb: FtbBase::new(http_client, None, None),
        }
    }
}

#[async_trait]
impl Installer for FtbModpackInstaller {
    /// 执行安装（源：`Task IInstaller.InstallAsync(string versionId, string inheritsFromJson,
    /// string packId, string packVersionId, string? para3, string? para4)`——para1=packId、
    /// para2=packVersionId；源直接 `return Task.CompletedTask`，不写任何文件 → Ok(())）。
    async fn install(
        &self,
        _version_id: &str,
        _inherits_from_json: &str,
        _para1: Option<&str>,
        _para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// 获取缺失库列表（源：`async Task<List<MissFileData>> GetMissLibrariesAsync(
    /// string versionId, string packId, string packVersionId)`——para1=versionId、
    /// para2=packId、para3=packVersionId）。
    async fn get_miss_libraries(
        &self,
        para1: Option<&str>,
        para2: Option<&str>,
        para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        let version_id = para1.unwrap_or_default();
        // 源：int.TryParse(packId, out int _packId) —— 解析失败 → 0
        let pack_id = para2.and_then(|s| s.parse().ok()).unwrap_or(0);
        let pack_version_id = para3.and_then(|s| s.parse().ok()).unwrap_or(0);

        println!("[FTB] 开始解析版本清单 packId={pack_id}, packVersionId={pack_version_id}");
        println!("[FTB] 请求版本详情...");
        // 源：await _ftb.GetVersionDetailAsync(_packId, _packVersionId)
        // （FTBBase 内部错误吞掉 → null → Rust Option::None，不产生 Err）
        let json = self.ftb.get_version_detail(pack_id, pack_version_id).await?;
        println!(
            "[FTB] 版本详情获取完成，文件数={}",
            json.as_ref().and_then(|j| j.files.as_ref()).map_or(0, |f| f.len())
        );

        let Some(detail) = json else {
            // 源：throw new Exception("无法获取整合包信息")
            // ⚠️ UNMAPPED：源 new Exception 无对应 Error 变体，借 Error::Http
            return Err(Error::Http {
                message: "无法获取整合包信息（源 new Exception）".to_string(),
                status: None,
                source: None,
            });
        };

        let mut miss_files = Vec::new();
        // 源：foreach (var file in json?.Files)
        // ⚠️ 微差：Files 为 null 时源 foreach 抛 NullReferenceException → 此处按空列表
        if let Some(files) = detail.files.as_ref() {
            for file in files {
                // 源：if (file.ServerOnly == true) continue;
                if file.server_only {
                    continue;
                }
                // 源：string baseDir = _versionIsolation ? Path.Combine(_gameDir, "versions", versionId) : _gameDir;
                let base_dir = if self.version_isolation {
                    Path::new(&self.game_dir)
                        .join("versions")
                        .join(version_id)
                } else {
                    PathBuf::from(&self.game_dir)
                };
                // 源：string relativePath = (file.Path ?? "").Replace("./", "").TrimStart('/');
                let relative_path = file
                    .path
                    .as_deref()
                    .unwrap_or_default()
                    .replace("./", "")
                    .trim_start_matches('/')
                    .to_string();
                // 源：string path = Path.Combine(baseDir, relativePath, file.Name);
                let path = base_dir
                    .join(&relative_path)
                    .join(&file.name)
                    .to_string_lossy()
                    .into_owned();
                miss_files.push(MissFileData {
                    name: file.name.clone(),
                    path,
                    url: file.url.clone(),
                    // 源：string sha1 = file.Sha1（string?，可能为 null 传入 record）
                    sha1: file.sha1.as_deref().unwrap_or_default().to_string(),
                });
            }
        }

        println!("[FTB] 请求 Mod 清单...");
        // 源：ModsDetail modsInfo = await _ftb.GetModDetailAsync(_packId, _packVersionId)
        // （FTBBase 内部错误吞掉 → null → Rust Option::None）
        let mods_info = self.ftb.get_mod_detail(pack_id, pack_version_id).await;
        println!(
            "[FTB] Mod 清单获取完成，mod数={}",
            mods_info.as_ref().and_then(|m| m.mods.as_ref()).map_or(0, |m| m.len())
        );

        let Some(mods_info) = mods_info else {
            // 源：throw new Exception("无法获取整合包 Mod 信息")
            // ⚠️ UNMAPPED：源 new Exception 无对应 Error 变体，借 Error::Http
            return Err(Error::Http {
                message: "无法获取整合包 Mod 信息（源 new Exception）".to_string(),
                status: None,
                source: None,
            });
        };

        // 源：var _cf = new Services.Expansion.CurseForge.Mods(_httpClient, _cfApiKety);
        // （Mods 仅继承 CurseForgeBase，ClassId=6 常量未被基类逻辑使用，直接用基类；
        //  baseUrl 用默认值，对应源构造函数 base(http, apiKey) 不传 baseUrl）
        let cf = CurseForgeBase::new(self.http_client.clone(), self.cf_api_key.clone(), None);

        // 源：var fileIds = modsInfo.Mods.Select(m => m.FileId).Where(id => id > 0).Distinct().ToList();
        // ⚠️ 微差：Distinct 保首现序 → sort_unstable + dedup（批量查询结果按 fileId 建映射，
        // 顺序无关；FtbModInfo.FileId → curse_file，B1 模型）
        let mut file_ids: Vec<i64> = mods_info
            .mods
            .as_ref()
            .map(|mods| {
                mods.iter()
                    .map(|m| m.curse_file)
                    .filter(|&id| id > 0)
                    .collect()
            })
            .unwrap_or_default();
        file_ids.sort_unstable();
        file_ids.dedup();
        println!(
            "[FTB] 批量查询 CurseForge 下载链接，有效fileId数={}（总数={}）",
            file_ids.len(),
            mods_info.mods.as_ref().map_or(0, |m| m.len())
        );
        // 源：var fileInfoMap = await _cf.GetFilesBatchAsync(fileIds);
        let file_info_map = cf.get_files_batch(&file_ids).await?;
        println!("[FTB] CurseForge 批量查询完成，成功获取={}", file_info_map.len());

        // 源：foreach (FtbModInfo modIndo in modsInfo?.Mods)
        // ⚠️ 微差：Mods 为 null 时源 foreach 抛 NullReferenceException → 此处按空列表
        if let Some(mods) = mods_info.mods.as_ref() {
            for mod_info in mods {
                // 源：string path = _versionIsolation
                //      ? Path.Combine(Path.Combine(_gameDir, "versions", versionId, "mods"), modIndo.FileName)
                //      : Path.Combine(Path.Combine(_gameDir, "mods"), modIndo.FileName);
                // （FtbModInfo.FileName → filename，B1 模型）
                let path = if self.version_isolation {
                    Path::new(&self.game_dir)
                        .join("versions")
                        .join(version_id)
                        .join("mods")
                        .join(&mod_info.filename)
                } else {
                    Path::new(&self.game_dir)
                        .join("mods")
                        .join(&mod_info.filename)
                };
                let path = path.to_string_lossy().into_owned();

                match file_info_map.get(&mod_info.curse_file) {
                    // 源：if (!fileInfoMap.TryGetValue(...) || string.IsNullOrEmpty(info.DownloadUrl))
                    Some(info) if !info.download_url.as_deref().unwrap_or_default().is_empty() => {
                        miss_files.push(MissFileData {
                            name: mod_info.name.clone(),
                            path,
                            url: info.download_url.clone().unwrap_or_default(),
                            // 源：info.Sha1 ?? ""
                            sha1: info.sha1.as_deref().unwrap_or_default().to_string(),
                        });
                    }
                    _ => {
                        // 源：System.Diagnostics.Trace.WriteLine(...) → eprintln!（B6 约定）
                        // （FtbModInfo.ModId → curse_project，B1 模型）
                        eprintln!(
                            "[FTB] 跳过 Mod {} (ID={}, FileID={}): 无下载链接或已下架",
                            mod_info.name, mod_info.curse_project, mod_info.curse_file
                        );
                    }
                }
            }
        }

        println!("[FTB] 清单解析完成，共 {} 个文件需要下载", miss_files.len());
        Ok(miss_files)
    }
}



