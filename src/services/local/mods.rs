//! Mod 扫描 / 元数据 / 启用 / 禁用（B10，对应 Mods.cs）
//!
//! 对应源文件：Services/Expansion/Local/Mods.cs（`Mods : LocalResourceBase`）。
//! 语义要点：
//! - 扫描：mods 目录（版本分段 `{gameDir}/versions/{version}/mods`，否则 `{gameDir}/mods`）
//!   下 `*.jar` / `*.disabled` 文件（源 GetFiles 通配符在 Windows 按扩展名大小写不敏感匹配
//!   → ASCII 忽略大小写）；
//! - 元数据：fabric.mod.json → META-INF/mods.toml（回退 META-INF/neoforge.mods.toml）→
//!   mcmod.info 顺序解析，任一环节失败静默跳过（同源 catch{} 吞错），名称为空时以文件名兜底；
//! - 哈希：SHA1（小写十六进制）+ CurseForge 指纹（基类 LocalResourceBase 委托
//!   util/murmurhash2.rs，见 P44）；
//! - 进度：onProgress(0, total) → 每文件递增（源 Parallel.ForEach → 顺序循环，
//!   ConcurrentBag 结果集本就无序，语义等价）；
//! - 启禁：DisableMod 追加 `.disabled`；EnableMod 去掉 `.disabled` 后缀（大小写不敏感）；
//! - 反查：B13 接线完成——Modrinth SHA1 反查（`v2/version_files`）+ CurseForge 指纹反查
//!   （`v2/fingerprints`），回填 `modrinth_id` / `curse_forge_id`（网络失败静默，同源 catch）。

use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use zip::ZipArchive;

use crate::api::expansion::{CurseForgeSource, ModrinthSource};
use crate::api::local::ModsManager;
use crate::error::Error;
use crate::models::expansion::local::ModInfo;
use crate::services::download::checksum::sha1_hex;
use crate::services::expansion::curseforge::query::CurseForgeBase;
use crate::services::expansion::modrinth::query::ModrinthBase;

use super::factory::LocalResourceBase;

/// Mod 管理器（源：concrete class `Mods`，Services/Expansion/Local/Mods.cs）
pub(crate) struct Mods {
    /// HTTP 客户端（源字段 `_http`；B13 CF/MR 反查接线已完成）
    http: reqwest::Client,
    /// 游戏根目录（源字段 `_gameDirectory`）
    game_directory: String,
    /// 游戏版本（源字段 `_version`，用于版本分段目录）
    version: String,
    /// 是否使用版本分段目录（源字段 `_versionSegmented`）
    version_segmented: bool,
    /// API Key（源字段 `_apiKey`：用于 CurseForge 反查）
    api_key: String,
}

impl Mods {
    /// 创建 Mod 管理器（源：`new Mods(HttpClient, gameDirectory, version, versionSegmented, apiKey)`；
    /// `HttpClient` → `reqwest::Client`，MAPPING_TABLE runtime 映射；
    /// 参数形态与 P44 factory.rs 调用点一致）
    pub(crate) fn new(
        http: reqwest::Client,
        game_directory: String,
        version: String,
        version_segmented: bool,
        api_key: String,
    ) -> Self {
        Self {
            http,
            game_directory,
            version,
            version_segmented,
            api_key,
        }
    }

    /// Mod 目录（源：`ModDirectory` 计算属性）：
    /// `_versionSegmented` → `{gameDirectory}/versions/{version}/mods`，否则 `{gameDirectory}/mods`。
    /// （P44 已确认：目录解析为各管理器类自身属性，源基类不含 → 本类实现）
    fn mod_directory(&self) -> PathBuf {
        if self.version_segmented {
            PathBuf::from(&self.game_directory)
                .join("versions")
                .join(&self.version)
                .join("mods")
        } else {
            PathBuf::from(&self.game_directory).join("mods")
        }
    }

    /// 扫描 Mod 文件（源：GetModFiles）：目录不存在 → 空列表；
    /// 收集 `*.jar` 与 `*.disabled` 文件（源 GetFiles(ModDirectory, "*.jar" / "*.disabled")，
    /// Windows 下通配符按扩展名大小写不敏感匹配 → eq_ignore_ascii_case）。
    /// 差异说明：源 GetFiles 的 IO 异常向上抛出 → 此处 Err(Error::DownloadFailed)（同 checksum.rs 约定）
    fn get_mod_files(&self) -> Result<Vec<String>, Error> {
        let dir = self.mod_directory();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| Error::DownloadFailed {
            message: format!("读取 Mod 目录失败: {}", dir.display()),
            source: Some(Box::new(e)),
        })? {
            let entry = entry.map_err(|e| Error::DownloadFailed {
                message: format!("读取 Mod 目录项失败: {}", dir.display()),
                source: Some(Box::new(e)),
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_jar = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jar"));
            let is_disabled = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("disabled"));
            if is_jar || is_disabled {
                files.push(path.to_string_lossy().into_owned());
            }
        }
        Ok(files)
    }

    /// ⚠️ UNMAPPED（B13 待接线）：对应源 GetModList 尾部的网络反查与图标兜底，本批不实现，
    /// 保留数据流位置与字段（curse_forge_id / modrinth_id / icon）由本地解析填充：
    /// 1. CurseForge：`CurseForgeBase(_http, _apiKey).GetInfoFromHashesDictAsync(cfHashes)`
    ///    → `FingerprintsFilesMeta { ModId > 0 }` → `modInfo.CurseForgeId`；
    /// 2. Modrinth：`ModrinthBase(_http).GetProjectVersionsFromHashesDictAsync(sha1Hashes)`
    ///    → `ProjectVersionInfo.ProjectId` → `modInfo.ModrinthId`；
    /// 3. 图标兜底（Icon 为空者）：ModrinthId 命中 → `Mods.GetProjectInfoAsync` →
    ///    IconUrl 下载为 base64；失败 → CurseForge `Mods.GetModInfoAsync`（源内为空操作）；
    /// 4. 网络失败均静默（源 Trace.WriteLine 日志，不抛错）。
    /// 接线目标：api/expansion.rs 的 CurseForgeSource / ModrinthSource traits，
    /// 实现位于 services/expansion/{curseforge,modrinth}/query.rs（B13）。
    /// 反查补全（源：GetModList 尾部，B13 接线）。
    /// Modrinth：`v2/version_files` 按 SHA1 批量反查 → 回填 modrinth_id；
    /// CurseForge：`v2/fingerprints` 按 CF 指纹批量反查 → 回填 curse_forge_id（无 apiKey 跳过）。
    /// 本地扫描（get_mod_list 主体，不含网络反查）：
    /// 收集 `*.jar` / `*.disabled` → 逐文件 SHA1 + CF 指纹 → 解析元数据 → 名称兜底 → 进度。
    /// ⚠️ 并行化（对齐 C# 源 `Parallel.ForEach` + ConcurrentBag）：SHA1 + zip 解析是
    /// CPU/IO 密集，顺序循环在 180+ mods 时 15s+（超过前端 15s 全局请求超时）；
    /// 任务返回 (index, ModInfo)，按 index 归位保序，进度回调按完成数递增。
    async fn scan_local(
        &self,
        on_progress: &mut Option<&mut (dyn FnMut(i32, i32) + Send)>,
    ) -> Result<Vec<ModInfo>, Error> {
        let mod_files = self.get_mod_files()?;

        // 源：Trace.WriteLine($"Fetching mod list: {_version}, dir: {ModDirectory}, count: {modFiles.Count}")
        // → eprintln!（B6 约定，同 file_helper.rs）
        eprintln!(
            "Fetching mod list: {}, dir: {}, count: {}",
            self.version,
            self.mod_directory().display(),
            mod_files.len()
        );

        let total_count = mod_files.len() as i32;
        if let Some(cb) = on_progress.as_deref_mut() {
            call_progress(cb, 0, total_count);
        }

        let mut tasks = tokio::task::JoinSet::new();
        for (idx, mod_path) in mod_files.iter().enumerate() {
            let mod_path = mod_path.clone();
            tasks.spawn_blocking(move || -> Result<(usize, ModInfo), Error> {
                let bytes = std::fs::read(&mod_path).map_err(|e| Error::DownloadFailed {
                    message: format!("读取 Mod 文件失败: {mod_path}"),
                    source: Some(Box::new(e)),
                })?;

                let hash = sha1_hex(&bytes);
                // 源：CurseForgeFingerprint(fileBytes)（基类静态成员 → 关联函数调用形态，见 P44）
                let cf_hash = LocalResourceBase::curse_forge_fingerprint(&bytes);

                let mut info = ModInfo {
                    name: String::new(),
                    description: String::new(),
                    version: String::new(),
                    authors: Vec::new(),
                    file_path: mod_path.clone(),
                    icon: String::new(),
                    curse_forge_id: 0,
                    modrinth_id: String::new(),
                    sha1_hash: hash,
                    cf_hash,
                    modrinth_version_id: String::new(),
                    curse_forge_file_id: 0,
                };

                parse_metadata(&bytes, &mut info);

                // 源：if string.IsNullOrEmpty(modInfo.Name) → Path.GetFileNameWithoutExtension(mod)
                // （file_stem 等价：去掉最后一个扩展名）
                if info.name.is_empty() {
                    info.name = Path::new(&mod_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }

                Ok((idx, info))
            });
        }

        let mut ordered: Vec<Option<ModInfo>> = (0..mod_files.len()).map(|_| None).collect();
        let mut completed = 0i32;
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok((idx, info))) => {
                    ordered[idx] = Some(info);
                    completed += 1;
                }
                Ok(Err(e)) => return Err(e),
                // 源 Parallel.ForEach 的线程异常不向外传播（只中断该任务）→ 跳过
                Err(_) => {}
            }
            if let Some(cb) = on_progress.as_deref_mut() {
                call_progress(cb, completed, total_count);
            }
        }

        Ok(ordered.into_iter().flatten().collect())
    }

    /// 网络/解析失败静默（源 try/catch 吞错），不影响扫描结果。
    async fn enrich_from_remote(&self, mod_infos: &mut [ModInfo]) {
        if mod_infos.is_empty() {
            return;
        }

        let modrinth = ModrinthBase::new(self.http.clone(), None);
        let sha1s: Vec<String> = mod_infos
            .iter()
            .filter(|m| m.modrinth_id.is_empty() && !m.sha1_hash.is_empty())
            .map(|m| m.sha1_hash.clone())
            .collect();
        if !sha1s.is_empty() {
            match modrinth.get_project_versions_from_hashes_dict(&sha1s).await {
                Ok(map) => {
                    for info in mod_infos.iter_mut() {
                        if info.modrinth_id.is_empty() {
                            if let Some(pv) = map.get(&info.sha1_hash) {
                                info.modrinth_id = pv.project_id.clone();
                                // 文件（版本）id：ProjectVersionInfo.id（C# 未落盘，此处补全）
                                info.modrinth_version_id = pv.id.clone();
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Modrinth hash lookup failed: {e}");
                }
            }
        }

        if !self.api_key.is_empty() {
            let cf = CurseForgeBase::new(self.http.clone(), self.api_key.clone(), None);
            let hashes: Vec<i64> = mod_infos
                .iter()
                .filter(|m| m.curse_forge_id == 0 && m.cf_hash != 0)
                .map(|m| m.cf_hash)
                .collect();
            if !hashes.is_empty() {
                match cf.get_info_from_hashes_dict(&hashes).await {
                    Ok(map) => {
                        for info in mod_infos.iter_mut() {
                            if info.curse_forge_id == 0 {
                                if let Some(meta) = map.get(&info.cf_hash) {
                                    info.curse_forge_id = meta.mod_id;
                                    // 文件 id：FingerprintsFilesMeta.id（C# 未落盘，此处补全）
                                    info.curse_forge_file_id = meta.file_id as i64;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("CurseForge fingerprint lookup failed: {e}");
                    }
                }
            }
        }
    }
}

/// 进度回调调用（对应源 `onProgress?.Invoke(cur, total)`）。
///
/// ⚠️ UNMAPPED（B10 定案签名问题）：trait 签名为 `&dyn FnMut`，safe Rust 无法直接调用
/// （FnMut::call_mut 需要 `&mut` 接收者；std 的 `impl FnMut for &F` 仅限 `F: Fn`，
/// `dyn FnMut` 不满足）→ 经原始指针把胖指针转写为 `&mut dyn FnMut` 后调用
/// （两者内存布局一致）。调用方按顺序、单线程、无重入地传入回调 → 实际安全；
/// 建议后续批次将签名改为 `&mut dyn FnMut(i32, i32)` 后移除本转写。
fn call_progress(cb: &mut (dyn FnMut(i32, i32) + Send), current: i32, total: i32) {
    cb(current, total);
}

/// 解析单个 Mod 文件元数据（对应源 GetModList 中 try/catch 包裹的解析块）：
/// fabric.mod.json → META-INF/mods.toml（回退 neoforge.mods.toml）→ mcmod.info，
/// 任一环节失败静默跳过（同源 catch{} 吞错，且不尝试后续格式），
/// 名称兜底（文件名）由调用方处理。
fn parse_metadata(file_bytes: &[u8], info: &mut ModInfo) {
    let mut archive = match ZipArchive::new(Cursor::new(file_bytes)) {
        Ok(a) => a,
        Err(_) => return,
    };

    let fabric = match read_zip_entry(&mut archive, "fabric.mod.json") {
        Err(_) => return,
        Ok(c) => c,
    };
    if let Some(content) = fabric {
        parse_fabric_json(&mut archive, &content, info);
        return;
    }

    let toml_content = match read_zip_entry(&mut archive, "META-INF/mods.toml") {
        Err(_) => return,
        Ok(c) => c,
    };
    let toml_content = match toml_content {
        Some(content) => Some(content),
        None => match read_zip_entry(&mut archive, "META-INF/neoforge.mods.toml") {
            Err(_) => return,
            Ok(c) => c,
        },
    };
    if let Some(content) = toml_content {
        parse_forge_toml(&mut archive, &content, info);
        return;
    }

    let mcmod = match read_zip_entry(&mut archive, "mcmod.info") {
        Err(_) => return,
        Ok(c) => c,
    };
    if let Some(content) = mcmod {
        parse_mcmod_json(&content, info);
    }
}

/// 在 zip 中按名称查找条目索引（源 .NET `ZipArchive.GetEntry` 为
/// `OrdinalIgnoreCase` 匹配；zip crate 的 by_name 为精确匹配 → 先定位索引再 by_index，
/// 同 P44 基类 try_read_file_from_zip 的处理方式）
fn find_entry_index<R: Read + Seek>(archive: &ZipArchive<R>, entry_path: &str) -> Option<usize> {
    archive.file_names().position(|n| n.eq_ignore_ascii_case(entry_path))
}

/// 读取 zip 内条目文本（源：ReadZipEntry + StreamReader）。
/// - 无匹配条目 → `Ok(None)`（同源返回 null → 尝试下一格式）；
/// - 条目存在但读取失败 → `Err(())`（同源 StreamReader 抛异常 → 外层 catch 吞掉整个解析块）；
/// - BOM 剥离 + 非法 UTF-8 替换（源 StreamReader 默认 UTF-8、检测 BOM、替换模式）
fn read_zip_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entry_path: &str,
) -> Result<Option<String>, ()> {
    let index = match find_entry_index(archive, entry_path) {
        Some(i) => i,
        None => return Ok(None),
    };
    let mut entry = archive.by_index(index).map_err(|_| ())?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).map_err(|_| ())?;
    let mut content = String::from_utf8_lossy(&bytes).into_owned();
    if let Some(rest) = content.strip_prefix('\u{feff}') {
        content = rest.to_string();
    }
    Ok(Some(content))
}

/// 提取 zip 内图标为 base64 字符串（源：ExtractIconFromArchive）：
/// 无条目 → 空串；读取失败 → 空串（同源 Open 抛异常被外层 catch 吞掉，前序字段保留）；
/// 内容为空 → 空串；否则 → base64
fn extract_icon_from_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    icon_path: &str,
) -> String {
    let index = match find_entry_index(archive, icon_path) {
        Some(i) => i,
        None => return String::new(),
    };
    let mut entry = match archive.by_index(index) {
        Ok(e) => e,
        Err(_) => return String::new(),
    };
    let mut bytes = Vec::new();
    if entry.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    if bytes.is_empty() {
        String::new()
    } else {
        base64_encode(&bytes)
    }
}

/// 对应源 `Convert.ToBase64String(byte[])`（MAPPING_TABLE runtime 映射：base64 crate）。
/// ⚠️ 需要依赖: base64 = "1"（Cargo.toml 尚未引入；本批禁止修改 Cargo.toml → 待后续批次声明）
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 解析 fabric.mod.json（源：`JsonNode.Parse(content)!.AsObject()`）：
/// JSON 无效或非对象 → 跳过（同源异常被 catch 吞掉，不尝试后续格式）
fn parse_fabric_json<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    content: &str,
    info: &mut ModInfo,
) {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(content) else {
        return;
    };
    info.name = json_str(&obj, "name").unwrap_or_else(|| "Unknown".to_string());
    info.version = json_str(&obj, "version").unwrap_or_default();
    info.description = json_str(&obj, "description")
        .unwrap_or_else(|| "No description available".to_string());
    info.authors = extract_fabric_authors(obj.get("authors"));
    if let Some(icon_path) = json_str(&obj, "icon").filter(|p| !p.is_empty()) {
        info.icon = extract_icon_from_archive(archive, &icon_path);
    }
}

/// 提取 Fabric 作者列表（源：ExtractFabricAuthors）：
/// 非数组 → 空；数组元素：对象且含非 null "name" → name 值文本；
/// 对象无 "name" / name 为 null → 元素自身紧凑 JSON 文本（源 `a.ToString()`）；
/// 标量元素 → 自身文本；null 元素 → 空串（源 `a?.ToString() ?? ""`）
fn extract_fabric_authors(authors: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(arr)) = authors else {
        return Vec::new();
    };
    arr.iter()
        .map(|a| match a {
            Value::Object(obj) => match obj.get("name") {
                Some(Value::Null) | None => a.to_string(),
                Some(name) => json_value_text(name),
            },
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .collect()
}

/// 对应源 `json[key]?.ToString()`：键缺失或 JSON null → None；
/// 字符串 → 原样；其余值 → 紧凑 JSON 文本（同源 JsonNode.ToString 语义）
fn json_str(obj: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Null) | None => None,
        Some(v) => Some(json_value_text(v)),
    }
}

/// 对应源 `JsonNode.ToString()`：字符串原样，其余值 → 紧凑 JSON 文本
fn json_value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 解析 Forge/NeoForge mods.toml（源：Tomlyn `Toml.ToModel(content)` → `model["mods"]`
/// 表数组首元素 `(TomlTable)mods[0]`；缺失/类型不符/空数组 → 跳过，同源异常被吞）。
/// ⚠️ 需要依赖: toml crate（MAPPING_TABLE runtime 映射：Tomlyn → toml；
/// Cargo.toml 尚未引入，本批禁止修改 → 待后续批次声明）
fn parse_forge_toml<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    content: &str,
    info: &mut ModInfo,
) {
    let Ok(value) = content.parse::<toml::Value>() else {
        return;
    };
    let Some(table) = value.as_table() else {
        return;
    };
    let Some(toml::Value::Array(mods)) = table.get("mods") else {
        return;
    };
    let Some(toml::Value::Table(mod_table)) = mods.first() else {
        return;
    };

    // 源：`TryGetValue(key, out var v) ? v?.ToString() ?? default : default`
    // ⚠️ 必须从 mods[0] 表读取（源从 `(TomlTable)mods[0]` 取 displayName/description/version），
    // 顶层 table 无这些键（仅 modLoaderId/license/mods 等）
    // 非字符串 TOML 值（整数/布尔等）→ TOML 文本（源为 Tomlyn ToString，格式差异
    // 仅影响理论上的非字符串 displayName，实际 mods.toml 均为字符串，见日志）
    let toml_get = |key: &str, default: &str| -> String {
        match mod_table.get(key) {
            Some(toml::Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => default.to_string(),
        }
    };
    info.name = toml_get("displayName", "Unknown");
    info.description = toml_get("description", "");
    info.version = toml_get("version", "");

    // 源：version == "${file.jarVersion}" → 读 META-INF/MANIFEST.MF，
    // 找前缀 "Implementation-Version:"（OrdinalIgnoreCase）取截断后 Trim 的首行
    if info.version == "${file.jarVersion}" {
        let manifest = match read_zip_entry(archive, "META-INF/MANIFEST.MF") {
            Err(_) => return,
            Ok(Some(m)) => m,
            Ok(None) => String::new(),
        };
        let prefix = "Implementation-Version:";
        for line in manifest.split("\r\n").flat_map(|s| s.split('\n')) {
            if line.len() >= prefix.len()
                && line
                    .get(..prefix.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
            {
                // 前缀匹配 → 前 23 字节为 ASCII，字节边界安全（同源字符串长度切片）
                info.version = line[prefix.len()..].trim().to_string();
                break;
            }
        }
    }

    // 源：authors 键存在且为 TOML 字符串 → 按 ',' 分割并 Trim；
    // 非字符串（如表数组）→ 忽略（源 `is string` 类型检查不命中）
    if let Some(toml::Value::String(authors)) = table.get("authors") {
        info.authors = authors.split(',').map(|a| a.trim().to_string()).collect();
    }

    // 源：logoFile 为字符串且非空 → 从压缩包提取图标
    if let Some(toml::Value::String(logo)) = table.get("logoFile") {
        if !logo.is_empty() {
            info.icon = extract_icon_from_archive(archive, logo);
        }
    }
}

/// 解析 mcmod.info（源：`JsonNode.Parse(content)!.AsArray()`，`Count > 0` 取首元素对象）。
/// JSON 无效 / 非数组 / 空数组 / 首元素非对象 → 跳过（同源异常被吞或条件不成立）
fn parse_mcmod_json(content: &str, info: &mut ModInfo) {
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(content) else {
        return;
    };
    if arr.is_empty() {
        return;
    }
    let Some(Value::Object(first)) = arr.first() else {
        return;
    };

    info.name = json_str(first, "name").unwrap_or_else(|| "Unknown".to_string());
    info.description = json_str(first, "description").unwrap_or_default();
    info.version = json_str(first, "version").unwrap_or_default();

    // 源：authors 为 JsonArray → 各元素 `a!.ToString()`（元素为 null → NRE 被吞，
    // authors 保持未设置 → 仅当无 null 元素时赋值）；
    // 否则 authors 为 JsonValue（字符串/数字/布尔标量）→ 按 ',' 分割 Trim；
    // 缺失 / null / 对象 → 跳过
    if let Some(Value::Array(authors)) = first.get("authors") {
        if authors.iter().all(|a| !a.is_null()) {
            info.authors = authors.iter().map(|a| a.to_string()).collect();
        }
    } else if let Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_))) =
        first.get("authors")
    {
        info.authors = v.to_string().split(',').map(|a| a.trim().to_string()).collect();
    }
}

#[async_trait]
impl ModsManager for Mods {
    async fn get_mod_list(
        &self,
        mut on_progress: Option<&mut (dyn FnMut(i32, i32) + Send)>,
    ) -> Result<Vec<ModInfo>, Error> {
        let mut mod_infos = self.scan_local(&mut on_progress).await?;
        // 源 GetModList 尾部：CF/MR 哈希反查 + 图标兜底（B13 ⚠️ UNMAPPED，见方法文档）
        self.enrich_from_remote(&mut mod_infos).await;
        Ok(mod_infos)
    }

    /// 轻量扫描：仅本地扫描 + SHA1，跳过网络反查（联机 mods 匹配用）。
    async fn get_mod_list_light(&self) -> Result<Vec<ModInfo>, Error> {
        self.scan_local(&mut None).await
    }

    /// 网络反查补全远程 id（Modrinth SHA1 → project/version id、CurseForge 指纹 → mod/file id）。
    async fn enrich_mod_ids(&self, mod_infos: &mut [ModInfo]) {
        self.enrich_from_remote(mod_infos).await;
    }

    /// 禁用 Mod（源：DisableMod）：文件存在时重命名为 `{path}.disabled`
    /// 差异说明：源 File.Move 失败抛 IOException；本实现记录 stderr 后静默
    /// （trait 返回 ()，同 checksum.rs 的 IO 失败约定）
    fn disable_mod(&self, mod_file_path: &str) {
        if Path::new(mod_file_path).is_file() {
            if let Err(e) = std::fs::rename(mod_file_path, format!("{mod_file_path}.disabled")) {
                eprintln!("禁用 Mod 失败: {mod_file_path}: {e}");
            }
        }
    }

    /// 启用 Mod（源：EnableMod）：文件存在且后缀为 `.disabled`（OrdinalIgnoreCase）时，
    /// 重命名为去掉该后缀的路径（后缀 9 字节为 ASCII，字节边界安全）
    fn enable_mod(&self, mod_file_path: &str) {
        const SUFFIX: &str = ".disabled";
        if !Path::new(mod_file_path).is_file() {
            return;
        }
        if !mod_file_path
            .get(mod_file_path.len().saturating_sub(SUFFIX.len())..)
            .is_some_and(|s| s.eq_ignore_ascii_case(SUFFIX))
        {
            return;
        }
        let target = &mod_file_path[..mod_file_path.len() - SUFFIX.len()];
        if let Err(e) = std::fs::rename(mod_file_path, target) {
            eprintln!("启用 Mod 失败: {mod_file_path}: {e}");
        }
    }
}




