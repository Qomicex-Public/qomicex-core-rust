//! 数据包扫描 + pack.mcmeta 解析（B10，对应源：Services/Expansion/Local/DataPacks.cs）
//!
//! 源类 `DataPacks : LocalResourceBase`（构造函数注入
//! `HttpClient http, string gameDirectory, string version, bool versionSegmented, string apiKey`）。
//! 实现 `DataPacksManager::get_data_pack_list`（api/local.rs）：
//! 1. 扫描数据包目录（zip 文件 + 含 pack.mcmeta 的子目录），版本分段目录为
//!    `{gameDirectory}/versions/{version}/datapacks`，否则 `{gameDirectory}/datapacks`
//! 2. 解析 pack.mcmeta：pack.description / pack.pack_format
//!    （zip 内条目名忽略大小写精确匹配，源 `FullName.Equals(OrdinalIgnoreCase)`）
//! 3. 读取 pack.png 转 Base64（源 `Convert.ToBase64String`：标准字母表 + '=' 填充、无换行）
//! 4. SHA1 + CurseForge 指纹（util::murmurhash2::curse_forge_fingerprint，
//!    对应源 LocalResourceBase::CurseForgeFingerprint）
//! 5. 目录形式的数据包：先在内存中把目录重新压缩为 ZIP 再计算哈希（源 ComputeHashesForFolder）
//! 6. CurseForge / Modrinth 哈希反查补全 CurseForgeId / ModrinthId / Name / Version
//!    （⚠️ UNMAPPED：对应服务 B13 未移植，占位空映射，见 U1）
//!
//! 源 DataPacks.cs 与 Resourcepacks.cs 逻辑逐方法一致（仅目录名不同）→
//! 本文件与 resourcepacks.rs 按源同样保留重复实现（源亦两处重复，逐字翻译优先）。
//!
//! Android 兼容性：纯 Rust（std + 既有依赖 zip/sha1/serde_json），无新增 C 依赖。

use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha1::{Digest, Sha1};

use crate::api::local::DataPacksManager;
use crate::error::Error;
use crate::models::expansion::curseforge::FingerprintsFilesMeta;
use crate::models::expansion::local::DataPackInfo;
use crate::models::expansion::modrinth::ProjectVersionInfo;
use crate::services::installers::installer::InstallerBase;
use crate::util::murmurhash2::curse_forge_fingerprint;
use std::collections::HashMap;

/// 数据包管理器实现（源：class `DataPacks : LocalResourceBase`，DataPacks.cs）。
/// 命名对齐 P44 factory.rs 引用点（`super::datapacks::DataPacks`）。
pub(crate) struct DataPacks {
    /// HTTP 客户端（源：`_http` HttpClient）。
    /// ⚠️ 当前哈希反查占位（U1）未消费，B13 接线后启用
    #[allow(dead_code)]
    http: reqwest::Client,
    /// 游戏根目录（源：`_gameDirectory` gameDirectory）
    game_root: String,
    /// 游戏版本（源：`_version` version）
    version: String,
    /// 版本分段目录（源：`_versionSegmented` versionSegmented）
    version_segmented: bool,
    /// CurseForge API Key（源：`_apiKey` apiKey）。
    /// ⚠️ 当前哈希反查占位（U1）未消费，B13 接线后启用
    #[allow(dead_code)]
    api_key: String,
}

impl DataPacks {
    /// 创建数据包管理器（源：构造函数 `DataPacks(HttpClient, string, string, bool, string)`）
    pub(crate) fn new(
        http: reqwest::Client,
        game_root: String,
        version: String,
        version_segmented: bool,
        api_key: String,
    ) -> Self {
        Self {
            http,
            game_root,
            version,
            version_segmented,
            api_key,
        }
    }

    /// 数据包目录（源：`_versionSegmented
    ///   ? Path.Combine(_gameDirectory, "versions", _version, "datapacks")
    ///   : Path.Combine(_gameDirectory, "datapacks")`）
    fn datapack_directory(&self) -> PathBuf {
        if self.version_segmented {
            Path::new(&self.game_root)
                .join("versions")
                .join(&self.version)
                .join("datapacks")
        } else {
            Path::new(&self.game_root).join("datapacks")
        }
    }

    /// 扫描数据包条目（源：GetDataPackFiles）。
    /// 两遍枚举对齐源 `Directory.GetFiles(..., "*.zip")`（文件）+
    /// `Directory.GetDirectories(...)`（含 pack.mcmeta 的目录）；
    /// 目录不存在 → 空列表（源 `return [];`）；枚举失败 → 错误上抛（源异常传播）。
    fn get_data_pack_files(&self) -> Result<Vec<PathBuf>, Error> {
        let datapack_directory = self.datapack_directory();
        if !datapack_directory.is_dir() {
            return Ok(Vec::new());
        }

        let mut entries: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(&datapack_directory).map_err(|e| Error::Params {
            message: format!(
                "读取数据包目录失败（{}）：{e}",
                datapack_directory.display()
            ),
            source: Some(Box::new(e)),
        })? {
            let path = entry
                .map_err(|e| Error::Params {
                    message: format!(
                        "读取数据包目录失败（{}）：{e}",
                        datapack_directory.display()
                    ),
                    source: Some(Box::new(e)),
                })?
                .path();
            // 源模式 "*.zip"：文件名以 .zip 结尾（Windows 上不区分大小写 → 忽略大小写匹配）
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            {
                entries.push(path);
            }
        }
        for entry in std::fs::read_dir(&datapack_directory).map_err(|e| Error::Params {
            message: format!(
                "读取数据包目录失败（{}）：{e}",
                datapack_directory.display()
            ),
            source: Some(Box::new(e)),
        })? {
            let path = entry
                .map_err(|e| Error::Params {
                    message: format!(
                        "读取数据包目录失败（{}）：{e}",
                        datapack_directory.display()
                    ),
                    source: Some(Box::new(e)),
                })?
                .path();
            // 源：`File.Exists(Path.Combine(dir, "pack.mcmeta"))` 才视为数据包目录
            if path.is_dir() && path.join("pack.mcmeta").is_file() {
                entries.push(path);
            }
        }
        Ok(entries)
    }

    /// 从 zip 读取 pack.mcmeta 并解析为 JSON（源：ReadMcmetaFromZip）。
    /// 读取失败 / 未找到 / JSON 解析失败 → None（源 TryReadFileFromZip 全异常 catch → null
    /// + JsonNode.Parse catch → null）。
    fn read_mcmeta_from_zip(zip_path: &Path) -> Option<serde_json::Value> {
        let bytes = try_read_file_from_zip(zip_path, "pack.mcmeta")?;
        serde_json::from_slice(&bytes).ok()
    }

    /// 从目录读取 pack.mcmeta 并解析为 JSON（源：ReadMcmetaFromFolder，
    /// 文件缺失 / 读取失败 / 解析失败 → None）。
    fn read_mcmeta_from_folder(folder_path: &Path) -> Option<serde_json::Value> {
        let mcmeta_path = folder_path.join("pack.mcmeta");
        if !mcmeta_path.is_file() {
            return None;
        }
        let json_content = std::fs::read_to_string(&mcmeta_path).ok()?;
        serde_json::from_str(&json_content).ok()
    }

    /// 从 zip 读取 pack.png 并转 Base64（源：ReadIconFromZip；
    /// 无图标 / 空文件 → 空字符串，源返回 string.Empty）。
    fn read_icon_from_zip(zip_path: &Path) -> String {
        match try_read_file_from_zip(zip_path, "pack.png") {
            Some(bytes) if !bytes.is_empty() => base64_encode(&bytes),
            _ => String::new(),
        }
    }

    /// 从目录读取 pack.png 并转 Base64（源：ReadIconFromFolder；
    /// 文件缺失 / 读取异常 → 空字符串，源 try/catch → string.Empty）。
    fn read_icon_from_folder(folder_path: &Path) -> String {
        let icon_path = folder_path.join("pack.png");
        if !icon_path.is_file() {
            return String::new();
        }
        match std::fs::read(&icon_path) {
            Ok(bytes) => base64_encode(&bytes),
            Err(_) => String::new(),
        }
    }

    /// 提取 pack.description（源：`mcmeta?["pack"]?["description"]?.ToString() ?? ""`）。
    /// 字符串节点 → 原值；其他类型节点 → JSON 文本（源 JsonNode.ToString() 对非字符串
    /// 节点输出 JSON 表示）；缺失 → 空字符串。
    fn read_description(mcmeta: &Option<serde_json::Value>) -> String {
        let Some(pack) = mcmeta.as_ref().and_then(|m| m.get("pack")) else {
            return String::new();
        };
        match pack.get("description") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    }

    /// 提取 pack.pack_format（源：`mcmeta?["pack"]?["pack_format"]?.GetValue<int>() ?? 0`）。
    /// 决策 D1：源 `GetValue<int>()` 在节点非数字时抛 InvalidOperationException 使整个列表
    /// 失败；Rust 收敛为非数字 / 缺失 → 0，避免单一包格式异常拖垮整个列表。
    fn read_pack_format(mcmeta: &Option<serde_json::Value>) -> i32 {
        mcmeta
            .as_ref()
            .and_then(|m| m.get("pack"))
            .and_then(|p| p.get("pack_format"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32
    }

    /// 计算文件的 SHA1 与 CurseForge 指纹（源：ComputeHashesForFile）。
    /// IO 错误向上传播（源 File.ReadAllBytes 抛异常直接传播，无 try/catch）。
    fn compute_hashes_for_file(file_path: &Path) -> Result<(String, i64), Error> {
        let file_bytes = std::fs::read(file_path).map_err(|e| Error::Params {
            message: format!("读取文件失败（{}）：{e}", file_path.display()),
            source: Some(Box::new(e)),
        })?;
        Ok((sha1_hex(&file_bytes), curse_forge_fingerprint(&file_bytes)))
    }

    /// 目录形式数据包：在内存中把目录全部文件重新压缩为 ZIP 再计算哈希
    /// （源：ComputeHashesForFolder：`Directory.GetFiles(folderPath, "*", AllDirectories)` 递归，
    /// 相对路径 `Path.GetRelativePath(...)` 且 '\' 转 '/' 作为条目名，默认压缩级别）。
    /// ⚠️ UNMAPPED（U2）：zip crate 与 .NET ZipArchive 的压缩产物无法保证字节一致
    /// （默认压缩级别映射 / DOS 时间戳 / 条目顺序差异）→ 目录包哈希与 C# 产物不保证一致，
    /// 影响 CurseForge/Modrinth 反查匹配，需 QA 快照比对评估。
    fn compute_hashes_for_folder(folder_path: &Path) -> Result<(String, i64), Error> {
        let mut files: Vec<PathBuf> = Vec::new();
        Self::collect_files_recursive(folder_path, &mut files)?;

        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for file in files {
            let relative_path = file
                .strip_prefix(folder_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| file.to_string_lossy().into_owned())
                .replace('\\', "/");
            let bytes = std::fs::read(&file).map_err(|e| Error::Params {
                message: format!("读取文件失败（{}）：{e}", file.display()),
                source: Some(Box::new(e)),
            })?;
            archive
                .start_file(
                    relative_path,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .map_err(|e| Error::Params {
                    message: format!("ZIP写入失败（{}）：{e}", file.display()),
                    source: Some(Box::new(e)),
                })?;
            archive.write_all(&bytes).map_err(|e| Error::Params {
                message: format!("ZIP写入失败（{}）：{e}", file.display()),
                source: Some(Box::new(e)),
            })?;
        }
        let cursor = archive.finish().map_err(|e| Error::Params {
            message: format!("ZIP写入失败（{}）：{e}", folder_path.display()),
            source: Some(Box::new(e)),
        })?;
        let zip_bytes = cursor.into_inner();
        Ok((sha1_hex(&zip_bytes), curse_forge_fingerprint(&zip_bytes)))
    }

    /// 递归收集目录下全部文件（源：`Directory.GetFiles(folderPath, "*", SearchOption.AllDirectories)`；
    /// 目录枚举顺序两者均未定义，条目顺序不保证与 .NET 一致，见 U2）。
    fn collect_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
        for entry in std::fs::read_dir(dir).map_err(|e| Error::Params {
            message: format!("读取目录失败（{}）：{e}", dir.display()),
            source: Some(Box::new(e)),
        })? {
            let path = entry
                .map_err(|e| Error::Params {
                    message: format!("读取目录失败（{}）：{e}", dir.display()),
                    source: Some(Box::new(e)),
                })?
                .path();
            if path.is_dir() {
                Self::collect_files_recursive(&path, files)?;
            } else {
                files.push(path);
            }
        }
        Ok(())
    }

    /// 兜底名称（源：`Path.GetFileNameWithoutExtension(entry)`：
    /// 取末段文件名并去掉最后一个扩展名；无扩展名 → 原文件名）。
    fn fallback_name(path: &Path) -> String {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[async_trait]
impl DataPacksManager for DataPacks {
    /// 扫描数据包列表（源：GetDataPackList）。
    /// 目录形式条目 zip 内无法直接读取 mcmeta/图标（zip 文件为子目录打包形式）。
    async fn get_data_pack_list(&self) -> Result<Vec<DataPackInfo>, Error> {
        let entries = self.get_data_pack_files()?;
        let mut sha1_list: Vec<String> = Vec::new();
        let mut m_hash_list: Vec<i64> = Vec::new();
        let mut pack_infos: Vec<DataPackInfo> = Vec::new();

        for entry in &entries {
            let is_directory = entry.is_dir();

            let mcmeta = if is_directory {
                Self::read_mcmeta_from_folder(entry)
            } else {
                Self::read_mcmeta_from_zip(entry)
            };
            let description = Self::read_description(&mcmeta);
            let pack_format = Self::read_pack_format(&mcmeta);

            let icon = if is_directory {
                Self::read_icon_from_folder(entry)
            } else {
                Self::read_icon_from_zip(entry)
            };

            let (sha1, cf_hash) = if is_directory {
                Self::compute_hashes_for_folder(entry)?
            } else {
                Self::compute_hashes_for_file(entry)?
            };

            sha1_list.push(sha1.clone());
            m_hash_list.push(cf_hash);

            pack_infos.push(DataPackInfo {
                name: Self::fallback_name(entry),
                description,
                version: String::new(),
                file_path: entry.to_string_lossy().into_owned(),
                is_directory,
                pack_format,
                icon,
                curse_forge_id: 0,
                modrinth_id: String::new(),
                sha1_hash: sha1,
                cf_hash,
            });
        }

        // ⚠️ UNMAPPED（U1）：源在此构造 `CurseForgeBase(_http, _apiKey)` /
        // `ModrinthBase(_http)` 并调用 `GetInfoFromHashesDictAsync(mHashList)` /
        // `GetProjectVersionsFromHashesDictAsync(sha1List)`（各包裹 try/catch 吞错 →
        // 失败时空字典）。对应实现 services/expansion/{curseforge,modrinth}（B13）
        // 未移植；api/expansion.rs 已有 trait（CurseForgeSource::get_info_from_hashes_dict /
        // ModrinthSource::get_project_versions_from_hashes_dict）。本批暂以空映射占位
        // （语义与源吞错一致：反查失败 → 不补全 ID），B13 完成后在此接线。
        let (cf_dict, mr_dict): (
            HashMap<i64, FingerprintsFilesMeta>,
            HashMap<String, ProjectVersionInfo>,
        ) = (HashMap::new(), HashMap::new());

        for pack_info in &mut pack_infos {
            if let Some(cf_meta) = cf_dict.get(&pack_info.cf_hash) {
                pack_info.curse_forge_id = cf_meta.mod_id;
            }

            if let Some(mr_meta) = mr_dict.get(&pack_info.sha1_hash) {
                pack_info.modrinth_id = mr_meta.project_id.clone();
                // 源：`if (!string.IsNullOrEmpty(mrMeta.Name))`（非空才覆盖）
                if !mr_meta.name.is_empty() {
                    pack_info.name = mr_meta.name.clone();
                }
                // 源：`if (!string.IsNullOrEmpty(mrMeta.VersionNumber))`（可空 + 非空才覆盖）
                if let Some(version_number) = &mr_meta.version_number {
                    if !version_number.is_empty() {
                        pack_info.version = version_number.clone();
                    }
                }
            }
        }

        Ok(pack_infos)
    }
}

/// 从 ZIP 中读取指定文件内容，失败（打不开 / 未找到 / 读取出错）返回 None
/// （源：LocalResourceBase.TryReadFileFromZip，全异常 catch → null；
/// 条目名忽略大小写精确匹配，源 `FullName.Equals(fileName, OrdinalIgnoreCase)`）。
/// 委托 B9 已移植的 `InstallerBase::read_specify_file_from_zip`（相同匹配语义），
/// 以 `.ok()` 收敛源「吞错返回 null」语义。
fn try_read_file_from_zip(zip_path: &Path, file_name: &str) -> Option<Vec<u8>> {
    let zip_path_str = zip_path.to_string_lossy();
    InstallerBase::read_specify_file_from_zip(&zip_path_str, file_name).ok()
}

/// SHA1 十六进制小写（源：`Convert.ToHexString(SHA1.HashData(bytes)).ToLowerInvariant()`）
/// 与源 Resourcepacks.cs 的 ComputeHashesForFile 重复实现保持一致（源亦两处重复）。
fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Base64 编码（源：`Convert.ToBase64String`：标准字母表 + '=' 填充，无换行）。
/// ⚠️ 映射表规划 `base64` crate，但 Cargo.toml 尚未引入（本批禁止改动 Cargo.toml）→
/// 手写最小 RFC 4648 编码器，crate 落地后可替换（输出等价，见 U3）。
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
