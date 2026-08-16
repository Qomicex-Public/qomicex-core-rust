//! 本地内容管理模型（B1）
//! 对应源：Models/Expansion/Local/ 下 6 个文件
//! (DataPackInfo.cs, ModInfo.cs, ResourcePackInfo.cs, SaveInfo.cs, ScreenshotInfo.cs, ShaderInfo.cs)

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 表示一个本地数据包（Data Pack）的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DataPackInfo {
    /// 数据包名称
    pub name: String,
    /// 数据包描述
    pub description: String,
    /// 数据包版本
    pub version: String,
    /// 数据包文件路径
    pub file_path: String,
    /// 是否为目录形式
    pub is_directory: bool,
    /// 数据包格式版本
    pub pack_format: i32,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}

/// 表示一个本地 Mod 的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    /// Mod 名称
    pub name: String,
    /// Mod 描述
    pub description: String,
    /// Mod 版本
    pub version: String,
    /// 作者列表
    pub authors: Vec<String>,
    /// Mod 文件路径
    pub file_path: String,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
    /// Modrinth 版本（文件）ID：SHA1 反查 `ProjectVersionInfo.id`（C# 响应里有但未落盘）
    pub modrinth_version_id: String,
    /// CurseForge 文件 ID：指纹反查 `FingerprintsFilesMeta.id`（C# 响应里有但未落盘）
    pub curse_forge_file_id: i64,
}

impl ModInfo {
    /// 是否为激活状态（源为 get-only 计算属性 `Active`，判断扩展名是否为 .jar）
    /// ⚠️ UNMAPPED：计算属性无法映射为 serde 字段，此处保留逻辑为方法，不参与序列化
    pub fn is_active(&self) -> bool {
        Path::new(&self.file_path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
    }
}

/// 表示一个本地资源包（Resource Pack）的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePackInfo {
    /// 资源包名称
    pub name: String,
    /// 资源包描述
    pub description: String,
    /// 资源包版本
    pub version: String,
    /// 资源包文件路径
    pub file_path: String,
    /// 是否为目录形式
    pub is_directory: bool,
    /// 资源包格式版本
    pub pack_format: i32,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}

/// 表示一个本地存档的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveInfo {
    /// 存档名称
    pub name: String,
    /// 存档文件路径
    pub file_path: String,
    /// 最后游玩时间戳
    pub last_played: i64,
    /// 图标路径
    pub icon: String,
}

/// 存档设置（level.dat `Data` 复合的精选白名单字段，camelCase 直通前端表单）。
/// 对应「存档设置管理」功能；未出现在 level.dat 的字段读取时取默认值，
/// 写入时按标准标签类型补齐。
///
/// 双格式映射（按内容探测，不依赖版本号，见 services/local/level_dat.rs）：
/// - 经典格式（≤1.21.1）：难度/硬核/锁定/出生点在 `Data` 顶层
///   （Difficulty/hardcore/DifficultyLocked/SpawnX/SpawnY/SpawnZ）；
/// - 过渡格式（1.21.2+，26.1snap6 前）：难度/硬核/锁定在 `Data.difficulty_settings`
///   复合（difficulty 为 String：peaceful/easy/normal/hard），出生点在 `Data.spawn.pos`
///   （IntArray [x,y,z]）；其余字段仍在 Data 顶层。
/// 重构格式（26.1snap6+）中 GameRules/天气/边界/流浪商人已移出 level.dat，
/// 本期 Z1 仍写经典键（新游戏忽略，不破坏存档），完整支持见二期任务。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LevelDatSettings {
    /// 存档名称（Data.LevelName，String）
    pub level_name: String,
    /// 游戏模式（Data.GameType，Int；0=生存 1=创造 2=冒险 3=旁观）
    pub game_type: i32,
    /// 难度（经典 Data.Difficulty Byte 或 difficulty_settings.difficulty String；0=和平 1=简单 2=普通 3=困难）
    pub difficulty: u8,
    /// 允许作弊（Data.allowCommands，Byte）
    pub allow_commands: bool,
    /// 硬核模式（经典 Data.hardcore 或 difficulty_settings.hardcore，Byte）
    pub hardcore: bool,
    /// 锁定难度（经典 Data.DifficultyLocked 或 difficulty_settings.locked，Byte）
    pub difficulty_locked: bool,
    /// 世界时间（Data.Time，Long；自世界创建起的总 tick）
    pub time: i64,
    /// 昼夜时间（Data.DayTime，Long；0-24000 循环）
    pub day_time: i64,
    /// 是否下雨（Data.raining，Byte）
    pub raining: bool,
    /// 是否雷暴（Data.thundering，Byte）
    pub thundering: bool,
    /// 晴天剩余时间（Data.clearWeatherTime，Int；0=无限）
    pub clear_weather_time: i32,
    /// 降雨剩余时间（Data.rainTime，Int）
    pub rain_time: i32,
    /// 雷暴剩余时间（Data.thunderTime，Int）
    pub thunder_time: i32,
    /// 出生点 X（经典 Data.SpawnX 或 Data.spawn.pos[0]，Int）
    pub spawn_x: i32,
    /// 出生点 Y（经典 Data.SpawnY 或 Data.spawn.pos[1]，Int）
    pub spawn_y: i32,
    /// 出生点 Z（经典 Data.SpawnZ 或 Data.spawn.pos[2]，Int）
    pub spawn_z: i32,
    /// 世界种子（Data.RandomSeed，Long）
    pub random_seed: i64,
    /// 流浪商人生成概率（Data.WanderingTraderSpawnChance，Int；0=禁用）
    pub wandering_trader_spawn_chance: i32,
    /// 流浪商人生成延迟（Data.WanderingTraderSpawnDelay，Int，单位 tick）
    pub wandering_trader_spawn_delay: i32,
    /// 世界边界中心 X（Data.BorderCenterX，Double）
    pub border_center_x: f64,
    /// 世界边界中心 Z（Data.BorderCenterZ，Double）
    pub border_center_z: f64,
    /// 世界边界大小（Data.BorderSize，Double）
    pub border_size: f64,
    /// 世界边界安全区（Data.BorderSafeZone，Double）
    pub border_safe_zone: f64,
    /// 边界外每格伤害（Data.BorderDamagePerBlock，Double）
    pub border_damage_per_block: f64,
    /// 边界警告距离（Data.BorderWarningBlocks，Double）
    pub border_warning_blocks: f64,
    /// 边界警告时间（Data.BorderWarningTime，Double，单位秒）
    pub border_warning_time: f64,
    /// 游戏规则（Data.GameRules，Compound(String) 子集）
    pub game_rules: LevelGameRules,
}

/// 精选游戏规则子集（Data.GameRules；未出现时读取取默认值，写入按字符串补齐）。
/// 布尔规则值为 String "true"/"false"；数值规则（randomTickSpeed/spawnRadius/
/// maxEntityCramming）值为十进制数字字符串。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LevelGameRules {
    /// 死亡不掉落（keepInventory，默认 false）
    pub keep_inventory: bool,
    /// 昼夜循环（doDaylightCycle，默认 true）
    pub do_daylight_cycle: bool,
    /// 火焰蔓延（doFireTick，默认 true）
    pub do_fire_tick: bool,
    /// 生物破坏方块（mobGriefing，默认 true）
    pub mob_griefing: bool,
    /// 生物生成（doMobSpawning，默认 true）
    pub do_mob_spawning: bool,
    /// 天气循环（doWeatherCycle，默认 true）
    pub do_weather_cycle: bool,
    /// 生物掉落物（doMobLoot，默认 true）
    pub do_mob_loot: bool,
    /// 方块掉落物（doTileDrops，默认 true）
    pub do_tile_drops: bool,
    /// 实体掉落物（doEntityDrops，默认 true）
    pub do_entity_drops: bool,
    /// 自然生命恢复（doNaturalRegeneration，默认 true）
    pub do_natural_regeneration: bool,
    /// 立即重生（doImmediateRespawn，默认 false）
    pub do_immediate_respawn: bool,
    /// 幻翼生成（doInsomnia，默认 true）
    pub do_insomnia: bool,
    /// 掠夺者巡逻队生成（doPatrolSpawning，默认 true）
    pub do_patrol_spawning: bool,
    /// 流浪商人生成（doTraderSpawning，默认 true）
    pub do_trader_spawning: bool,
    /// 溺水伤害（drowningDamage，默认 true）
    pub drowning_damage: bool,
    /// 摔落伤害（fallDamage，默认 true）
    pub fall_damage: bool,
    /// 火焰伤害（fireDamage，默认 true）
    pub fire_damage: bool,
    /// 冰冻伤害（freezeDamage，默认 true）
    pub freeze_damage: bool,
    /// 死亡消息（showDeathMessages，默认 true）
    pub show_death_messages: bool,
    /// 公告进度（announceAdvancements，默认 true）
    pub announce_advancements: bool,
    /// 命令方块输出（commandBlockOutput，默认 true）
    pub command_block_output: bool,
    /// 命令反馈（sendCommandFeedback，默认 true）
    pub send_command_feedback: bool,
    /// 简化调试信息（reducedDebugInfo，默认 false）
    pub reduced_debug_info: bool,
    /// 禁用鞘翅移动检查（disableElytraMovementCheck，默认 false）
    pub disable_elytra_movement_check: bool,
    /// 旁观者生成区块（spectatorsGenerateChunks，默认 true）
    pub spectators_generate_chunks: bool,
    /// 限制合成（doLimitedCrafting，默认 false）
    pub do_limited_crafting: bool,
    /// 随机刻速度（randomTickSpeed，默认 3；0=禁用）
    pub random_tick_speed: i32,
    /// 出生点半径（spawnRadius，默认 10）
    pub spawn_radius: i32,
    /// 实体堆叠上限（maxEntityCramming，默认 24）
    pub max_entity_cramming: i32,
}

/// 表示一个本地截图的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotInfo {
    /// 截图文件路径
    pub file_path: String,
    /// 截图文件名
    pub file_name: String,
    /// 创建时间（源为 DateTime，暂用原始字符串保真，类型决策见日志 ⚠️ UNMAPPED）
    pub created_at: String,
    /// 文件大小（字节）
    pub file_size: i64,
}

/// 表示一个本地光影包（Shader）的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShaderInfo {
    /// 光影包名称
    pub name: String,
    /// 光影包描述
    pub description: String,
    /// 光影包版本
    pub version: String,
    /// 光影包文件路径
    pub file_path: String,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}
