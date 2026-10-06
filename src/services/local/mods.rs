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
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::ZipArchive;

use crate::api::expansion::{CurseForgeSource, ModrinthSource};
use crate::api::local::ModsManager;
use crate::error::Error;
use crate::models::expansion::local::{ModDependencyInfo, ModInfo};
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
    /// 图标缓存目录（为空时禁用 per-jar 缓存）
    icon_cache_dir: Option<PathBuf>,
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
        icon_cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            http,
            game_directory,
            version,
            version_segmented,
            api_key,
            icon_cache_dir,
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
}

// =====================================================================
// Per-jar scan cache (avoid re-unzipping unchanged mods)
// =====================================================================

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedModMeta {
    /// 缓存结构版本。缺省 0 = 引入该字段之前的旧缓存。
    /// 仅靠 size+mtime 无法发现「jar 没变但解析逻辑升级了」——旧缓存会把新字段
    /// 一律读成空（`#[serde(default)]`），导致 issue #165 的依赖数据静默缺失。
    #[serde(default)]
    v: u32,
    size: u64,
    mtime: u64,
    sha1: String,
    cf_hash: i64,
    name: String,
    description: String,
    version: String,
    authors: Vec<String>,
    icon_sha1: Option<String>,
    /// 模组自身 mod id（issue #165）
    #[serde(default)]
    mod_id: String,
    /// 强制前置依赖（issue #165）
    #[serde(default)]
    dependencies: Vec<ModDependencyInfo>,
    /// 嵌套 jar 提供的额外 mod id（issue #165）
    #[serde(default)]
    provides_ids: Vec<String>,
}

/// 当前 per-jar 缓存结构版本（v3 起含 mcmod.info 世代的 modId/requiredMods/注解依赖）。
const MOD_META_CACHE_VERSION: u32 = 3;

fn load_cached_mod(cache_file: &Path, size: u64, mtime: u64) -> Option<CachedModMeta> {
    let bytes = std::fs::read(cache_file).ok()?;
    let meta: CachedModMeta = serde_json::from_slice(&bytes).ok()?;
    // 结构版本不符 → 视为未命中，强制重新解析（旧的 size+mtime 判断发现不了逻辑升级）
    if meta.v != MOD_META_CACHE_VERSION {
        return None;
    }
    if meta.size == size && meta.mtime == mtime {
        Some(meta)
    } else {
        None
    }
}

fn save_cached_mod(cache_file: &Path, meta: &CachedModMeta) {
    if let Some(parent) = cache_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec(meta) {
        let _ = std::fs::write(cache_file, json);
    }
}

impl Mods {
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
            let is_jar = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jar"));
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
    ///
    /// Per-jar 磁盘缓存（`icon_cache_dir` 存在时启用）：以 jar 文件路径 SHA1 为 key
    /// 缓存元数据 + 图标内容哈希，缓存命中时跳过文件读取和 zip 解析，直接从磁盘
    /// 读取图标文件，避免重复解压 Jar。
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
            let cache_dir = self.icon_cache_dir.clone();
            tasks.spawn_blocking(move || -> Result<(usize, ModInfo), Error> {
                // Stat jar for cache validation (cheap, no file content read)
                let (file_size, file_mtime_millis) = match std::fs::metadata(&mod_path) {
                    Ok(m) => {
                        let size = m.len();
                        let mtime = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        let millis = mtime
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        (size, millis)
                    }
                    Err(_) => (0, 0),
                };

                let path_hash = sha1_hex(mod_path.as_bytes());

                // Per-jar cache hit: skip file read and zip parsing entirely
                if let Some(ref dir) = cache_dir {
                    let cache_file = dir.join(format!("{path_hash}.json"));
                    if let Some(cached) = load_cached_mod(&cache_file, file_size, file_mtime_millis)
                    {
                        let icon_base64 = cached.icon_sha1.and_then(|sha1| {
                            let icon_file = dir.join("icons").join(format!("{sha1}.png"));
                            std::fs::read(&icon_file).ok().and_then(|bytes| {
                                if bytes.is_empty() {
                                    None
                                } else {
                                    Some(base64_encode(&bytes))
                                }
                            })
                        });
                        return Ok((
                            idx,
                            ModInfo {
                                name: cached.name,
                                description: cached.description,
                                version: cached.version,
                                authors: cached.authors,
                                file_path: mod_path.clone(),
                                icon: icon_base64.unwrap_or_default(),
                                curse_forge_id: 0,
                                modrinth_id: String::new(),
                                sha1_hash: cached.sha1,
                                cf_hash: cached.cf_hash,
                                modrinth_version_id: String::new(),
                                curse_forge_file_id: 0,
                                mod_id: cached.mod_id,
                                dependencies: cached.dependencies,
                                provides_ids: cached.provides_ids,
                            },
                        ));
                    }
                }

                // Cache miss: full scan
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
                    sha1_hash: hash.clone(),
                    cf_hash,
                    modrinth_version_id: String::new(),
                    curse_forge_file_id: 0,
                    mod_id: String::new(),
                    dependencies: Vec::new(),
                    provides_ids: Vec::new(),
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

                // Write per-jar cache (icon + metadata) after successful parse
                if let Some(dir) = cache_dir {
                    let icon_sha1 = if info.icon.is_empty() {
                        None
                    } else {
                        base64::decode(&info.icon).ok().and_then(|bytes| {
                            if bytes.is_empty() {
                                None
                            } else {
                                let sha1 = sha1_hex(&bytes);
                                let icon_file = dir.join("icons").join(format!("{sha1}.png"));
                                let _ = std::fs::create_dir_all(icon_file.parent().unwrap());
                                let _ = std::fs::write(&icon_file, &bytes);
                                Some(sha1)
                            }
                        })
                    };

                    let cache_file = dir.join(format!("{path_hash}.json"));
                    let meta = CachedModMeta {
                        v: MOD_META_CACHE_VERSION,
                        size: file_size,
                        mtime: file_mtime_millis,
                        sha1: hash.clone(),
                        cf_hash,
                        name: info.name.clone(),
                        description: info.description.clone(),
                        version: info.version.clone(),
                        authors: info.authors.clone(),
                        icon_sha1,
                        mod_id: info.mod_id.clone(),
                        dependencies: info.dependencies.clone(),
                        provides_ids: info.provides_ids.clone(),
                    };
                    let _ = save_cached_mod(&cache_file, &meta);
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
    /// Modrinth 与 CurseForge 反查并行执行（tokio::join!），互不阻塞。
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

        let cf_hashes: Vec<i64> = mod_infos
            .iter()
            .filter(|m| m.curse_forge_id == 0 && m.cf_hash != 0)
            .map(|m| m.cf_hash)
            .collect();

        let api_key = self.api_key.clone();
        let http = self.http.clone();

        // MR 与 CF 反查并行（源两段串行 await → tokio::join!）
        let (mr_result, cf_result) = tokio::join!(
            async {
                if sha1s.is_empty() {
                    return Ok(std::collections::HashMap::new());
                }
                modrinth.get_project_versions_from_hashes_dict(&sha1s).await
            },
            async {
                if api_key.is_empty() || cf_hashes.is_empty() {
                    return Ok(std::collections::HashMap::new());
                }
                let cf = CurseForgeBase::new(http, api_key, None);
                cf.get_info_from_hashes_dict(&cf_hashes).await
            }
        );

        match mr_result {
            Ok(map) => {
                for info in mod_infos.iter_mut() {
                    if info.modrinth_id.is_empty() {
                        if let Some(pv) = map.get(&info.sha1_hash) {
                            info.modrinth_id = pv.project_id.clone();
                            info.modrinth_version_id = pv.id.clone();
                        }
                    }
                }
            }
            Err(e) => eprintln!("Modrinth hash lookup failed: {e}"),
        }

        match cf_result {
            Ok(map) => {
                for info in mod_infos.iter_mut() {
                    if info.curse_forge_id == 0 {
                        if let Some(meta) = map.get(&info.cf_hash) {
                            info.curse_forge_id = meta.mod_id;
                            info.curse_forge_file_id = meta.file_id as i64;
                        }
                    }
                }
            }
            Err(e) => eprintln!("CurseForge fingerprint lookup failed: {e}"),
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

/// 收集嵌套（Jar-in-Jar, JiJ）jar 里声明的全部 mod id（issue #165）。
///
/// **为什么必须做**：容器 jar 只声明自身 id，子模块 id 在嵌套 jar 里。真实案例：
/// `fabric-api-0.161.0.jar` 顶层 `fabric.mod.json` 的 `id` 只有 `fabric-api`，
/// 但它在 `META-INF/jars/` 下嵌了 44 个 jar，分别提供 `fabric-lifecycle-events-v1`、
/// `fabric-resource-loader-v0` 等。若只看顶层 id，依赖这些子模块的模组（Fabric API
/// 生态里极普遍，实测 Fabulously Optimized 38 个 mod 中有 7 个）会被全部误报「缺失依赖」。
///
/// 两种真实布局（均由真实整合包样本确认）：
/// 1. **Fabric**：`META-INF/jars/*.jar`，逐个读其内部 `fabric.mod.json` 的 `id`；
/// 2. **Forge/NeoForge JarJar**：`META-INF/jarjar/metadata.json` 的 `jars[].path`
///    指向嵌套 jar（同样是读其 `fabric.mod.json` / `META-INF/mods.toml` 的 id）。
///
/// 只做**一层**嵌套：JiJ 规范本身即一层，真实样本（含 44 个嵌套 jar 的 fabric-api）
/// 未出现二级嵌套；限定深度可避免恶意/异常 jar 造成解压炸弹式递归。
/// 任何单步失败都静默跳过该项（与既有解析的 catch{} 吞错约定一致）。
fn collect_nested_ids<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<String> {
    // 先收集待处理条目的索引（不能在遍历时再借 archive 读其它条目）
    let mut nested_paths: Vec<String> = Vec::new();

    // ① Fabric：META-INF/jars/*.jar
    for name in archive.file_names() {
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("meta-inf/jars/") && lower.ends_with(".jar") {
            nested_paths.push(name.to_string());
        }
    }

    // ② Forge/NeoForge JarJar：META-INF/jarjar/metadata.json → jars[].path
    if let Ok(Some(meta)) = read_zip_entry(archive, "META-INF/jarjar/metadata.json")
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&meta)
        && let Some(Value::Array(jars)) = obj.get("jars")
    {
        for entry in jars {
            if let Some(path) = entry.get("path").and_then(|p| p.as_str())
                && !path.is_empty()
            {
                nested_paths.push(path.to_string());
            }
        }
    }

    let mut ids: Vec<String> = Vec::new();
    for path in nested_paths {
        let Some(index) = find_entry_index(archive, &path) else {
            continue;
        };
        let mut bytes = Vec::new();
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        drop(entry);

        if let Some(id) = nested_jar_id(&bytes)
            && !ids.iter().any(|x| x.eq_ignore_ascii_case(&id))
        {
            ids.push(id);
        }
    }
    ids
}

/// 从一个嵌套 jar 的字节里读出它的 mod id（fabric.mod.json `id`，回退 mods.toml `modId`）。
fn nested_jar_id(bytes: &[u8]) -> Option<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    if let Ok(Some(content)) = read_zip_entry(&mut archive, "fabric.mod.json")
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&content)
        && let Some(id) = json_str(&obj, "id").filter(|s| !s.trim().is_empty())
    {
        return Some(id);
    }
    let toml_content = read_zip_entry(&mut archive, "META-INF/mods.toml")
        .ok()
        .flatten()
        .or_else(|| {
            read_zip_entry(&mut archive, "META-INF/neoforge.mods.toml")
                .ok()
                .flatten()
        })?;
    let value: toml::Value = toml_content.parse().ok()?;
    let mods = value.as_table()?.get("mods")?.as_array()?;
    let first = mods.first()?.as_table()?;
    match first.get("modId") {
        Some(toml::Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
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

    // issue #165：先收集嵌套（Jar-in-Jar）模组 id——容器 jar 只声明自己的 id，
    // 真正的子模块 id 在嵌套 jar 里（见 collect_nested_ids 文档）。
    info.provides_ids = collect_nested_ids(&mut archive);

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
        parse_mcmod_json(&mut archive, &content, info);
    }
}

/// 在 zip 中按名称查找条目索引（源 .NET `ZipArchive.GetEntry` 为
/// `OrdinalIgnoreCase` 匹配；zip crate 的 by_name 为精确匹配 → 先定位索引再 by_index，
/// 同 P44 基类 try_read_file_from_zip 的处理方式）
fn find_entry_index<R: Read + Seek>(archive: &ZipArchive<R>, entry_path: &str) -> Option<usize> {
    archive
        .file_names()
        .position(|n| n.eq_ignore_ascii_case(entry_path))
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
/// issue #165 扩展：额外读取 `id`（模组自身 id）与 `depends`（强制前置依赖）。
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
    info.description =
        json_str(&obj, "description").unwrap_or_else(|| "No description available".to_string());
    info.authors = extract_fabric_authors(obj.get("authors"));
    if let Some(icon_path) = json_str(&obj, "icon").filter(|p| !p.is_empty()) {
        info.icon = extract_icon_from_archive(archive, &icon_path);
    }
    info.mod_id = json_str(&obj, "id").unwrap_or_default();
    info.dependencies = extract_fabric_depends(obj.get("depends"));
}

/// 提取 Fabric 强制依赖（issue #165）：`depends` 为 `{ "<modid>": <版本谓词> }` 对象。
/// - 非对象（缺失 / null / 数组 / 标量）→ 空列表；
/// - 值为字符串 → 原样（如 `">=1.20"`、`"*"`）；`"*"` 视为「无版本约束」→ 空串；
/// - 值为数组 → 逐元素文本以 ` || ` 连接（Fabric 语义为「任一满足即可」）；
/// - 值为其它（数字/布尔/null）→ 紧凑 JSON 文本（与 `json_value_text` 一致）。
///
/// 仅收 `depends`：`recommends` / `suggests` / `breaks` / `conflicts` 不是启动阻塞项，
/// 收录会产生误报（缺失的 suggests 不影响游戏启动）。
fn extract_fabric_depends(depends: Option<&Value>) -> Vec<ModDependencyInfo> {
    let Some(Value::Object(map)) = depends else {
        return Vec::new();
    };
    map.iter()
        .filter(|(id, _)| !id.trim().is_empty())
        .map(|(id, v)| {
            let version_range = match v {
                Value::String(s) => {
                    if s.trim() == "*" {
                        String::new()
                    } else {
                        s.clone()
                    }
                }
                Value::Array(arr) => arr
                    .iter()
                    .map(|a| match a {
                        Value::Null => String::new(),
                        other => json_value_text(other),
                    })
                    .collect::<Vec<_>>()
                    .join(" || "),
                Value::Null => String::new(),
                other => json_value_text(other),
            };
            ModDependencyInfo {
                mod_id: id.clone(),
                version_range,
            }
        })
        .collect()
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

    // issue #165：Forge/NeoForge 的依赖声明位于**顶层** `[[dependencies.<自身modid>]]`
    // 表数组（不在 mods[0] 里）。modId 取自 mods[0].modId（缺失时回退依赖表键本身）。
    let self_id = toml_get("modId", "");
    if !self_id.is_empty() {
        info.mod_id = self_id.clone();
    }
    let dep_key = if self_id.is_empty() {
        // 无 modId 时无法定位依赖表归属 → 不猜，直接放弃（宁可漏报也不误报）
        None
    } else {
        Some(self_id)
    };
    if let Some(key) = dep_key {
        info.dependencies = extract_forge_dependencies(table.get("dependencies"), &key);
    }
}

/// 提取 Forge/NeoForge 强制依赖（issue #165）：
/// TOML 里 `[[dependencies.<自身modid>]]` 展开为 `dependencies` 表 → `<自身modid>` 键
/// → **表数组**（无额外中间层），每项含 `modId` / `mandatory` / `versionRange` /
/// `type`（NeoForge 用 `type = "required" | "optional" | ...`）。
/// 收录口径（只收启动阻塞项，避免误报）：
/// - Forge：`mandatory` 非 false 才算（缺省为 true，与 Forge 语义一致）；
/// - NeoForge：若存在 `type` 字符串，则仅 `"required"` 收录；`"optional"` /
///   `"required_but_not_integrated"` 等一律排除；
/// - `modId` 缺失/空 → 跳过该项；`versionRange` 缺失或 `*` → 空串。
///
/// 内置依赖（`minecraft` / `forge` / `neoforge` / `fabricloader` 等平台自身条目）由
/// **前端**统一过滤——它们在 Forge 元数据里普遍声明为 mandatory，但恒被加载器满足。
fn extract_forge_dependencies(
    dependencies: Option<&toml::Value>,
    self_id: &str,
) -> Vec<ModDependencyInfo> {
    let Some(toml::Value::Table(dep_table)) = dependencies else {
        return Vec::new();
    };
    let Some(toml::Value::Array(entries)) = dep_table.get(self_id) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let toml::Value::Table(t) = entry else {
                return None;
            };
            let mod_id = match t.get("modId") {
                Some(toml::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
                _ => return None,
            };
            // mandatory 缺省 true；显式 false → 非启动阻塞项
            if let Some(false) = t.get("mandatory").and_then(|v| v.as_bool()) {
                return None;
            }
            // NeoForge 的 type 优先级高于 mandatory
            if let Some(toml::Value::String(ty)) = t.get("type")
                && !ty.eq_ignore_ascii_case("required")
            {
                return None;
            }
            let version_range = match t.get("versionRange") {
                Some(toml::Value::String(s)) if s.trim() != "*" => s.clone(),
                _ => String::new(),
            };
            Some(ModDependencyInfo {
                mod_id,
                version_range,
            })
        })
        .collect()
}

/// 解析 mcmod.info（源：`JsonNode.Parse(content)!.AsArray()`，`Count > 0` 取首元素对象）。
/// JSON 无效 / 非数组 / 空数组 / 首元素非对象 → 元数据字段跳过（同源异常被吞或条件不成立）。
///
/// issue #165 后续（1.12.2 世代补全）：除源有的展示字段外，还补读
/// `modid`、`useDependencyInformation=true` 时的 `requiredMods`（缺失即崩溃的硬依赖），
/// 并扫描 @Mod 字节码注解（`required-after:` / `required-before:`）——Forge 运行时
/// 强制检查的权威来源。`mcmod.info` 的 `dependencies` 列表刻意**不读**：Forge 官方
/// 语义是纯加载顺序、缺失不影响启动，收进来会把可选软依赖误报成缺失。
fn parse_mcmod_json<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    content: &str,
    info: &mut ModInfo,
) {
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
    if let Some(id) = json_str(first, "modid").filter(|s| !s.trim().is_empty()) {
        info.mod_id = id.trim().to_string();
    }

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
        info.authors = v
            .to_string()
            .split(',')
            .map(|a| a.trim().to_string())
            .collect();
    }

    // 硬依赖 = 注解（权威）∪ requiredMods（useDependencyInformation=true 才生效；
    // FML 完整条件另需 @Mod(useMetadata=true)，见 extract_legacy_required_mods 的
    // 近似说明）。以注解为主：注解恒在，mcmod.info 依赖字段只在 useMetadata 时被采用。
    let mut deps = scan_legacy_forge_annotation_deps(archive);
    if first
        .get("useDependencyInformation")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        for dep in extract_legacy_required_mods(first.get("requiredMods")) {
            if !deps
                .iter()
                .any(|d| d.mod_id.eq_ignore_ascii_case(&dep.mod_id))
            {
                deps.push(dep);
            }
        }
    }
    info.dependencies = deps;
}

/// 扫描 jar 内 class 文件常量池中的 Forge @Mod 注解依赖声明
/// （`required-after:<modid>[@<range>]` / `required-before:<modid>[@<range>]`）。
/// 1.12.2 世代注解未经 LoaderMigration 处理时以该字符串形式留在常量池里，
/// 是 FML 实际执行强制检查的权威数据源；只匹配带 `required-` 前缀的项，
/// `after:` / `before:` 仅是加载顺序、缺失不崩溃，不收录（避免误报）。
/// 任意单步失败静默跳过（与既有解析的 catch{} 吞错约定一致）。
fn scan_legacy_forge_annotation_deps<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
) -> Vec<ModDependencyInfo> {
    const MARKERS: [&str; 2] = ["required-after:", "required-before:"];

    let mut deps: Vec<ModDependencyInfo> = Vec::new();
    // 先收集条目索引（不能在遍历时再借 archive 读其它条目，同 collect_nested_ids）
    let class_indices: Vec<usize> = archive
        .file_names()
        .filter(|n| n.to_ascii_lowercase().ends_with(".class"))
        .map(|n| find_entry_index(archive, n))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    for index in class_indices {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        drop(entry);
        // class 常量池字符串是修改版 UTF-8；依赖声明（modid/range）为 ASCII，
        // lossy 解码足够，也天然容错非 UTF-8 字节
        let text = String::from_utf8_lossy(&bytes);
        for marker in MARKERS {
            let mut from = 0;
            while let Some(pos) = text[from..].find(marker) {
                let start = from + pos + marker.len();
                let tail = &text[start..];
                let end = tail
                    .find(|c: char| c == ';' || c == '\u{0}')
                    .unwrap_or(tail.len());
                if let Some(dep) = parse_legacy_dep_spec(tail[..end].trim()) {
                    if !deps
                        .iter()
                        .any(|d| d.mod_id.eq_ignore_ascii_case(&dep.mod_id))
                    {
                        deps.push(dep);
                    }
                }
                from = start + end;
                if from >= text.len() {
                    break;
                }
            }
        }
    }
    deps
}

/// 解析 `modid` / `modid@<version-range>`（Forge 注解依赖声明格式；
/// 空声明 / 空白 modid → None）。`*` 视为「无版本约束」→ 空串。
fn parse_legacy_dep_spec(spec: &str) -> Option<ModDependencyInfo> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let (mod_id, version_range) = match spec.split_once('@') {
        Some((id, range)) => (id, range),
        None => (spec, ""),
    };
    let mod_id = mod_id.trim();
    if mod_id.is_empty() {
        return None;
    }
    let version_range = match version_range.trim() {
        "*" | "" => String::new(),
        r => r.to_string(),
    };
    Some(ModDependencyInfo {
        mod_id: mod_id.to_string(),
        version_range,
    })
}

/// 提取 mcmod.info `requiredMods`（1.12.2 硬依赖：缺失即崩溃）。
/// FML 完整采用条件：`useDependencyInformation=true` **且**该 mod 声明
/// `@Mod(useMetadata=true)`（Forge 官方文档 structuring：requiredMods 缺失会崩溃，
/// dependencies 只影响加载顺序；两开关缺省均为 false）。useMetadata 是注解属性、
/// 缺省时不在常量池留字符串，无法可靠判定 → 以 `useDependencyInformation=true`
/// 近似（CodeRabbit PR #219 评审指出，父仓 ADR-101 v1.4 同步记录）：极端「声明了
/// requiredMods 却未开 useMetadata」的 jar 可能误报，为守住硬依赖告警覆盖接受
/// （漏报 = 崩溃无提示，即 issue #165 原始痛点）；`modid@[range]` / `modid`
/// 混合格式逐项解析，无效项跳过。
fn extract_legacy_required_mods(required_mods: Option<&Value>) -> Vec<ModDependencyInfo> {
    let Some(Value::Array(arr)) = required_mods else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| v.as_str())
        .filter_map(parse_legacy_dep_spec)
        .collect()
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

/// issue #165 依赖解析单元测试。这些函数是模块私有（`fn parse_*`），
/// 只能内联测——外部 `tests/` 集成测试无法触及。
#[cfg(test)]
mod dependency_tests {
    use super::*;

    fn empty_info() -> ModInfo {
        ModInfo {
            name: String::new(),
            description: String::new(),
            version: String::new(),
            authors: Vec::new(),
            file_path: String::new(),
            icon: String::new(),
            curse_forge_id: 0,
            modrinth_id: String::new(),
            sha1_hash: String::new(),
            cf_hash: 0,
            modrinth_version_id: String::new(),
            curse_forge_file_id: 0,
            mod_id: String::new(),
            dependencies: Vec::new(),
            provides_ids: Vec::new(),
        }
    }

    // ── fabric ────────────────────────────────────────────────

    #[test]
    fn fabric_parses_mod_id_and_depends() {
        let json = r#"{
            "id": "create",
            "name": "Create",
            "version": "0.5.1f",
            "depends": {
                "minecraft": ">=1.20.1",
                "fabricloader": ">=0.14.21",
                "flywheel": ">=0.6.10"
            }
        }"#;
        let mut info = empty_info();
        parse_fabric_json(&mut dummy_archive(), json, &mut info);

        assert_eq!(info.mod_id, "create");
        let deps: Vec<(&str, &str)> = info
            .dependencies
            .iter()
            .map(|d| (d.mod_id.as_str(), d.version_range.as_str()))
            .collect();
        assert!(deps.contains(&("minecraft", ">=1.20.1")));
        assert!(deps.contains(&("fabricloader", ">=0.14.21")));
        assert!(deps.contains(&("flywheel", ">=0.6.10")));
        assert_eq!(deps.len(), 3);
    }

    #[test]
    fn fabric_wildcard_version_becomes_empty_range() {
        let json = r#"{"id":"a","depends":{"other":"*"}}"#;
        let mut info = empty_info();
        parse_fabric_json(&mut dummy_archive(), json, &mut info);
        assert_eq!(info.dependencies.len(), 1);
        assert_eq!(info.dependencies[0].mod_id, "other");
        assert_eq!(info.dependencies[0].version_range, "");
    }

    #[test]
    fn fabric_array_version_joins_with_or() {
        let json = r#"{"id":"a","depends":{"other":[">=1.0","<2.0"]}}"#;
        let mut info = empty_info();
        parse_fabric_json(&mut dummy_archive(), json, &mut info);
        assert_eq!(info.dependencies[0].version_range, ">=1.0 || <2.0");
    }

    #[test]
    fn fabric_ignores_non_blocking_relations() {
        // recommends / suggests / breaks / conflicts 不是启动阻塞项 → 不收录
        let json = r#"{
            "id": "a",
            "depends": {"required_mod": "*"},
            "recommends": {"nice_to_have": "*"},
            "suggests": {"maybe": "*"},
            "breaks": {"incompatible": "*"},
            "conflicts": {"also_bad": "*"}
        }"#;
        let mut info = empty_info();
        parse_fabric_json(&mut dummy_archive(), json, &mut info);
        assert_eq!(info.dependencies.len(), 1);
        assert_eq!(info.dependencies[0].mod_id, "required_mod");
    }

    #[test]
    fn fabric_missing_depends_is_empty() {
        let json = r#"{"id":"a","name":"A"}"#;
        let mut info = empty_info();
        parse_fabric_json(&mut dummy_archive(), json, &mut info);
        assert!(info.dependencies.is_empty());
        assert_eq!(info.mod_id, "a");
    }

    #[test]
    fn fabric_invalid_json_leaves_fields_untouched() {
        let mut info = empty_info();
        parse_fabric_json(&mut dummy_archive(), "not json", &mut info);
        assert!(info.mod_id.is_empty());
        assert!(info.dependencies.is_empty());
    }

    // ── forge / neoforge ──────────────────────────────────────

    #[test]
    fn forge_parses_mandatory_dependencies() {
        let toml = r#"
modLoader="javafml"
[[mods]]
modId="mekanism"
displayName="Mekanism"
version="10.4.5"
[[dependencies.mekanism]]
    modId="minecraft"
    mandatory=true
    versionRange="[1.20.1,1.21)"
[[dependencies.mekanism]]
    modId="forge"
    mandatory=true
    versionRange="[47,)"
[[dependencies.mekanism]]
    modId="optionalmod"
    mandatory=false
    versionRange="[1.0,)"
"#;
        let mut info = empty_info();
        parse_forge_toml(&mut dummy_archive(), toml, &mut info);

        assert_eq!(info.mod_id, "mekanism");
        let deps: Vec<(&str, &str)> = info
            .dependencies
            .iter()
            .map(|d| (d.mod_id.as_str(), d.version_range.as_str()))
            .collect();
        assert_eq!(deps.len(), 2, "mandatory=false 不应收录: {deps:?}");
        assert!(deps.contains(&("minecraft", "[1.20.1,1.21)")));
        assert!(deps.contains(&("forge", "[47,)")));
    }

    #[test]
    fn forge_mandatory_defaults_true_when_absent() {
        let toml = r#"
[[mods]]
modId="m"
[[dependencies.m]]
    modId="needed"
    versionRange="[1,)"
"#;
        let mut info = empty_info();
        parse_forge_toml(&mut dummy_archive(), toml, &mut info);
        assert_eq!(info.dependencies.len(), 1);
        assert_eq!(info.dependencies[0].mod_id, "needed");
    }

    #[test]
    fn neoforge_type_optional_excluded() {
        let toml = r#"
[[mods]]
modId="m"
[[dependencies.m]]
    modId="req"
    type="required"
    versionRange="[1,)"
[[dependencies.m]]
    modId="opt"
    type="optional"
    versionRange="[1,)"
[[dependencies.m]]
    modId="notintegrated"
    type="required_but_not_integrated"
    versionRange="[1,)"
"#;
        let mut info = empty_info();
        parse_forge_toml(&mut dummy_archive(), toml, &mut info);
        let ids: Vec<&str> = info
            .dependencies
            .iter()
            .map(|d| d.mod_id.as_str())
            .collect();
        assert_eq!(ids, vec!["req"], "只有 type=required 收录，实际: {ids:?}");
    }

    #[test]
    fn forge_deps_ignored_when_mod_id_absent() {
        // 无 mods[0].modId 时无法定位依赖表归属 → 不猜（宁可漏报不误报）
        let toml = r#"
[[mods]]
displayName="Mystery"
[[dependencies.something]]
    modId="x"
    mandatory=true
"#;
        let mut info = empty_info();
        parse_forge_toml(&mut dummy_archive(), toml, &mut info);
        assert!(info.mod_id.is_empty());
        assert!(info.dependencies.is_empty());
    }

    #[test]
    fn forge_deps_not_read_from_other_mod_id_table() {
        // 依赖表键与自身 modId 不符 → 不取（避免把别人的依赖表算到自己头上）
        let toml = r#"
[[mods]]
modId="m"
[[dependencies.other]]
    modId="x"
    mandatory=true
"#;
        let mut info = empty_info();
        parse_forge_toml(&mut dummy_archive(), toml, &mut info);
        assert_eq!(info.mod_id, "m");
        assert!(info.dependencies.is_empty());
    }

    #[test]
    fn forge_wildcard_range_becomes_empty() {
        let toml = r#"
[[mods]]
modId="m"
[[dependencies.m]]
    modId="x"
    mandatory=true
    versionRange="*"
"#;
        let mut info = empty_info();
        parse_forge_toml(&mut dummy_archive(), toml, &mut info);
        assert_eq!(info.dependencies[0].version_range, "");
    }

    // ── 缓存 round-trip（新字段必须能持久化并读回） ──────────────

    #[test]
    fn cached_meta_roundtrips_dependency_fields() {
        let meta = CachedModMeta {
            v: MOD_META_CACHE_VERSION,
            size: 10,
            mtime: 20,
            sha1: "abc".to_string(),
            cf_hash: 1,
            name: "N".to_string(),
            description: "D".to_string(),
            version: "V".to_string(),
            authors: vec!["A".to_string()],
            icon_sha1: None,
            mod_id: "self".to_string(),
            dependencies: vec![ModDependencyInfo {
                mod_id: "dep".to_string(),
                version_range: ">=1".to_string(),
            }],
            provides_ids: vec!["nested-sub".to_string()],
        };
        let json = serde_json::to_vec(&meta).unwrap();
        let back: CachedModMeta = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.mod_id, "self");
        assert_eq!(back.dependencies.len(), 1);
        assert_eq!(back.dependencies[0].mod_id, "dep");
        assert_eq!(back.provides_ids, vec!["nested-sub".to_string()]);
    }

    #[test]
    fn cached_meta_accepts_legacy_json_without_new_fields() {
        // 升级前写的缓存文件没有新字段 → 必须仍能反序列化（serde default），
        // 否则每个 jar 的缓存都会失效并回落到全量重扫。
        let legacy = r#"{"size":1,"mtime":2,"sha1":"s","cfHash":3,"name":"n",
            "description":"d","version":"v","authors":[],"iconSha1":null}"#;
        let meta: CachedModMeta = serde_json::from_str(legacy).unwrap();
        assert!(meta.mod_id.is_empty());
        assert!(meta.dependencies.is_empty());
        assert!(meta.provides_ids.is_empty());
    }

    #[test]
    fn legacy_cache_version_forces_rescan() {
        // 核心回归：旧缓存（无 v 字段 / v 不匹配）即使 size+mtime 完全一致，
        // 也必须判为未命中。否则升级后 jar 未变 → 复用旧缓存 → 新的依赖字段
        // 被 serde default 读成空 → issue #165 功能静默失效（真实踩过）。
        let dir = std::env::temp_dir().join(format!("qomicex-cachever-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("m.json");

        // v=0（缺省，等价于老缓存）
        let old = r#"{"size":100,"mtime":200,"sha1":"s","cfHash":0,"name":"n",
            "description":"d","version":"v","authors":[],"iconSha1":null,
            "modId":"m","dependencies":[]}"#;
        std::fs::write(&f, old).unwrap();
        assert!(
            load_cached_mod(&f, 100, 200).is_none(),
            "旧版本缓存必须判为未命中（否则新字段读不到）"
        );

        // v 匹配 → 命中
        let ok = format!(
            r#"{{"v":{MOD_META_CACHE_VERSION},"size":100,"mtime":200,"sha1":"s","cfHash":0,
                "name":"n","description":"d","version":"v","authors":[],"iconSha1":null,
                "modId":"m","dependencies":[],"providesIds":["sub"]}}"#
        );
        std::fs::write(&f, ok).unwrap();
        let hit = load_cached_mod(&f, 100, 200).expect("版本一致应命中");
        assert_eq!(hit.provides_ids, vec!["sub".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 这些解析函数只在「图标路径为空」时才需要读 zip；测试数据均不带 icon 字段，
    /// 因此传一个空归档即可（任何读取都会因找不到条目而返回空串）。
    fn dummy_archive() -> ZipArchive<Cursor<Vec<u8>>> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            w.start_file("placeholder.txt", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        ZipArchive::new(Cursor::new(buf)).unwrap()
    }

    // ── 端到端：真实 jar → 完整扫描管线（scan_local）────────────────
    // 上面的测试直接调 `parse_*`，只证明解析函数本身对。这里构造**真实 zip**，
    // 走 `Mods::new(...).get_mod_list_light()`，证明数据确实能从磁盘上的 jar
    // 一路流到 `Vec<ModInfo>`（即 backend metadata 端点的数据来源）。

    /// 把 (条目名, 内容) 写成真实 zip 文件。
    fn write_jar(path: &std::path::Path, entries: &[(&str, &str)]) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    /// 创建临时 gameDir（含 mods/ 子目录），返回 (gameDir, modsDir)。
    fn temp_game_dir(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "qomicex-dep-e2e-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mods_dir = dir.join("mods");
        std::fs::create_dir_all(&mods_dir).unwrap();
        (dir, mods_dir)
    }

    /// 用给定 gameDir 跑一次真实扫描（version_segmented=false → 读 {gameDir}/mods）。
    fn scan(game_dir: &std::path::Path) -> Vec<ModInfo> {
        let mods = Mods::new(
            reqwest::Client::new(),
            game_dir.to_string_lossy().into_owned(),
            "1.20.1".to_string(),
            false,
            String::new(),
            None,
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(mods.get_mod_list_light())
            .unwrap()
    }

    fn find<'a>(list: &'a [ModInfo], file_name: &str) -> &'a ModInfo {
        list.iter()
            .find(|m| {
                Path::new(&m.file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    == Some(file_name.to_string())
            })
            .unwrap_or_else(|| {
                panic!(
                    "未找到 {file_name}: {:?}",
                    list.iter().map(|m| &m.file_path).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn e2e_fabric_jar_exposes_mod_id_and_dependencies() {
        let (game_dir, mods_dir) = temp_game_dir("fabric");
        write_jar(
            &mods_dir.join("create-0.5.1f.jar"),
            &[(
                "fabric.mod.json",
                r#"{"id":"create","name":"Create","version":"0.5.1f",
                    "depends":{"minecraft":">=1.20.1","flywheel":">=0.6.10","missinglib":"*"}}"#,
            )],
        );
        write_jar(
            &mods_dir.join("flywheel-0.6.10.jar"),
            &[(
                "fabric.mod.json",
                r#"{"id":"flywheel","name":"Flywheel","version":"0.6.10"}"#,
            )],
        );

        let list = scan(&game_dir);
        let create = find(&list, "create-0.5.1f.jar");
        assert_eq!(create.mod_id, "create", "fabric id 必须落到 mod_id");
        let ids: Vec<&str> = create
            .dependencies
            .iter()
            .map(|d| d.mod_id.as_str())
            .collect();
        assert!(ids.contains(&"flywheel"), "deps: {ids:?}");
        assert!(ids.contains(&"missinglib"), "deps: {ids:?}");
        assert!(ids.contains(&"minecraft"), "deps: {ids:?}");
        assert_eq!(
            create
                .dependencies
                .iter()
                .find(|d| d.mod_id == "missinglib")
                .unwrap()
                .version_range,
            "",
            "'*' 应归一化为空串"
        );
        assert_eq!(
            create
                .dependencies
                .iter()
                .find(|d| d.mod_id == "flywheel")
                .unwrap()
                .version_range,
            ">=0.6.10"
        );
        // 前置自身也带 id → 前端判定才能匹配上
        assert_eq!(find(&list, "flywheel-0.6.10.jar").mod_id, "flywheel");
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    // ── 嵌套 Jar-in-Jar（真实数据暴露的误报来源）─────────────────────
    // 实测：Fabulously Optimized（Fabric，38 mod）中 `fabric-api` 顶层 id 只有
    // `fabric-api`，但其 META-INF/jars/ 下嵌了 44 个 jar，提供
    // `fabric-lifecycle-events-v1` 等子模块 id；只读顶层 id 会让 7 个 mod 误报缺失。

    /// 构造一个嵌套 jar 的字节（内含 fabric.mod.json）。
    fn nested_jar_bytes(id: &str) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            w.start_file("fabric.mod.json", opts).unwrap();
            w.write_all(format!(r#"{{"id":"{id}","name":"{id}"}}"#).as_bytes())
                .unwrap();
            w.finish().unwrap();
        }
        buf
    }

    /// 把嵌套 jar 作为一个条目写进外层容器 jar。
    fn write_container_jar(
        path: &std::path::Path,
        top_id: &str,
        nested: &[(&str, Vec<u8>)],
        extra_entries: &[(&str, &str)],
    ) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        zip.start_file("fabric.mod.json", opts).unwrap();
        zip.write_all(format!(r#"{{"id":"{top_id}","name":"{top_id}"}}"#).as_bytes())
            .unwrap();
        for (name, bytes) in nested {
            zip.start_file(name.to_string(), opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        for (name, content) in extra_entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn e2e_fabric_nested_jars_expose_submodule_ids() {
        let (game_dir, mods_dir) = temp_game_dir("jij-fabric");
        // 容器 jar：模拟 fabric-api（顶层 id 是 fabric-api，子模块 id 在内层）
        write_container_jar(
            &mods_dir.join("fabric-api-0.161.0.jar"),
            "fabric-api",
            &[
                (
                    "META-INF/jars/fabric-lifecycle-events-v1-4.1.9.jar",
                    nested_jar_bytes("fabric-lifecycle-events-v1"),
                ),
                (
                    "META-INF/jars/fabric-resource-loader-v0-3.3.26.jar",
                    nested_jar_bytes("fabric-resource-loader-v0"),
                ),
            ],
            &[],
        );
        // 依赖子模块的使用者：修复前会被误报缺失
        write_jar(
            &mods_dir.join("modmenu-21.0.0.jar"),
            &[(
                "fabric.mod.json",
                r#"{"id":"modmenu","name":"Mod Menu",
                    "depends":{"fabric-lifecycle-events-v1":"*","fabric-resource-loader-v0":"*"}}"#,
            )],
        );

        let list = scan(&game_dir);
        let api = find(&list, "fabric-api-0.161.0.jar");
        assert_eq!(api.mod_id, "fabric-api");
        let provided: Vec<&str> = api.provides_ids.iter().map(|s| s.as_str()).collect();
        assert!(
            provided.contains(&"fabric-lifecycle-events-v1"),
            "嵌套子模块 id 必须被收集: {provided:?}"
        );
        assert!(
            provided.contains(&"fabric-resource-loader-v0"),
            "{provided:?}"
        );
        // 自身 id 不应混进 provides_ids（避免重复计入）
        assert!(!provided.contains(&"fabric-api"), "{provided:?}");
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn e2e_forge_jarjar_nested_jars_expose_submodule_ids() {
        let (game_dir, mods_dir) = temp_game_dir("jij-forge");
        // 模拟 Forge JarJar：metadata.json 指定嵌套 jar 路径
        write_container_jar(
            &mods_dir.join("create-1.20.1.jar"),
            "create",
            &[(
                "META-INF/jarjar/flywheel-forge-0.6.11.jar",
                nested_jar_bytes("flywheel"),
            )],
            &[(
                "META-INF/jarjar/metadata.json",
                r#"{"jars":[{"identifier":{"group":"g","artifact":"a"},
                    "version":{"range":"[0.6.11,)","artifactVersion":"0.6.11-13"},
                    "path":"META-INF/jarjar/flywheel-forge-0.6.11.jar",
                    "isObfuscated":false}]}"#,
            )],
        );

        let list = scan(&game_dir);
        let create = find(&list, "create-1.20.1.jar");
        assert_eq!(create.mod_id, "create");
        assert_eq!(
            create.provides_ids,
            vec!["flywheel".to_string()],
            "JarJar metadata.json 指向的嵌套 jar id 必须被收集"
        );
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn e2e_jar_without_nested_jars_has_empty_provides_ids() {
        let (game_dir, mods_dir) = temp_game_dir("no-jij");
        write_jar(
            &mods_dir.join("plain-1.0.jar"),
            &[("fabric.mod.json", r#"{"id":"plain","name":"Plain"}"#)],
        );
        let list = scan(&game_dir);
        assert!(list[0].provides_ids.is_empty(), "无嵌套 jar → 空列表");
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn e2e_corrupt_nested_jar_is_skipped_not_fatal() {
        // 异常：嵌套 jar 数据损坏 → 跳过该项，不影响外层解析
        let (game_dir, mods_dir) = temp_game_dir("jij-corrupt");
        write_container_jar(
            &mods_dir.join("broken-1.0.jar"),
            "broken",
            &[
                ("META-INF/jars/corrupt.jar", b"not a zip at all".to_vec()),
                ("META-INF/jars/good.jar", nested_jar_bytes("good-submodule")),
            ],
            &[],
        );
        let list = scan(&game_dir);
        let broken = find(&list, "broken-1.0.jar");
        assert_eq!(broken.mod_id, "broken", "损坏的嵌套 jar 不应影响外层 id");
        assert_eq!(
            broken.provides_ids,
            vec!["good-submodule".to_string()],
            "损坏项被跳过，其余正常项仍要收集"
        );
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn e2e_forge_jar_exposes_mandatory_dependencies_only() {
        let (game_dir, mods_dir) = temp_game_dir("forge");
        write_jar(
            &mods_dir.join("mekanism-10.4.5.jar"),
            &[(
                "META-INF/mods.toml",
                r#"
modLoader="javafml"
[[mods]]
modId="mekanism"
displayName="Mekanism"
version="10.4.5"
[[dependencies.mekanism]]
    modId="minecraft"
    mandatory=true
    versionRange="[1.20.1,1.21)"
[[dependencies.mekanism]]
    modId="forge"
    mandatory=true
    versionRange="[47,)"
[[dependencies.mekanism]]
    modId="optional_extra"
    mandatory=false
    versionRange="[1.0,)"
"#,
            )],
        );

        let list = scan(&game_dir);
        let mek = find(&list, "mekanism-10.4.5.jar");
        assert_eq!(mek.mod_id, "mekanism");
        let ids: Vec<&str> = mek.dependencies.iter().map(|d| d.mod_id.as_str()).collect();
        assert!(
            ids.contains(&"minecraft") && ids.contains(&"forge"),
            "mandatory 依赖应解析: {ids:?}"
        );
        assert!(
            !ids.contains(&"optional_extra"),
            "mandatory=false 不应收录: {ids:?}"
        );
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn e2e_disabled_jar_is_still_scanned_with_dependency_data() {
        // 边界：`.disabled` 文件同样要解析出 id/依赖——前端「严格口径」靠
        // active=false 把被禁用的前置排除出「已提供」集合，数据必须齐全。
        let (game_dir, mods_dir) = temp_game_dir("disabled");
        write_jar(
            &mods_dir.join("disabledlib-1.0.jar.disabled"),
            &[(
                "fabric.mod.json",
                r#"{"id":"disabledlib","name":"DisabledLib","version":"1.0"}"#,
            )],
        );

        let list = scan(&game_dir);
        assert_eq!(list.len(), 1, "禁用文件也应被扫描");
        assert_eq!(list[0].mod_id, "disabledlib", "禁用文件仍需解析 mod_id");
        assert!(!list[0].is_active(), "应以 .disabled 后缀判定为非激活");
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    // ── mcmod.info（1.12.2 世代：modid + requiredMods + 注解依赖）──────────

    #[test]
    fn legacy_mcmod_reads_mod_id() {
        let json = r#"[{"modid":"jeresources","name":"Just Enough Resources",
            "version":"0.9.3","dependencies":["JEI"]}]"#;
        let mut info = empty_info();
        parse_mcmod_json(&mut dummy_archive(), json, &mut info);
        assert_eq!(info.mod_id, "jeresources");
        // mcmod.info 的 dependencies 是加载顺序软依赖 → 不收录
        assert!(info.dependencies.is_empty(), "{:?}", info.dependencies);
    }

    #[test]
    fn legacy_mcmod_required_mods_only_when_use_dependency_information() {
        // useDependencyInformation 缺省 false → requiredMods 被 FML 忽略 → 不收录
        let json = r#"[{"modid":"m","name":"M","requiredMods":["a@[1.0,)"]}]"#;
        let mut info = empty_info();
        parse_mcmod_json(&mut dummy_archive(), json, &mut info);
        assert!(info.dependencies.is_empty(), "{:?}", info.dependencies);

        // 显式 true → requiredMods 是硬依赖（缺失即崩溃）
        let json = r#"[{"modid":"m","name":"M","useDependencyInformation":true,
            "requiredMods":["a@[1.0,)"]}]"#;
        let mut info = empty_info();
        parse_mcmod_json(&mut dummy_archive(), json, &mut info);
        assert_eq!(info.dependencies.len(), 1);
        assert_eq!(info.dependencies[0].mod_id, "a");
        assert_eq!(info.dependencies[0].version_range, "[1.0,)");
    }

    #[test]
    fn legacy_annotation_deps_deduped_against_required_mods() {
        // 同一依赖（大小写差异）同时出现在注解与 requiredMods → 只收一条，区间取注解
        let mut info = empty_info();
        let jar = test_jar_with_entries(&[
            (
                "mcmod.info",
                r#"[{"modid":"m","name":"M","useDependencyInformation":true,
                    "requiredMods":["jei@[4.6.0,)"]}]"#,
            ),
            ("com/x/Mod.class", "x required-after:JEI@[4.7.0,); y"),
        ]);
        let mut archive = ZipArchive::new(Cursor::new(jar)).unwrap();
        let mcmod = read_zip_entry(&mut archive, "mcmod.info")
            .ok()
            .flatten()
            .unwrap();
        parse_mcmod_json(&mut archive, &mcmod, &mut info);
        let jei = info
            .dependencies
            .iter()
            .find(|d| d.mod_id.eq_ignore_ascii_case("jei"))
            .expect("注解依赖应被收录");
        assert_eq!(
            jei.version_range, "[4.7.0,)",
            "注解先入列，requiredMods 不覆盖"
        );
        assert_eq!(info.dependencies.len(), 1, "去重后只留一条");
        assert_eq!(info.mod_id, "m");
    }

    #[test]
    fn legacy_annotation_scanner_ignores_soft_ordering_markers() {
        // after:/before: 是加载顺序声明，缺失不崩溃 → 不收录
        let mut info = empty_info();
        let jar = test_jar_with_entries(&[
            ("mcmod.info", r#"[{"modid":"m","name":"M"}]"#),
            (
                "com/x/Mod.class",
                "deps;after:someorder;before:otherorder;required-after:hard",
            ),
        ]);
        let mut archive = ZipArchive::new(Cursor::new(jar)).unwrap();
        let mcmod = read_zip_entry(&mut archive, "mcmod.info")
            .ok()
            .flatten()
            .unwrap();
        parse_mcmod_json(&mut archive, &mcmod, &mut info);
        let ids: Vec<&str> = info
            .dependencies
            .iter()
            .map(|d| d.mod_id.as_str())
            .collect();
        assert_eq!(ids, vec!["hard"], "{ids:?}");
    }

    #[test]
    fn legacy_dep_spec_parses_id_and_range() {
        let dep = parse_legacy_dep_spec("jei@[4.7.0,)").unwrap();
        assert_eq!(dep.mod_id, "jei");
        assert_eq!(dep.version_range, "[4.7.0,)");
        let dep = parse_legacy_dep_spec("mekanism@[1.12.2-9.8.3.390]").unwrap();
        assert_eq!(dep.mod_id, "mekanism");
        assert_eq!(dep.version_range, "[1.12.2-9.8.3.390]");
        let dep = parse_legacy_dep_spec("plain").unwrap();
        assert_eq!(dep.mod_id, "plain");
        assert_eq!(dep.version_range, "");
        let dep = parse_legacy_dep_spec("x@*").unwrap();
        assert_eq!(dep.version_range, "");
        assert!(parse_legacy_dep_spec("").is_none());
        assert!(parse_legacy_dep_spec("@[1.0,)").is_none());
    }

    #[test]
    fn e2e_legacy_forge_mcmod_jar_reports_annotation_dependency() {
        // 核心回归（用户场景）：1.12.2 Forge mod，硬依赖只写在 @Mod 注解里，
        // mcmod.info 的 dependencies 是软列表。修复前 mod_id/dependencies 恒为空
        // → 缺失依赖检测对该世代完全失效。
        let (game_dir, mods_dir) = temp_game_dir("legacy-forge");
        write_jar(
            &mods_dir.join("JustEnoughResources-0.9.3.jar"),
            &[
                (
                    "mcmod.info",
                    r#"[{"modid":"jeresources","name":"Just Enough Resources",
                        "version":"0.9.3","dependencies":["JEI"]}]"#,
                ),
                (
                    "jeresources/JEResources.class",
                    "trailing.required-after:jei@[4.7.0,);required-after:forge@[14.23.5.2779,);",
                ),
            ],
        );
        write_jar(
            &mods_dir.join("VoxelMap-1.9.28.jar"),
            &[(
                "mcmod.info",
                r#"[{"modid":"voxelmap","name":"VoxelMap","version":"1.9.28",
                    "dependencies":[]}]"#,
            )],
        );

        let list = scan(&game_dir);
        let jer = find(&list, "JustEnoughResources-0.9.3.jar");
        assert_eq!(jer.mod_id, "jeresources");
        let ids: Vec<&str> = jer.dependencies.iter().map(|d| d.mod_id.as_str()).collect();
        assert!(ids.contains(&"jei"), "注解硬依赖必须解析: {ids:?}");
        assert!(ids.contains(&"forge"), "{ids:?}");
        assert!(!ids.contains(&"JEI"), "软依赖列表不得混入: {ids:?}");
        assert_eq!(find(&list, "VoxelMap-1.9.28.jar").dependencies.len(), 0);
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn e2e_legacy_mekanism_required_mods_only() {
        // Mekanism 1.12.2：useDependencyInformation=true，requiredMods 只有 forge
        //（恒被加载器满足）；12 项 dependencies 是可选软依赖，收了会大面积误报。
        let (game_dir, mods_dir) = temp_game_dir("legacy-mek");
        write_jar(
            &mods_dir.join("Mekanism-1.12.2.jar"),
            &[(
                "mcmod.info",
                r#"[{"modid":"mekanism","name":"Mekanism","version":"9.8.3.390",
                    "useDependencyInformation":true,
                    "requiredMods":["forge@[14.23.5.2768,)"],
                    "dependencies":["redstoneflux","mcmultipart","jei",
                        "buildcraftcore","ic2","computercraft"]}]"#,
            )],
        );
        let list = scan(&game_dir);
        let mek = find(&list, "Mekanism-1.12.2.jar");
        assert_eq!(mek.mod_id, "mekanism");
        let ids: Vec<&str> = mek.dependencies.iter().map(|d| d.mod_id.as_str()).collect();
        assert_eq!(ids, vec!["forge"], "只收 requiredMods: {ids:?}");
        let _ = std::fs::remove_dir_all(&game_dir);
    }

    /// 把 (条目名, 内容) 写成内存 zip 字节（单元测试用）。
    fn test_jar_with_entries(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            for (name, content) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(content.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn e2e_jar_without_dependency_metadata_is_not_an_error() {
        // 异常/边界：无 fabric.mod.json / mods.toml 的 jar → 不报错，deps 为空，
        // 名称回退文件名，id 保持空串（不伪造）。
        let (game_dir, mods_dir) = temp_game_dir("nodeps");
        write_jar(
            &mods_dir.join("mystery-1.0.jar"),
            &[("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\n")],
        );

        let list = scan(&game_dir);
        assert_eq!(list.len(), 1, "无元数据 jar 仍应列出");
        assert!(list[0].dependencies.is_empty(), "无依赖声明 → 空列表");
        assert!(list[0].mod_id.is_empty(), "无法解析 id → 空串（不伪造）");
        assert_eq!(list[0].name, "mystery-1.0", "名称回退文件名主干");
        let _ = std::fs::remove_dir_all(&game_dir);
    }
}
