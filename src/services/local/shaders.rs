//! 光影包扫描（B10，对应源：Services/Expansion/Local/Shaders.cs）
//!
//! 源类 `Shaders : LocalResourceBase`（构造函数注入
//! `HttpClient http, string gameDirectory, string version, bool versionSegmented, string apiKey`）。
//! 实现 `ShadersManager::get_shader_list`（api/local.rs）：
//! 1. 扫描 shaderpacks 目录下的 *.zip 文件，版本分段目录为
//!    `{gameDirectory}/versions/{version}/shaderpacks`，否则 `{gameDirectory}/shaderpacks`
//! 2. 计算 SHA1 + CurseForge 指纹（util::murmurhash2::curse_forge_fingerprint，
//!    对应源 LocalResourceBase::CurseForgeFingerprint）
//! 3. CurseForge / Modrinth 哈希反查补全 CurseForgeId / ModrinthId / Name / Version
//!    （⚠️ UNMAPPED：对应服务 B13 未移植，占位空映射，见 U1）
//!
//! Android 兼容性：纯 Rust（std + 既有依赖 sha1），无新增 C 依赖。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha1::{Digest, Sha1};

use crate::api::local::ShadersManager;
use crate::error::Error;
use crate::models::expansion::curseforge::FingerprintsFilesMeta;
use crate::models::expansion::local::ShaderInfo;
use crate::models::expansion::modrinth::ProjectVersionInfo;
use crate::util::murmurhash2::curse_forge_fingerprint;

/// 光影包管理器实现（源：class `Shaders : LocalResourceBase`，Shaders.cs）。
pub(crate) struct ShadersService {
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

impl ShadersService {
    /// 创建光影包管理器（源：构造函数 `Shaders(HttpClient, string, string, bool, string)`）
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

    /// 光影包目录（源：`_versionSegmented
    ///   ? Path.Combine(_gameDirectory, "versions", _version, "shaderpacks")
    ///   : Path.Combine(_gameDirectory, "shaderpacks")`）
    fn shader_directory(&self) -> PathBuf {
        if self.version_segmented {
            Path::new(&self.game_root)
                .join("versions")
                .join(&self.version)
                .join("shaderpacks")
        } else {
            Path::new(&self.game_root).join("shaderpacks")
        }
    }

    /// 扫描光影包文件（源：GetShaderFiles，仅 *.zip 文件，无目录形式）。
    /// 目录不存在 → 空列表（源 `return [];`）；枚举失败 → 错误上抛（源异常传播）。
    fn get_shader_files(&self) -> Result<Vec<PathBuf>, Error> {
        let shader_directory = self.shader_directory();
        if !shader_directory.is_dir() {
            return Ok(Vec::new());
        }

        let mut files: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(&shader_directory).map_err(|e| Error::Params {
            message: format!("读取光影包目录失败（{}）：{e}", shader_directory.display()),
            source: Some(Box::new(e)),
        })? {
            let path = entry
                .map_err(|e| Error::Params {
                    message: format!("读取光影包目录失败（{}）：{e}", shader_directory.display()),
                    source: Some(Box::new(e)),
                })?
                .path();
            // 源模式 "*.zip"：文件名以 .zip 结尾（Windows 上不区分大小写 → 忽略大小写匹配）
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            {
                files.push(path);
            }
        }
        Ok(files)
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

    /// 兜底名称（源：`Path.GetFileNameWithoutExtension(file)`：
    /// 取末段文件名并去掉最后一个扩展名；无扩展名 → 原文件名）。
    fn fallback_name(path: &Path) -> String {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[async_trait]
impl ShadersManager for ShadersService {
    /// 扫描光影包列表（源：GetShaderList）。
    async fn get_shader_list(&self) -> Result<Vec<ShaderInfo>, Error> {
        let files = self.get_shader_files()?;
        let mut sha1_list: Vec<String> = Vec::new();
        let mut m_hash_list: Vec<i64> = Vec::new();
        let mut shader_infos: Vec<ShaderInfo> = Vec::new();

        for file in &files {
            let (sha1, cf_hash) = Self::compute_hashes_for_file(file)?;
            sha1_list.push(sha1.clone());
            m_hash_list.push(cf_hash);

            shader_infos.push(ShaderInfo {
                name: Self::fallback_name(file),
                description: String::new(),
                version: String::new(),
                file_path: file.to_string_lossy().into_owned(),
                icon: String::new(),
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

        for shader_info in &mut shader_infos {
            if let Some(cf_meta) = cf_dict.get(&shader_info.cf_hash) {
                shader_info.curse_forge_id = cf_meta.mod_id;
            }

            if let Some(mr_meta) = mr_dict.get(&shader_info.sha1_hash) {
                shader_info.modrinth_id = mr_meta.project_id.clone();
                // 源：`if (!string.IsNullOrEmpty(mrMeta.Name))`（非空才覆盖）
                if !mr_meta.name.is_empty() {
                    shader_info.name = mr_meta.name.clone();
                }
                // 源：`if (!string.IsNullOrEmpty(mrMeta.VersionNumber))`（可空 + 非空才覆盖）
                if let Some(version_number) = &mr_meta.version_number {
                    if !version_number.is_empty() {
                        shader_info.version = version_number.clone();
                    }
                }
            }
        }

        Ok(shader_infos)
    }
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
