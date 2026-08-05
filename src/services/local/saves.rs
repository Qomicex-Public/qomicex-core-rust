//! 存档列表 / 重命名 / 备份（NBT 解析）（B10，对应 Saves.cs）
//!
//! 对应源文件：Services/Expansion/Local/Saves.cs（`Saves : LocalResourceBase`，518 行）。
//! 语义要点：
//! - 扫描：saves 目录（版本分段 `{gameDirectory}/versions/{version}/saves`，否则
//!   `{gameDirectory}/saves`），目录不存在 → 空列表；
//! - level.dat 解析：源文件内嵌**全量** NBT 解析器（Byte/Short/Int/Long/Float/Double/
//!   String/List/Compound/IntArray/LongArray + gzip 魔数探测）→ 本文件私有移植；
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
//! ⚠️ NBT 差异说明（与任务指令的偏差，见 P48 日志）：任务指令原定复用 util/nbt.rs 的
//! read / get_optional_string / get_optional_bool，但 B2 定案（b2_decisions，NbtIO.cs 专用）
//! 仅支持 Byte(→bool)/String/List(Compound)/Compound 四种类型，无 Long/Int/Float/Double/
//! IntArray/LongArray 变体 → 无法解析真实 level.dat（LastPlayed 为 TAG_Long 等）→
//! 按源 Saves.cs 结构移植其内嵌全量解析器（源解析器本就是类内私有成员，结构一致）。
//! MAPPING_TABLE utils.NBT 条目仅映射 NbtIO.cs → util/nbt.rs，不覆盖 Saves.cs 内嵌解析器。
//!
//! ⚠️ 需要依赖（Cargo.toml 本批禁止修改，待后续批次声明；同 mods.rs P45 约定）：
//! - flate2 = "1"（源 GZipStream 读/写；Cargo.lock 已有传递依赖 flate2 1.1.9）
//! - chrono（源 DateTime.Now.ToString("yyyyMMdd_HHmmss") 本地时间戳）
//! - base64 = "1"（源 Convert.ToBase64String）

use std::io::{Read, Write};
use std::path::Path;

use crate::api::local::SavesManager;
use crate::models::expansion::local::SaveInfo;

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
        let original_name = read_level_name(save_dir).unwrap_or_else(|| folder_name(save_directory));

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
        // ⚠️ 需要依赖: chrono（Cargo.toml 尚未引入；本批禁止修改 → 待后续批次声明，
        // 同 mods.rs base64 约定；B1 定案 chrono 决策推迟）
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let save_name = folder_name(save_directory);
        let parent_dir = save_dir.parent().unwrap_or_else(|| Path::new(""));
        let backup_path = parent_dir.join(format!("{save_name}_backup_{timestamp}"));

        if backup_path.is_dir() {
            return;
        }

        copy_directory_recursive(save_dir, &backup_path);
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
    let root = parse_level_dat(&bytes).ok()?;
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
    let Ok(root) = parse_level_dat(&bytes) else {
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
/// ⚠️ 需要依赖: base64 = "1"（Cargo.toml 尚未引入；本批禁止修改 Cargo.toml →
/// 待后续批次声明，同 mods.rs P45 约定）
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
///    gzip 压缩写出（源 GZipStream(CompressionMode.Compress)，始终压缩）；
/// 3. 任一步失败 → 写回原字节（源 catch：File.WriteAllBytes 恢复后 rethrow；
///    恢复失败被源吞掉 → 此处 `.ok()`）
fn write_level_name(save_directory: &Path, new_name: &str) -> Result<(), NbtError> {
    let level_dat_path = save_directory.join("level.dat");
    let original_bytes = std::fs::read(&level_dat_path)?;

    let result = (|| -> Result<(), NbtError> {
        let mut root = parse_level_dat(&original_bytes)?;
        // 源：`data["LevelName"] = new NbtValue { TagType = String, Value = newName }`
        //（Data 复合存在 → 原位替换/追加；缺失或非复合 → 抛 InvalidDataException）
        match root.get_mut("Data") {
            Some(NbtValue::Compound(data)) => {
                data.insert("LevelName".to_string(), NbtValue::String(new_name.to_string()));
            }
            _ => return Err(NbtError::NoDataCompound),
        }
        // 源：GZipStream(CompressionMode.Compress)。压缩级别为实现细节
        // （.NET Optimal vs flate2 默认级别，输出均为合法 gzip，不构成契约）
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        write_root_compound(&mut encoder, &root)?;
        let compressed = encoder.finish()?;
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
        let entry = entry.unwrap_or_else(|e| {
            panic!("读取备份源目录项失败: {}: {e}", source_dir.display())
        });
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
        std::fs::copy(&file, &dest_file)
            .unwrap_or_else(|e| panic!("备份文件失败 {} -> {}: {e}", file.display(), dest_file.display()));
    }

    for dir in dirs {
        let dest_sub_dir = dest_dir.join(dir.file_name().unwrap_or_default());
        copy_directory_recursive(&dir, &dest_sub_dir);
    }
}

// ===== NBT 解析器（源 #region NBT 解析器）=====

/// 对应源私有常量类 NbtTagType（全部 13 个常量）。
/// ⚠️ ByteArray(7) 源虽声明，但读路径 switch 无该分支 → 落入 default 抛异常 → 保留
#[allow(dead_code)] // 源声明但读路径不支持（同源）
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// 对应源私有 struct NbtValue（`TagType` 字节 + `object Value`；
/// object 的运行时类型分派 → Rust 枚举变体，TagType 由 tag_type() 推导，杜绝不一致）
#[derive(Debug, Clone, PartialEq)]
enum NbtValue {
    Byte(u8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    List(NbtList),
    Compound(NbtCompound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtValue {
    /// 对应源 `NbtValue.TagType` 字段（写出时写回类型字节）
    fn tag_type(&self) -> u8 {
        match self {
            NbtValue::Byte(_) => TAG_BYTE,
            NbtValue::Short(_) => TAG_SHORT,
            NbtValue::Int(_) => TAG_INT,
            NbtValue::Long(_) => TAG_LONG,
            NbtValue::Float(_) => TAG_FLOAT,
            NbtValue::Double(_) => TAG_DOUBLE,
            NbtValue::String(_) => TAG_STRING,
            NbtValue::List(_) => TAG_LIST,
            NbtValue::Compound(_) => TAG_COMPOUND,
            NbtValue::IntArray(_) => TAG_INT_ARRAY,
            NbtValue::LongArray(_) => TAG_LONG_ARRAY,
        }
    }
}

/// 对应源私有 sealed class NbtList（`ElementType` + `Items`）
#[derive(Debug, Clone, PartialEq)]
struct NbtList {
    element_type: u8,
    items: Vec<NbtValue>,
}

/// 对应源私有 sealed class NbtCompound : Dictionary<string, NbtValue>。
/// ⚠️ 与 B2 util/nbt.rs 的 NbtCompound（HashMap）差异：此处用 `Vec<(String, NbtValue)>`
/// 保**插入序**——.NET Dictionary 无删除操作时按插入序迭代 → 写回 level.dat 的条目
/// 顺序与源一致（HashMap 随机序会改变写出字节布局）；替换已有键保原位（同 .NET）。
/// 查找 O(n)，level.dat 条目规模下无性能影响（纯读路径最后一次按顺序写回）
#[derive(Debug, Clone, PartialEq, Default)]
struct NbtCompound {
    entries: Vec<(String, NbtValue)>,
}

impl NbtCompound {
    /// 新建空复合标签
    fn new() -> Self {
        Self::default()
    }

    /// 对应源 Dictionary.TryGetValue
    fn get(&self, name: &str) -> Option<&NbtValue> {
        self.entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut NbtValue> {
        self.entries
            .iter_mut()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// 对应源索引器赋值 `compound[name] = value`（已存在 → 原位替换，序不变；
    /// 不存在 → 追加到末尾，同 .NET Dictionary 插入序）
    fn insert(&mut self, name: String, value: NbtValue) {
        if let Some(entry) = self.entries.iter_mut().find(|(key, _)| *key == name) {
            entry.1 = value;
        } else {
            self.entries.push((name, value));
        }
    }

    /// 对应源 `foreach (var entry in compound)`（插入序）
    fn iter(&self) -> impl Iterator<Item = &(String, NbtValue)> {
        self.entries.iter()
    }
}

/// NBT 解析/写入错误（对应源 InvalidDataException / EndOfStreamException /
/// InvalidOperationException / IOException；消息文本保留源 Exception.Message）
#[derive(Debug, thiserror::Error)]
enum NbtError {
    /// 源 ReadRootCompound：Expected root compound tag, but found type {tagType}.
    #[error("Expected root compound tag, but found type {0}.")]
    ExpectedRootCompound(u8),
    /// 源 ReadTagPayload default 分支：Unsupported NBT tag type {tagType}.
    #[error("Unsupported NBT tag type {0}.")]
    UnsupportedNbtTagType(u8),
    /// 源 ReadListPayload：NBT list length cannot be negative.
    #[error("NBT list length cannot be negative.")]
    NegativeListLength,
    /// 源 ReadIntArrayPayload：NBT int array length cannot be negative.
    #[error("NBT int array length cannot be negative.")]
    NegativeIntArrayLength,
    /// 源 ReadLongArrayPayload：NBT long array length cannot be negative.
    #[error("NBT long array length cannot be negative.")]
    NegativeLongArrayLength,
    /// 源 WriteString：NBT string length exceeds UInt16.MaxValue.
    #[error("NBT string length exceeds UInt16.MaxValue.")]
    StringTooLong,
    /// 源 WriteLevelName 的 Data 复合缺失：level.dat does not contain Data compound.
    #[error("level.dat does not contain Data compound.")]
    NoDataCompound,
    /// 源 ReadString 的 EndOfStreamException / 其余 read_exact 到流尾
    /// （BinaryReader 各 Read 的 EndOfStreamException）
    #[error("Unexpected end of stream while reading NBT string.")]
    UnexpectedEndOfStream,
    /// 源 IO 异常（BinaryWriter 写失败 IOException / File 读写失败）
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 对应源 CreateReadStream + ReadRootCompound 组合：
/// 探测 gzip 魔数（0x1F 0x8B，前 2 字节不足 2 字节视为非 gzip）→ gzip 解压读取，
/// 否则原样读取（源 stream.Read 后复位 Position 语义等价整块字节判定）。
/// ⚠️ 需要依赖: flate2 = "1"（对应源 GZipStream；Cargo.toml 尚未引入，
/// 本批禁止修改 → 待后续批次声明，同 mods.rs base64 约定；Cargo.lock 已有传递依赖
/// flate2 1.1.9，后续声明即用）
fn parse_level_dat(bytes: &[u8]) -> Result<NbtCompound, NbtError> {
    let is_gzip = bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B;
    let mut reader: Box<dyn Read + '_> = if is_gzip {
        Box::new(flate2::read::GzDecoder::new(bytes))
    } else {
        Box::new(bytes)
    };
    read_root_compound(&mut reader)
}

/// 对应源 ReadRootCompound：读类型必须是 Compound，根名称读出后丢弃，再读负载
fn read_root_compound<R: Read>(reader: &mut R) -> Result<NbtCompound, NbtError> {
    let tag_type = read_u8(reader)?;
    if tag_type != TAG_COMPOUND {
        return Err(NbtError::ExpectedRootCompound(tag_type));
    }
    // 源 `_ = ReadString(reader);`：根名称读出后丢弃
    read_string(reader)?;
    read_compound_payload(reader)
}

/// 对应源 WriteRootCompound：写 Compound 类型、空名称、负载
fn write_root_compound<W: Write>(writer: &mut W, root: &NbtCompound) -> Result<(), NbtError> {
    writer.write_all(&[TAG_COMPOUND])?;
    write_string(writer, "")?;
    write_compound_payload(writer, root)
}

/// 对应源 ReadCompoundPayload：循环读类型，End 结束；否则读名称 + 负载
fn read_compound_payload<R: Read>(reader: &mut R) -> Result<NbtCompound, NbtError> {
    let mut compound = NbtCompound::new();
    loop {
        let tag_type = read_u8(reader)?;
        if tag_type == TAG_END {
            return Ok(compound);
        }
        let name = read_string(reader)?;
        let value = read_tag_payload(reader, tag_type)?;
        compound.insert(name, value);
    }
}

/// 对应源 ReadTagPayload 的 switch（10 种类型；源 ByteArray(7) 等未支持类型 →
/// switch default 抛 InvalidDataException）
fn read_tag_payload<R: Read>(reader: &mut R, tag_type: u8) -> Result<NbtValue, NbtError> {
    match tag_type {
        TAG_BYTE => Ok(NbtValue::Byte(read_u8(reader)?)),
        TAG_SHORT => Ok(NbtValue::Short(read_i16_big_endian(reader)?)),
        TAG_INT => Ok(NbtValue::Int(read_i32_big_endian(reader)?)),
        TAG_LONG => Ok(NbtValue::Long(read_i64_big_endian(reader)?)),
        TAG_FLOAT => Ok(NbtValue::Float(read_f32_big_endian(reader)?)),
        TAG_DOUBLE => Ok(NbtValue::Double(read_f64_big_endian(reader)?)),
        TAG_STRING => Ok(NbtValue::String(read_string(reader)?)),
        TAG_LIST => Ok(NbtValue::List(read_list_payload(reader)?)),
        TAG_COMPOUND => Ok(NbtValue::Compound(read_compound_payload(reader)?)),
        TAG_INT_ARRAY => Ok(NbtValue::IntArray(read_int_array_payload(reader)?)),
        TAG_LONG_ARRAY => Ok(NbtValue::LongArray(read_long_array_payload(reader)?)),
        _ => Err(NbtError::UnsupportedNbtTagType(tag_type)),
    }
}

/// 对应源 ReadListPayload：元素类型 + 大端 i32 长度；长度负数抛异常；
/// 元素类型任意（源经 ReadTagPayload 递归分派，与 B2 NbtIO 的
/// 仅 Compound 元素约束不同——此处按源 Saves.cs 语义）
fn read_list_payload<R: Read>(reader: &mut R) -> Result<NbtList, NbtError> {
    let element_type = read_u8(reader)?;
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeListLength);
    }
    let mut items = Vec::with_capacity(length as usize);
    for _ in 0..length {
        items.push(read_tag_payload(reader, element_type)?);
    }
    Ok(NbtList {
        element_type,
        items,
    })
}

/// 对应源 ReadIntArrayPayload：大端 i32 长度（负数抛异常）+ 大端 i32 元素
fn read_int_array_payload<R: Read>(reader: &mut R) -> Result<Vec<i32>, NbtError> {
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeIntArrayLength);
    }
    let mut array = Vec::with_capacity(length as usize);
    for _ in 0..length {
        array.push(read_i32_big_endian(reader)?);
    }
    Ok(array)
}

/// 对应源 ReadLongArrayPayload：大端 i32 长度（负数抛异常）+ 大端 i64 元素
fn read_long_array_payload<R: Read>(reader: &mut R) -> Result<Vec<i64>, NbtError> {
    let length = read_i32_big_endian(reader)?;
    if length < 0 {
        return Err(NbtError::NegativeLongArrayLength);
    }
    let mut array = Vec::with_capacity(length as usize);
    for _ in 0..length {
        array.push(read_i64_big_endian(reader)?);
    }
    Ok(array)
}

/// 对应源 WriteCompoundPayload：逐条目写命名标签，最后写 End
fn write_compound_payload<W: Write>(writer: &mut W, compound: &NbtCompound) -> Result<(), NbtError> {
    for (name, value) in compound.iter() {
        write_named_tag(writer, name, value)?;
    }
    writer.write_all(&[TAG_END])?;
    Ok(())
}

/// 对应源 WriteNamedTag：类型字节 + 名称 + 负载。
/// 源 default 分支（不支持的值类型抛 InvalidOperationException）在 Rust
/// 类型系统下不可达（枚举覆盖全类型），同 B2 util/nbt.rs 的处理
fn write_named_tag<W: Write>(writer: &mut W, name: &str, tag: &NbtValue) -> Result<(), NbtError> {
    writer.write_all(&[tag.tag_type()])?;
    write_string(writer, name)?;
    write_tag_value(writer, tag)
}

/// 对应源 WriteTagValue（List 元素负载，无名称）：
/// 按运行时类型分派写出（源 switch (tag.Value)）
fn write_tag_value<W: Write>(writer: &mut W, tag: &NbtValue) -> Result<(), NbtError> {
    match tag {
        NbtValue::Byte(value) => writer.write_all(&[*value])?,
        NbtValue::Short(value) => write_i16_big_endian(writer, *value)?,
        NbtValue::Int(value) => write_i32_big_endian(writer, *value)?,
        NbtValue::Long(value) => write_i64_big_endian(writer, *value)?,
        NbtValue::Float(value) => write_f32_big_endian(writer, *value)?,
        NbtValue::Double(value) => write_f64_big_endian(writer, *value)?,
        NbtValue::String(value) => write_string(writer, value)?,
        NbtValue::List(list) => {
            // 源：写 ElementType + 大端 i32 长度 + 逐元素负载
            writer.write_all(&[list.element_type])?;
            write_i32_big_endian(writer, list.items.len() as i32)?;
            for item in &list.items {
                write_tag_value(writer, item)?;
            }
        }
        NbtValue::Compound(compound) => write_compound_payload(writer, compound)?,
        NbtValue::IntArray(array) => {
            // 源：大端 i32 长度 + 逐元素
            write_i32_big_endian(writer, array.len() as i32)?;
            for value in array {
                write_i32_big_endian(writer, *value)?;
            }
        }
        NbtValue::LongArray(array) => {
            write_i32_big_endian(writer, array.len() as i32)?;
            for value in array {
                write_i64_big_endian(writer, *value)?;
            }
        }
    }
    Ok(())
}

/// 对应源 ReadString：u16 大端长度 + 按字节读取；
/// 长度不足 → EndOfStreamException（源 ReadBytes 后校验长度）；
/// UTF-8 解码用替换语义（源 Encoding.UTF8.GetString 遇无效字节替换为 U+FFFD）
fn read_string<R: Read>(reader: &mut R) -> Result<String, NbtError> {
    let length = read_u16_big_endian(reader)?;
    let mut bytes = vec![0u8; length as usize];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 对应源 WriteString：UTF-8 字节长度超过 UInt16.MaxValue 抛
/// InvalidOperationException（源：NBT string length exceeds UInt16.MaxValue.）
fn write_string<W: Write>(writer: &mut W, value: &str) -> Result<(), NbtError> {
    let bytes = value.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(NbtError::StringTooLong);
    }
    write_u16_big_endian(writer, bytes.len() as u16)?;
    writer.write_all(bytes)?;
    Ok(())
}

// ── 大端读写原语（源 Read*/Write*BigEndian 全组：ReadExactly + 字节反转 ↔ read_exact + from_be_bytes）──

/// 对应源 ReadInt32BigEndian
fn read_i32_big_endian<R: Read>(reader: &mut R) -> Result<i32, NbtError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(i32::from_be_bytes(bytes))
}

/// 对应源 WriteInt32BigEndian
fn write_i32_big_endian<W: Write>(writer: &mut W, value: i32) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadInt16BigEndian
fn read_i16_big_endian<R: Read>(reader: &mut R) -> Result<i16, NbtError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(i16::from_be_bytes(bytes))
}

/// 对应源 WriteInt16BigEndian
fn write_i16_big_endian<W: Write>(writer: &mut W, value: i16) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadInt64BigEndian
fn read_i64_big_endian<R: Read>(reader: &mut R) -> Result<i64, NbtError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(i64::from_be_bytes(bytes))
}

/// 对应源 WriteInt64BigEndian
fn write_i64_big_endian<W: Write>(writer: &mut W, value: i64) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadFloatBigEndian
fn read_f32_big_endian<R: Read>(reader: &mut R) -> Result<f32, NbtError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(f32::from_be_bytes(bytes))
}

/// 对应源 WriteFloatBigEndian
fn write_f32_big_endian<W: Write>(writer: &mut W, value: f32) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadDoubleBigEndian
fn read_f64_big_endian<R: Read>(reader: &mut R) -> Result<f64, NbtError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(f64::from_be_bytes(bytes))
}

/// 对应源 WriteDoubleBigEndian
fn write_f64_big_endian<W: Write>(writer: &mut W, value: f64) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 ReadUInt16BigEndian
fn read_u16_big_endian<R: Read>(reader: &mut R) -> Result<u16, NbtError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(u16::from_be_bytes(bytes))
}

/// 对应源 WriteUInt16BigEndian
fn write_u16_big_endian<W: Write>(writer: &mut W, value: u16) -> Result<(), NbtError> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

/// 对应源 BinaryReader.ReadByte（流尾 → EndOfStreamException）
fn read_u8<R: Read>(reader: &mut R) -> Result<u8, NbtError> {
    let mut byte = [0u8; 1];
    reader
        .read_exact(&mut byte)
        .map_err(|_| NbtError::UnexpectedEndOfStream)?;
    Ok(byte[0])
}
