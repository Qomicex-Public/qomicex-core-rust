//! 存档列表 / 重命名 / 备份（NBT 解析）（B10，对应 Saves.cs）
//!
//! 对应源文件：Services/Expansion/Local/Saves.cs（`Saves : LocalResourceBase`，518 行）。
//! 语义要点：
//! - 扫描：saves 目录（版本分段 `{gameDirectory}/versions/{version}/saves`，否则
//!   `{gameDirectory}/saves`），目录不存在 → 空列表；
//! - level.dat 解析：NBT 读写统一走公共模块 `util/nbt_full`（gzip 魔数探测 + 全类型）——
//!   原私有解析器（B10 按源 Saves.cs 移植）已提升为该公共模块（见 util/nbt_full.rs 头）；
//! - 信息读取：LevelName（缺失/类型不符/任何异常 → None → 目录名兜底）、LastPlayed
//!   （同 → 0）、icon.png（base64，失败 → 空串）——语义对应 util/nbt.rs 的
//!   get_optional_string / get_optional_bool，但 util 类型承载不了 Long/Int 等（见 ⚠️ 差异）；
//! - 重命名：Data.LevelName 改写（gzip 压缩写回）+ 目录改名；任一步失败 → 写回原
//!   level.dat 字节 + 恢复原 LevelName + 异常上抛；
//! - 备份：递归复制目录为 `{name}_backup_{yyyyMMdd_HHmmss}`，目标已存在 → 直接返回；
//! - 异常语义：契约 trait 全 void（无 Result，同源 void 方法）→ C# throw → `panic!`
//!   （DirectoryNotFoundException / FileNotFoundException / IOException 的消息文本保留
//!   源 Exception.Message）。
//!
//! ⚠️ NBT 差异说明（历史，见 P48 日志）：任务指令原定复用 util/nbt.rs 的
//! read / get_optional_string / get_optional_bool，但 B2 定案（b2_decisions，NbtIO.cs 专用）
//! 仅支持 Byte(→bool)/String/List(Compound)/Compound 四种类型，无 Long/Int/Float/Double/
//! IntArray/LongArray 变体 → 无法解析真实 level.dat（LastPlayed 为 TAG_Long 等）→
//! 按源 Saves.cs 结构移植其内嵌全量解析器；该解析器后续在「存档设置管理」功能中
//! 提升为公共模块 `util/nbt_full`（存档设置 level.dat 编辑复用，避免重复实现）。
//! MAPPING_TABLE utils.NBT 条目仅映射 NbtIO.cs → util/nbt.rs，不覆盖 Saves.cs 内嵌解析器。

use std::path::Path;

use super::level_dat;
use crate::api::local::SavesManager;
use crate::error::Error;
use crate::models::expansion::local::{LevelDatSettings, SaveInfo};
use crate::util::nbt_full::{NbtError, NbtValue, read as nbt_read, write_gzip as nbt_write_gzip};

/// 存档管理器（源：concrete class `Saves`，Services/Expansion/Local/Saves.cs）
pub(crate) struct Saves {
    /// HTTP 客户端（源字段 `_http`；B13 网络接线后使用）
    #[allow(dead_code)] // 待 B13 网络接线
    http: reqwest::Client,
    /// 游戏根目录（源字段 `_gameDirectory`）
    game_directory: String,
    /// 游戏版本（源字段 `_version`，用于版本分段目录）
    version: String,
    /// 是否使用版本分段目录（源字段 `_versionSegmented`）
    version_segmented: bool,
    /// API Key（源字段 `_apiKey`；B13 网络接线后使用）
    #[allow(dead_code)] // 待 B13 网络接线
    api_key: String,
}

impl Saves {
    /// 创建存档管理器（源：`new Saves(HttpClient, gameDirectory, version, versionSegmented, apiKey)`；
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

    // ===== 文件扫描（源 #region 文件扫描）=====

    /// 扫描存档目录（源：GetSaveFolders）：
    /// 版本分段 → `{gameDirectory}/versions/{version}/saves`，否则 `{gameDirectory}/saves`；
    /// 目录不存在 → 空列表（源 Directory.Exists 前置检查）。
    /// 差异说明：源 Directory.GetDirectories 的 IO 异常向上抛出 → 此处 panic!
    /// （契约返回 Vec 无错误通道，见模块头"异常语义"）
    fn get_save_folders(&self) -> Vec<String> {
        let saves_directory = if self.version_segmented {
            Path::new(&self.game_directory)
                .join("versions")
                .join(&self.version)
                .join("saves")
        } else {
            Path::new(&self.game_directory).join("saves")
        };

        if !saves_directory.is_dir() {
            return Vec::new();
        }

        let entries = std::fs::read_dir(&saves_directory)
            .unwrap_or_else(|e| panic!("读取存档目录失败: {}: {e}", saves_directory.display()));
        let mut folders = Vec::new();
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| {
                panic!("读取存档目录项失败: {}: {e}", saves_directory.display())
            });
            let path = entry.path();
            if path.is_dir() {
                // 源 Directory.GetDirectories 返回完整路径
                folders.push(path.to_string_lossy().into_owned());
            }
        }
        folders
    }
}

impl SavesManager for Saves {
    /// 扫描存档列表（源：GetSaveList）：
    /// 每个存档目录 → LevelName（level.dat 缺失/解析失败 → `Path.GetFileName(folder)` 兜底）
    /// + LastPlayed（失败 → 0）+ icon.png（base64，失败 → 空串）
    fn get_save_list(&self) -> Vec<SaveInfo> {
        let folders = self.get_save_folders();
        let mut save_infos = Vec::with_capacity(folders.len());
        for folder in folders {
            let folder_path = Path::new(&folder);
            // 源：`ReadLevelName(folder) ?? Path.GetFileName(folder)`
            let level_name = read_level_name(folder_path).unwrap_or_else(|| folder_name(&folder));
            let last_played = read_last_played(folder_path);
            let icon = read_icon_from_save(folder_path);

            save_infos.push(SaveInfo {
                name: level_name,
                file_path: folder,
                last_played,
                icon,
            });
        }
        save_infos
    }

    /// 重命名存档（源：RenameSave）：
    /// 1. 目录/level.dat 存在性校验（缺失 → 抛 DirectoryNotFoundException /
    ///    FileNotFoundException → panic!，消息文本同源）；
    /// 2. 改写 level.dat 的 Data.LevelName（gzip 压缩写回）；
    /// 3. 目录改名为 `{parent}/{newName}`（目标目录已存在 → IOException → panic!）；
    /// 任一步失败 → 恢复 level.dat 原名 + panic!（源 catch：WriteLevelName 恢复后 rethrow）
    fn rename_save(&self, save_directory: &str, new_name: &str) {
        let save_dir = Path::new(save_directory);
        if !save_dir.is_dir() {
            panic!("Save directory not found: {save_directory}");
        }
        if !save_dir.join("level.dat").is_file() {
            panic!("level.dat not found in save directory: {save_directory}");
        }

        // 源：`ReadLevelName(saveDirectory) ?? Path.GetFileName(saveDirectory)`
        let original_name =
            read_level_name(save_dir).unwrap_or_else(|| folder_name(save_directory));

        if let Err(e) = write_level_name(save_dir, new_name) {
            // 源 catch：WriteLevelName(saveDirectory, originalName) 恢复原名后 rethrow。
            // 差异说明：恢复若也失败，源会以恢复异常取代原异常（catch 内 throw 无包裹）；
            // 此处忽略恢复错误（`let _ =`），保留原错误
            let _ = write_level_name(save_dir, &original_name);
            panic!("重命名存档失败 {save_directory}: {e}");
        }

        let parent_dir = save_dir.parent().unwrap_or_else(|| Path::new(""));
        let new_path = parent_dir.join(new_name);

        if new_path.is_dir() {
            let _ = write_level_name(save_dir, &original_name);
            panic!("Target directory already exists: {}", new_path.display());
        }

        if let Err(e) = std::fs::rename(save_dir, &new_path) {
            let _ = write_level_name(save_dir, &original_name);
            panic!(
                "重命名存档目录失败 {} -> {}: {e}",
                save_directory,
                new_path.display()
            );
        }
    }

    /// 备份存档（源：BackupSave）：
    /// 目录存在性校验 → 目标 `{parent}/{name}_backup_{yyyyMMdd_HHmmss}` →
    /// 目标已存在 → 直接返回（源提前 return）→ 递归复制目录
    fn backup_save(&self, save_directory: &str) {
        let save_dir = Path::new(save_directory);
        if !save_dir.is_dir() {
            panic!("Save directory not found: {save_directory}");
        }

        // 源：`DateTime.Now.ToString("yyyyMMdd_HHmmss")`（本地时间）
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let save_name = folder_name(save_directory);
        let parent_dir = save_dir.parent().unwrap_or_else(|| Path::new(""));
        let backup_path = parent_dir.join(format!("{save_name}_backup_{timestamp}"));

        if backup_path.is_dir() {
            return;
        }

        copy_directory_recursive(save_dir, &backup_path);
    }

    /// 读取存档设置（level.dat NBT，精选白名单字段；缺失字段取默认值）
    fn read_level_dat_settings(&self, save_directory: &str) -> Result<LevelDatSettings, Error> {
        level_dat::read_settings(Path::new(save_directory))
    }

    /// 更新存档设置（写前自动备份 level.dat.qomicex.bak，失败回滚原字节）
    fn update_level_dat_settings(
        &self,
        save_directory: &str,
        settings: &LevelDatSettings,
    ) -> Result<(), Error> {
        level_dat::update_settings(Path::new(save_directory), settings)
    }

    /// 从 level.dat_old 恢复（备份当前 level.dat 后覆盖）
    fn restore_level_dat_from_old(&self, save_directory: &str) -> Result<(), Error> {
        level_dat::restore_from_old(Path::new(save_directory))
    }
}

// ===== 存档信息读取（源 #region 存档信息读取）=====

/// 读取 level.dat 的 LevelName（源：ReadLevelName）：
/// level.dat 缺失 → None；解析异常 / Data 复合缺失 / LevelName 非字符串 → None
/// （源 try/catch 全捕获 → null；类型检查 `is string` 不符即跳过）
fn read_level_name(save_directory: &Path) -> Option<String> {
    let level_dat_path = save_directory.join("level.dat");
    if !level_dat_path.is_file() {
        return None;
    }
    let bytes = std::fs::read(&level_dat_path).ok()?;
    let root = nbt_read(&bytes).ok()?;
    // 源：`root.TryGetValue("Data", out var dataTag) && dataTag.Value is NbtCompound data`
    let data = match root.get("Data") {
        Some(NbtValue::Compound(data)) => data,
        _ => return None,
    };
    // 源：`data.TryGetValue("LevelName", ...) && nameTag.Value is string name`
    match data.get("LevelName") {
        Some(NbtValue::String(name)) => Some(name.clone()),
        _ => None,
    }
}

/// 读取 level.dat 的 LastPlayed（源：ReadLastPlayed）：
/// level.dat 缺失 → 0；解析异常 / Data 复合缺失 / LastPlayed 非 Long → 0（同源 catch → 0）
fn read_last_played(save_directory: &Path) -> i64 {
    let level_dat_path = save_directory.join("level.dat");
    if !level_dat_path.is_file() {
        return 0;
    }
    let Ok(bytes) = std::fs::read(&level_dat_path) else {
        return 0;
    };
    let Ok(root) = nbt_read(&bytes) else {
        return 0;
    };
    let Some(NbtValue::Compound(data)) = root.get("Data") else {
        return 0;
    };
    // 源：`data.TryGetValue("LastPlayed", ...) && lastPlayedTag.Value is long lastPlayed`
    match data.get("LastPlayed") {
        Some(NbtValue::Long(last_played)) => *last_played,
        _ => 0,
    }
}

/// 读取 icon.png 为 base64（源：ReadIconFromSave）：
/// 缺失 → 空串；读取/编码异常 → 空串（源 catch → string.Empty）
fn read_icon_from_save(save_directory: &Path) -> String {
    let icon_path = save_directory.join("icon.png");
    if !icon_path.is_file() {
        return String::new();
    }
    match std::fs::read(&icon_path) {
        Ok(bytes) => base64_encode(&bytes),
        Err(_) => String::new(),
    }
}

/// 对应源 `Convert.ToBase64String(byte[])`（MAPPING_TABLE runtime 映射：base64 crate）。
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 对应源 `Path.GetFileName`：目录路径取末段文件名（尾部分隔符/根目录 → 空串，
/// 源 GetFileName 对尾分隔符返回空串、对 null 返回 null——本调用点路径均非 null）
fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ===== 存档重命名（源 #region 存档重命名）=====

/// 改写 level.dat 的 LevelName 并 gzip 压缩写回（源：WriteLevelName）：
/// 1. 读原字节（源 ReadAllBytes 在 try 外：失败不触发恢复，直接上抛）；
/// 2. try 内：解析 → `Data` 复合缺失/非复合 → InvalidDataException
///    （"level.dat does not contain Data compound."）→ 改写 LevelName →
///    gzip 压缩写出（util::nbt_full::write_gzip，恒 gzip，同源 GZipStream(Compress)）；
/// 3. 任一步失败 → 写回原字节（源 catch：File.WriteAllBytes 恢复后 rethrow；
///    恢复失败被源吞掉 → 此处 `.ok()`）
fn write_level_name(save_directory: &Path, new_name: &str) -> Result<(), NbtError> {
    let level_dat_path = save_directory.join("level.dat");
    let original_bytes = std::fs::read(&level_dat_path)?;

    let result = (|| -> Result<(), NbtError> {
        let mut root = nbt_read(&original_bytes)?;
        // 源：`data["LevelName"] = new NbtValue { TagType = String, Value = newName }`
        //（Data 复合存在 → 原位替换/追加；缺失或非复合 → 抛 InvalidDataException）
        match root.get_mut("Data") {
            Some(NbtValue::Compound(data)) => {
                data.insert(
                    "LevelName".to_string(),
                    NbtValue::String(new_name.to_string()),
                );
            }
            _ => return Err(NbtError::NoDataCompound),
        }
        // 源：GZipStream(CompressionMode.Compress)。压缩级别为实现细节
        // （.NET Optimal vs flate2 默认级别，输出均为合法 gzip，不构成契约）
        let compressed = nbt_write_gzip(&root)?;
        std::fs::write(&level_dat_path, compressed)?;
        Ok(())
    })();

    if let Err(e) = result {
        // 源 catch：写回原字节后 rethrow
        let _ = std::fs::write(&level_dat_path, original_bytes);
        return Err(e);
    }
    Ok(())
}

// ===== 存档备份（源 #region 存档备份）=====

/// 递归复制目录（源：CopyDirectoryRecursive）：
/// 先 `Directory.CreateDirectory(dest)`，再复制全部文件（File.Copy 默认**不覆盖**），
/// 最后递归复制子目录。IO 失败 → 源无捕获直接上抛 → 此处 panic!。
/// 差异说明：`fs::copy` 默认覆盖 → 目标文件已存在时前置检查后 panic!（保 File.Copy 语义）；
/// 源枚举顺序（GetFiles 后 GetDirectories）→ 先文件后目录
fn copy_directory_recursive(source_dir: &Path, dest_dir: &Path) {
    std::fs::create_dir_all(dest_dir)
        .unwrap_or_else(|e| panic!("创建备份目录失败: {}: {e}", dest_dir.display()));

    let entries = std::fs::read_dir(source_dir)
        .unwrap_or_else(|e| panic!("读取备份源目录失败: {}: {e}", source_dir.display()));

    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|e| panic!("读取备份源目录项失败: {}: {e}", source_dir.display()));
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }

    for file in files {
        let dest_file = dest_dir.join(
            file.file_name().unwrap_or_default(), // read_dir 条目必有文件名
        );
        if dest_file.exists() {
            panic!("备份目标文件已存在: {}", dest_file.display());
        }
        std::fs::copy(&file, &dest_file).unwrap_or_else(|e| {
            panic!(
                "备份文件失败 {} -> {}: {e}",
                file.display(),
                dest_file.display()
            )
        });
    }

    for dir in dirs {
        let dest_sub_dir = dest_dir.join(dir.file_name().unwrap_or_default());
        copy_directory_recursive(&dir, &dest_sub_dir);
    }
}
