//! Modrinth 整合包安装器（B13）
//!
//! 对应源文件：Qomicex.Core.AOT/Services/Installers/Modpacks/ModrinthModpackInstaller.cs（100 行）
//!
//! 契约（只读）：
//! - `Installer` trait / `InstallerBase` 静态工具：src/services/installers/installer.rs（B9）
//! - `InstallerFactory` 的 3 个 create_modpack 方法签名：src/api/installer.rs（B9）
//! - Modrinth API 客户端：src/services/expansion/modrinth/query.rs（B13，并行批次已存在，本文件未直接引用）
//!
//! 流程要点（逐字保留源）：
//! - `InstallAsync`：仅解压整合包 `override/` 目录（固定前缀，非 manifest 配置）到版本目录
//!   （版本隔离时 `{gameDir}/versions/{versionId}`），返回 CompletedTask；
//!   缺失库查询恒返回空列表（依赖下载不在本安装器，与源一致）；
//! - `GetModpackInfo(versionId)`：读 zip 内 `modrinth.index.json`，校验 `game == "minecraft"`；
//!   解析 name/summary/versionId；dependencies 对象逐键：`minecraft` → GameVersion，
//!   `quilt-loader`/`fabric-loader`/`forge`/`neoforge` → ModLoader + ModLoaderVersion；
//!   files 仅收集 `env.client == "required"` 的条目（path 拼接版本目录基路径、hashes.sha1、
//!   downloads[0]、fileSize）——path 为完整目标路径（与 CF 的 ProjectId/FileId 不同）。
//!
//! 错误映射（沿既有先例）：
//! - JsonNode.Parse 失败 / AsObject 非对象 / InvalidOperationException → Error::Http
//!   （fabric/install.rs 先例：JsonException 与 InvalidOperationException 均 → Error::Http）；
//! - ZIP/文件 IO（源 IOException/FileNotFoundException）→ Error::Params
//!   （同 installer.rs `read_specify_file_from_zip` 先例，见 B9 日志 U1）；
//! - 进度事件：源无 IProgress → 无需 ProgressReporter。
//!
//! Android 兼容性：纯 Rust（std + zip + serde_json），无平台 API 依赖。
//!
//! ⚠️ 微差（源显式转换/索引在非预期类型时抛异常，此处容错为默认值，正常响应不触发）：
//! - `(string?)node ?? ""` / `downloads[0]` 非字符串节点源抛 InvalidCastException → 按空串；
//! - `file["env"]?["client"]` 非对象节点源抛 InvalidOperationException → 按 "required"；
//! - 源错误类型详见文件头注释与 B13 日志 p58。

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::Error;
use crate::services::installers::installer::{Installer, MissFileData};

/// Modrinth 整合包安装器（源：`internal sealed class ModrinthModpackInstaller : InstallerBase, IInstaller`）。
pub(crate) struct ModrinthModpackInstaller {
    /// 游戏根目录（源：`_gameDir`）
    game_dir: String,
    /// 是否版本隔离（源：`_versionIsolation`，true 时解压到 `{gameDir}/versions/{versionId}`）
    version_isolation: bool,
    /// 整合包 zip 文件路径（源：`_modpackFilePath`）
    modpack_file_path: String,
}

impl ModrinthModpackInstaller {
    /// 创建安装器（源：构造函数
    /// `ModrinthModpackInstaller(string gameDir, bool versionIsolation, string modpackFilePath)`）。
    pub(crate) fn new(game_dir: &str, version_isolation: bool, modpack_file_path: &str) -> Self {
        Self {
            game_dir: game_dir.to_string(),
            version_isolation,
            modpack_file_path: modpack_file_path.to_string(),
        }
    }

    /// 读取整合包信息（源：`internal ModrinthModpackInfo GetModpackInfo(string versionId)`）。
    ///
    /// 解析 zip 内 `modrinth.index.json` 并提取名称/描述/版本/游戏版本/加载器/文件列表；
    /// 文件路径已按版本隔离规则拼接 `{gameDir}` 或 `{gameDir}/versions/{versionId}` 基路径。

    /// 解压整合包 override 目录到版本目录（源：`ReleaseFiles(string versionId)`）。
    ///
    /// 与 CurseForge 版差异：目录前缀固定为 `override/`（非 manifest 配置），
    /// 逐条保留源逻辑（大小写不敏感匹配、目录条目建目录、文件条目覆盖写入）。
    fn release_files(&self, version_id: &str) -> Result<(), Error> {
        // 源：var versionDir = _versionIsolation ? Path.Combine(_gameDir, "versions", versionId) : _gameDir;
        let version_dir = if self.version_isolation {
            Path::new(&self.game_dir).join("versions").join(version_id)
        } else {
            PathBuf::from(&self.game_dir)
        };
        // 源：if (!Directory.Exists(versionDir)) Directory.CreateDirectory(versionDir)
        std::fs::create_dir_all(&version_dir)
            .map_err(|e| file_io_err(&self.modpack_file_path, e))?;

        let file = std::fs::File::open(&self.modpack_file_path)
            .map_err(|e| file_io_err(&self.modpack_file_path, e))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| zip_io_err(&self.modpack_file_path, e))?;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| zip_io_err(&self.modpack_file_path, e))?;
            let full_name = entry.name();
            // 源：entry.FullName.StartsWith("override/", OrdinalIgnoreCase)
            const PREFIX: &str = "override/";
            if !full_name
                .get(..PREFIX.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(PREFIX))
            {
                continue;
            }
            // 源：string relativePath = entry.FullName.Substring("override/".Length);
            let relative_path = &full_name[PREFIX.len()..];
            // 源：string destinationPath = Path.Combine(versionDir, relativePath);
            let destination_path = version_dir.join(relative_path);

            // 源：if (string.IsNullOrEmpty(entry.Name)) —— .NET ZipArchiveEntry.Name 为
            // FullName 末段，目录条目（尾部 '/'）→ 空串；zip crate 无该属性，按 '/' 取末段
            let last_segment = full_name.rsplit('/').next().unwrap_or("");
            if last_segment.is_empty() {
                // 源：Directory.CreateDirectory(destinationPath)
                std::fs::create_dir_all(&destination_path)
                    .map_err(|e| file_io_err(&self.modpack_file_path, e))?;
            } else {
                // 源：Directory.CreateDirectory(Path.GetDirectoryName(destinationPath)!) 后
                //      entry.ExtractToFile(destinationPath, overwrite: true)
                if let Some(parent) = destination_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| file_io_err(&self.modpack_file_path, e))?;
                }
                let mut out = std::fs::File::create(&destination_path)
                    .map_err(|e| file_io_err(&self.modpack_file_path, e))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| file_io_err(&self.modpack_file_path, e))?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Installer for ModrinthModpackInstaller {
    /// 执行安装（源：`Task IInstaller.InstallAsync(string versionId, string inheritsFromJson,
    /// string? para1..para4)`；para1-4 均未使用——本安装器只解压 override 目录）。
    async fn install(
        &self,
        version_id: &str,
        _inherits_from_json: &str,
        _para1: Option<&str>,
        _para2: Option<&str>,
        _para3: Option<&str>,
        _para4: Option<&str>,
    ) -> Result<(), Error> {
        // 源：ReleaseFiles(versionId); return Task.CompletedTask;
        // （同步体异常经 async 包装为任务失败 → Rust 直接 `?` 传播）
        self.release_files(version_id)?;
        Ok(())
    }

    /// 获取缺失库列表（源：`Task<List<MissFileData>> GetMissLibrariesAsync(...)`
    /// → `Task.FromResult(new List<MissFileData>())`，恒返回空列表）。
    async fn get_miss_libraries(
        &self,
        _para1: Option<&str>,
        _para2: Option<&str>,
        _para3: Option<&str>,
    ) -> Result<Vec<MissFileData>, Error> {
        Ok(Vec::new())
    }
}

// ── 模块私有辅助（对应源内联 JsonNode 操作）────────────────

/// 读取并解析 zip 内 JSON 清单（源：`Encoding.UTF8.GetString(ReadSpecifyFileFromZip(...))` +
/// `JsonNode.Parse(jsonData)!.AsObject()`）。
///
/// UTF-8 解码为有损（对应 .NET Encoding.UTF8 替换无效序列）；Parse 失败 →
/// Error::Http（源 JsonException）；顶层非对象 → Error::Http（源 AsObject() 抛
/// InvalidOperationException）。

/// 源 `(string?)node ?? string.Empty`：字符串节点 → 原文；缺失/JSON null → 空串。
///
/// ⚠️ 非字符串节点源显式转换抛 InvalidCastException，此处按空串容错（微差，见文件头）。

/// 源 `(long?)node ?? 0`（数值节点 → 值；缺失/JSON null → 0）。
///
/// ⚠️ 非数字节点源显式转换抛 InvalidCastException，此处按 0 容错（微差，见文件头）。

/// ZIP/文件 IO 错误包装（源 IOException/FileNotFoundException → 无对应 Error 变体，
/// 沿用 installer.rs `read_specify_file_from_zip` 的 Error::Params 先例，见 B9 日志 U1）。
fn zip_io_err(path: &str, e: zip::result::ZipError) -> Error {
    Error::Params {
        message: format!("读取ZIP失败（{path}）：{e}"),
        source: None,
    }
}

fn file_io_err(path: &str, e: std::io::Error) -> Error {
    Error::Params {
        message: format!("读取ZIP失败（{path}）：{e}"),
        source: Some(Box::new(e)),
    }
}
