//! servers.dat CRUD（B11，对应 ServerManager.cs 的服务器列表持久化部分）
//!
//! 对应源文件：Services/Options/ServerManager.cs（1247 行；本文件只移植 servers.dat 部分：
//! LoadServerList / SaveServerList / AddOrUpdateServer / RemoveServer / GetServer /
//! ServerFileExists / ClearServers / GetServerFilePath / ToServerEntry / ToNbtCompound / Clone）。
//! 源其余区域（服务器状态查询 / 局域网发现 / MC 协议通信 / SRV）在 P51 mc_ping.rs、
//! P52 lan_discovery.rs 以同 struct 另一 impl 块移植。
//!
//! servers.dat NBT 结构（tag 类型逐字保留源 ToNbtCompound/ToServerEntry）：
//! - 根：TAG_Compound（根名称空串，读出后丢弃），含 `servers` → TAG_List（元素 TAG_Compound）；
//! - 每个服务器条目 Compound 字段：`name`/`ip`/`icon` → TAG_String，
//!   `acceptTextures`/`hidden` → TAG_Byte（读为 bool，写回 1/0）；
//! - `icon` 缺失或空串 → 不写该 tag（源仅 `!string.IsNullOrEmpty` 时写入）；
//! - util/nbt.rs（B2）的 Byte/String/List(Compound)/Compound 已覆盖全部类型 → 无需扩展。
//!
//! 语义要点：
//! - 文件路径：版本分段 → `{gameDirectory}/versions/{version}/servers.dat`，
//!   否则 `{gameDirectory}/servers.dat`（源 GetServerFilePath）；
//! - 读取：servers.dat 缺失 → 尝试 servers.dat_old → 仍缺失 → 空列表；
//!   读流按 gzip 魔数（0x1F 0x8B）探测解压（源 CreateReadStream + GZipStream；
//!   flate2 已声明于 Cargo.toml）；写始终未压缩（源注释：Minecraft 客户端不支持 GZip）
//!   并同步写 servers.dat_old；
//! - 异常语义：契约同步方法无 Result（api/server.rs 注释"同步方法直接映射…暂不包装
//!   Result"）→ C# throw → panic!；NBT 读取/解析错误统一包裹为源 catch 文本
//!   "Failed to read Minecraft servers file '{path}': {message}"（源 EndOfStreamException /
//!   InvalidDataException 双 catch 同文本）；文件 IO 错误（源 try 外原样上抛）→
//!   panic! 带路径与 io 错误（同 saves.rs P48 约定）；
//! - 调试日志：源 Console.Error.WriteLine（[DEBUG-SERVERS] 前缀）→ eprintln!（纯 Rust，
//!   Android 兼容）；
//! - 差异：① C# `Clone()`（引用类型手工字段拷贝）→ Rust ServerEntry derive(Clone)，
//!   调用点直接 `server.clone()`；② `StringComparison.OrdinalIgnoreCase` →
//!   `eq_ignore_ascii_case`（.NET 为 Unicode 大小写折叠，ASCII 服务器地址场景等价）；
//!   ③ util/nbt.rs 的 NbtCompound 为 HashMap → 写出条目顺序非源插入序（合法 NBT，
//!   B2 已定案 HashMap 语义，读取方按名查找）。

use std::path::Path;

use crate::models::local::ServerEntry;
use crate::util::nbt::{self, NbtCompound, NbtError, NbtValue};

/// 服务器管理器（源：`ServerManager` 具体类，Services/Options/ServerManager.cs）。
/// 本文件定义 struct 并实现 servers.dat 持久化部分（服务器列表增删改查/读写）；
/// 字段 pub(crate)：P51 mc_ping.rs / P52 lan_discovery.rs 以同 struct 另一 impl 块
/// 补充 Ping/LAN/SRV 方法。契约 trait `crate::api::server::ServerManager` 的实现
/// 待三个文件就绪后由集成批次统一补全（Rust 不允许同一 trait 分多个 impl 块）。
pub(crate) struct ServerManager {
    /// 游戏根目录（源字段 `_gameDirectory`）
    pub(crate) game_directory: String,
    /// 游戏版本（源字段 `_version`，用于版本分段路径）
    pub(crate) version: String,
    /// 是否使用版本分段目录（源字段 `_versionSpecific`）
    pub(crate) version_specific: bool,
}

impl ServerManager {
    /// 创建服务器管理器（源构造函数：
    /// `new ServerManager(string gameDirectory, string version, bool versionSpecific)`；
    /// 源对 null 参数抛 ArgumentNullException → Rust 类型系统下不可达）
    pub(crate) fn new(game_directory: String, version: String, version_specific: bool) -> Self {
        Self {
            game_directory,
            version,
            version_specific,
        }
    }

    /// 加载服务器列表（源：LoadServerList）：
    /// servers.dat 缺失 → 尝试 servers.dat_old → 仍缺失 → 空列表；
    /// 根复合无 `servers` tag → 空列表（源 TryGetValue 失败分支）；
    /// `servers` 非 Compound 列表 → 源抛 InvalidDataException → panic!
    /// （"The 'servers' tag is not a compound list."，被源 catch 包裹为
    /// "Failed to read Minecraft servers file…"）；条目解析（ToServerEntry）错误同抛。
    pub(crate) fn load_server_list(&self) -> Vec<ServerEntry> {
        let mut server_file_path = self.get_server_file_path();
        eprintln!(
            "[DEBUG-SERVERS] 读取 servers.dat: {server_file_path} (存在: {})",
            Path::new(&server_file_path).is_file()
        );
        if !Path::new(&server_file_path).is_file() {
            // 尝试从 servers.dat_old 读取
            let old_path = format!("{server_file_path}_old");
            eprintln!(
                "[DEBUG-SERVERS] servers.dat 不存在，尝试 servers.dat_old: {old_path} (存在: {})",
                Path::new(&old_path).is_file()
            );
            if Path::new(&old_path).is_file() {
                server_file_path = old_path;
            } else {
                return Vec::new();
            }
        }

        // 源 try 块外：File.OpenRead 的 IO 异常原样上抛（无包裹）→ 直接 panic!（saves.rs 约定）
        let bytes = std::fs::read(&server_file_path)
            .unwrap_or_else(|e| panic!("读取 servers.dat 失败: {server_file_path}: {e}"));
        // 源 try 块内：EndOfStreamException / InvalidDataException 统一包裹后重新抛出
        let root = read_root_compound(&bytes).unwrap_or_else(|e| {
            panic!("Failed to read Minecraft servers file '{server_file_path}': {e}")
        });
        let Some(servers_tag) = root.get("servers") else {
            return Vec::new();
        };
        let compounds = match servers_tag {
            NbtValue::List(compounds) => compounds,
            // 源：`throw new InvalidDataException("The 'servers' tag is not a compound list.")`
            // → 被 catch(InvalidDataException) 包裹（同一 "Failed to read…" 文本）
            _ => panic!(
                "Failed to read Minecraft servers file '{server_file_path}': The 'servers' tag is not a compound list."
            ),
        };
        compounds
            .iter()
            .map(to_server_entry)
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|e| {
                panic!("Failed to read Minecraft servers file '{server_file_path}': {e}")
            })
    }

    /// 保存服务器列表（源：SaveServerList）：
    /// 目录缺失 → 创建（源 Path.GetDirectoryName 非空才 Directory.CreateDirectory）；
    /// 始终写未压缩 NBT（源注释：Minecraft 游戏客户端不支持 GZip 格式），
    /// 并同步写入 servers.dat_old（同字节）。
    pub(crate) fn save_server_list(&self, servers: &[ServerEntry]) {
        let server_file_path = self.get_server_file_path();
        // 源：`Path.GetDirectoryName` + `!string.IsNullOrWhiteSpace` + `Directory.CreateDirectory`
        let directory = Path::new(&server_file_path).parent();
        if let Some(directory) = directory {
            if !directory.as_os_str().is_empty() {
                std::fs::create_dir_all(directory)
                    .unwrap_or_else(|e| panic!("创建目录失败: {}: {e}", directory.display()));
            }
        }

        eprintln!(
            "[DEBUG-SERVERS] 写入 servers.dat: {server_file_path} (共 {} 个服务器)",
            servers.len()
        );
        for server in servers {
            eprintln!("[DEBUG-SERVERS]   - {} @ {}", server.name, server.address);
        }

        // 源：MemoryStream + NbtIO.Write → ToArray → 两次 File.WriteAllBytes
        let mut root = NbtCompound::new();
        root.insert(
            "servers".to_string(),
            NbtValue::List(servers.iter().map(to_nbt_compound).collect()),
        );
        let mut data = Vec::new();
        nbt::write(&mut data, &root)
            .unwrap_or_else(|e| panic!("写入 servers.dat NBT 失败 {server_file_path}: {e}"));

        std::fs::write(&server_file_path, &data)
            .unwrap_or_else(|e| panic!("写入 servers.dat 失败: {server_file_path}: {e}"));

        // 同步写入 servers.dat_old
        let old_file_path = format!("{server_file_path}_old");
        std::fs::write(&old_file_path, &data)
            .unwrap_or_else(|e| panic!("写入 servers.dat_old 失败: {old_file_path}: {e}"));
        eprintln!("[DEBUG-SERVERS] 同步写入 servers.dat_old: {old_file_path}");
    }

    /// 新增或更新服务器（源：AddOrUpdateServer）：
    /// 按地址（忽略大小写）匹配：命中 → 原位替换（源 `servers[index] = Clone(server)`），
    /// 未命中 → 追加（Clone）；随后保存。
    pub(crate) fn add_or_update_server(&self, server: &ServerEntry) {
        let mut servers = self.load_server_list();
        let index = servers
            .iter()
            .position(|existing| existing.address.eq_ignore_ascii_case(&server.address));
        if let Some(index) = index {
            servers[index] = server.clone();
        } else {
            servers.push(server.clone());
        }
        self.save_server_list(&servers);
    }

    /// 按地址移除服务器（源：RemoveServer）：
    /// RemoveAll 语义（全部匹配项移除）；有移除才写盘；返回是否移除成功。
    pub(crate) fn remove_server(&self, address: &str) -> bool {
        let mut servers = self.load_server_list();
        let original_len = servers.len();
        servers.retain(|server| !server.address.eq_ignore_ascii_case(address));
        let removed = servers.len() < original_len;
        if removed {
            self.save_server_list(&servers);
        }
        removed
    }

    /// 按地址获取服务器（源：GetServer；`ServerEntry?` → `Option<ServerEntry>`）
    pub(crate) fn get_server(&self, address: &str) -> Option<ServerEntry> {
        self.load_server_list()
            .into_iter()
            .find(|server| server.address.eq_ignore_ascii_case(address))
    }

    /// 服务器文件（servers.dat）是否存在（源：ServerFileExists；File.Exists 语义，
    /// 任何错误 → false）
    pub(crate) fn server_file_exists(&self) -> bool {
        Path::new(&self.get_server_file_path()).is_file()
    }

    /// 清空全部服务器（源：ClearServers → `SaveServerList(Array.Empty<ServerEntry>())`）
    pub(crate) fn clear_servers(&self) {
        self.save_server_list(&[]);
    }

    /// 获取服务器文件路径（源：GetServerFilePath）：
    /// 版本分段 → `{gameDirectory}/versions/{version}/servers.dat`，
    /// 否则 → `{gameDirectory}/servers.dat`。
    pub(crate) fn get_server_file_path(&self) -> String {
        if self.version_specific {
            return Path::new(&self.game_directory)
                .join("versions")
                .join(&self.version)
                .join("servers.dat")
                .to_string_lossy()
                .into_owned();
        }
        Path::new(&self.game_directory)
            .join("servers.dat")
            .to_string_lossy()
            .into_owned()
    }
}

// ===== NBT 转换（源 #region NBT 转换）=====

/// 服务器条目 → ServerEntry（源：ToServerEntry）：
/// name/ip 缺失 → 空串（源 `?? string.Empty`）；icon 缺失 → None；
/// acceptTextures/hidden 缺失 → false（源 GetOptionalBool 缺失分支）；
/// tag 存在但类型不符 → 错误（源 GetOptionalString/GetOptionalBool 抛异常）
fn to_server_entry(compound: &NbtCompound) -> Result<ServerEntry, NbtError> {
    Ok(ServerEntry {
        name: nbt::get_optional_string(compound, "name")?.unwrap_or_default(),
        address: nbt::get_optional_string(compound, "ip")?.unwrap_or_default(),
        icon_base64: nbt::get_optional_string(compound, "icon")?,
        accept_textures: nbt::get_optional_bool(compound, "acceptTextures")?,
        hidden: nbt::get_optional_bool(compound, "hidden")?,
    })
}

/// ServerEntry → NBT Compound（源：ToNbtCompound）：
/// name/ip → TAG_String（源 `server.Name ?? string.Empty`，模型非空 → 直接写）；
/// acceptTextures/hidden → TAG_Byte(1/0)；icon 仅非 null 且非空才写入
/// （源 `if (!string.IsNullOrEmpty(server.IconBase64))`）
fn to_nbt_compound(server: &ServerEntry) -> NbtCompound {
    let mut compound = NbtCompound::new();
    compound.insert("name".to_string(), NbtValue::String(server.name.clone()));
    compound.insert("ip".to_string(), NbtValue::String(server.address.clone()));
    compound.insert(
        "acceptTextures".to_string(),
        NbtValue::Byte(server.accept_textures),
    );
    compound.insert("hidden".to_string(), NbtValue::Byte(server.hidden));
    if let Some(icon) = &server.icon_base64 {
        if !icon.is_empty() {
            compound.insert("icon".to_string(), NbtValue::String(icon.clone()));
        }
    }
    compound
}

// ===== 读取辅助（源 #region 二进制读写辅助 的 CreateReadStream）=====

/// 对应源 `CreateReadStream` + `NbtIO.Read` 组合：
/// 按 gzip 魔数（0x1F 0x8B，前 2 字节不足视为非 gzip）探测 → GZipStream 解压读取，
/// 否则原样读取。
/// ⚠️ 差异说明：util/nbt.rs 文件头注明"源 Read 不支持压缩流"——该结论针对 NbtIO.cs；
/// ServerManager.cs 的 CreateReadStream 本身支持 gzip 魔数探测（GZipStream，
/// flate2 已声明于 Cargo.toml）→ 在此以 flate2 实现解压，util/nbt.rs 无需改动。
/// 损坏 gzip 数据的 io 错误经 read_exact 落为 NbtError::UnexpectedEndOfStream →
/// 与源 InvalidDataException 同样被 "Failed to read…" 包裹，仅内部文本略异
/// （源 GZipStream 的 InvalidDataException 消息无法逐字复现）。
fn read_root_compound(bytes: &[u8]) -> Result<NbtCompound, NbtError> {
    let is_gzip = bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B;
    if is_gzip {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        nbt::read(&mut decoder)
    } else {
        let mut cursor = std::io::Cursor::new(bytes);
        nbt::read(&mut cursor)
    }
}
