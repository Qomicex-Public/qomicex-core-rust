//! 存档设置（level.dat NBT）读写服务。
//!
//! 对应「存档设置管理」功能（前端实例管理 → 存档 tab → 每存档「设置」弹窗）。
//! 精选白名单字段（`LevelDatSettings`，见 models/expansion/local.rs）：名称 / 游戏模式 /
//! 难度（含锁定）/ 作弊 / 硬核 / 世界时间 / 昼夜时间 / 天气（状态+剩余时间）/ 出生点 /
//! 种子 / 流浪商人（概率+延迟）/ 世界边界 7 项 + 游戏规则子集（26 布尔 + 3 数值）。
//!
//! 双格式映射（**按内容探测**，不依赖 DataVersion 数字表）：
//! - 经典格式（≤1.21.1）：难度/硬核/锁定/出生点全部在 `Data` 顶层
//!   （`Difficulty` Byte / `hardcore` Byte / `DifficultyLocked` Byte / `SpawnX|Y|Z` Int）；
//! - 过渡格式（1.21.2+，26.1snap6 前）：`Data.difficulty_settings` 复合
//!   （`difficulty` String：peaceful/easy/normal/hard；`hardcore` Byte；`locked` Byte），
//!   出生点在 `Data.spawn.pos`（IntArray [x,y,z]）；其余字段仍在 Data 顶层。
//! 探测：`Data` 含 `difficulty_settings` → 难度块读写新结构；含 `spawn` → 出生点读写
//! `spawn.pos`；否则经典键。读取时两种结构取对应源，写入时只写当前存档所在结构的键
//! （另一结构的残留键保留不动，符合「未知键保留」）。
//!
//! 重构格式（26.1snap6+）已知限制（Z1 一期）：GameRules/天气/边界/流浪商人已移出
//! level.dat 到独立文件（game_rules.dat / weather.dat / world_border.dat /
//! wandering_trader.dat），本期仍写经典键（新游戏忽略、不破坏存档），
//! 完整支持见二期任务。
//!
//! 写入安全（方案 E1 定案）：
//! - 写前自动备份当前 level.dat → `level.dat.qomicex.bak`（备份失败 → 拒绝写入）；
//! - 任一步失败 → 恢复原字节到 level.dat（同 saves.rs write_level_name 语义）；
//! - 全树往返：只改白名单键，未知键/类型原样保留（NbtCompound 保插入序，字节布局稳定）。
//!
//! 错误语义：统一 `Error::Params { message }`（backend 映射 400；文件缺失由端点预检
//! 返回 404）。读取时缺失字段取默认值（不报错，同源 ReadLevelName 兜底语义）。

use std::path::Path;

use crate::error::Error;
use crate::models::expansion::local::{LevelDatSettings, LevelGameRules};
use crate::util::nbt_full::{self, NbtCompound, NbtValue};

/// level.dat 写前安全备份文件名
const BACKUP_FILE: &str = "level.dat.qomicex.bak";

/// 读取存档设置（缺失字段取默认值；level.dat 缺失/损坏 → Err）。
pub(crate) fn read_settings(save_directory: &Path) -> Result<LevelDatSettings, Error> {
    let root = read_root(save_directory)?;
    let data = match root.get("Data") {
        Some(NbtValue::Compound(data)) => data,
        // Data 缺失 → 全默认（同源 ReadLevelName 的兜底语义）
        _ => return Ok(default_settings()),
    };
    let (difficulty, hardcore, difficulty_locked) = read_difficulty_block(data);
    let (spawn_x, spawn_y, spawn_z) = read_spawn_block(data);
    Ok(LevelDatSettings {
        level_name: get_string(data, "LevelName").unwrap_or_default(),
        game_type: get_int(data, "GameType").unwrap_or(0),
        difficulty,
        allow_commands: get_byte(data, "allowCommands")
            .map(|v| v != 0)
            .unwrap_or(false),
        hardcore,
        difficulty_locked,
        time: get_long(data, "Time").unwrap_or(0),
        day_time: get_long(data, "DayTime").unwrap_or(0),
        raining: get_byte(data, "raining").map(|v| v != 0).unwrap_or(false),
        thundering: get_byte(data, "thundering")
            .map(|v| v != 0)
            .unwrap_or(false),
        clear_weather_time: get_int(data, "clearWeatherTime").unwrap_or(0),
        rain_time: get_int(data, "rainTime").unwrap_or(0),
        thunder_time: get_int(data, "thunderTime").unwrap_or(0),
        spawn_x,
        spawn_y,
        spawn_z,
        random_seed: get_long(data, "RandomSeed").unwrap_or(0),
        wandering_trader_spawn_chance: get_int(data, "WanderingTraderSpawnChance").unwrap_or(25),
        wandering_trader_spawn_delay: get_int(data, "WanderingTraderSpawnDelay").unwrap_or(2400),
        border_center_x: get_double(data, "BorderCenterX").unwrap_or(0.0),
        border_center_z: get_double(data, "BorderCenterZ").unwrap_or(0.0),
        border_size: get_double(data, "BorderSize").unwrap_or(60_000_000.0),
        border_safe_zone: get_double(data, "BorderSafeZone").unwrap_or(5.0),
        border_damage_per_block: get_double(data, "BorderDamagePerBlock").unwrap_or(0.2),
        border_warning_blocks: get_double(data, "BorderWarningBlocks").unwrap_or(5.0),
        border_warning_time: get_double(data, "BorderWarningTime").unwrap_or(15.0),
        game_rules: read_game_rules(data),
    })
}

/// 更新存档设置（写前备份 + 失败回滚；未知键保留）。
pub(crate) fn update_settings(
    save_directory: &Path,
    settings: &LevelDatSettings,
) -> Result<(), Error> {
    let level_dat_path = save_directory.join("level.dat");
    let original_bytes =
        std::fs::read(&level_dat_path).map_err(|e| io_err(&level_dat_path, "读取", e))?;

    let result = (|| -> Result<(), Error> {
        let mut root = nbt_full::read(&original_bytes).map_err(parse_err)?;
        // Data 复合缺失/非复合 → 重建（正常 level.dat 必有，此处仅健壮性兜底）
        if !matches!(root.get("Data"), Some(NbtValue::Compound(_))) {
            root.insert("Data".to_string(), NbtValue::Compound(NbtCompound::new()));
        }
        let data = match root.get_mut("Data") {
            Some(NbtValue::Compound(data)) => data,
            _ => {
                return Err(Error::Params {
                    message: "level.dat Data 复合无法访问".to_string(),
                    source: None,
                });
            }
        };
        apply_settings(data, settings);
        // E1：写前备份（备份失败 → 拒绝写入，不落盘）
        backup_current(&level_dat_path, &original_bytes)?;
        let compressed = nbt_full::write_gzip(&root).map_err(parse_err)?;
        std::fs::write(&level_dat_path, compressed)
            .map_err(|e| io_err(&level_dat_path, "写入", e))?;
        Ok(())
    })();

    if let Err(e) = result {
        // 失败回滚原字节（同 saves.rs write_level_name：恢复失败被吞掉）
        let _ = std::fs::write(&level_dat_path, original_bytes);
        return Err(e);
    }
    Ok(())
}

/// 从 `level.dat_old` 恢复（备份当前 level.dat 后覆盖；_old 缺失 → Err）。
/// 恢复源也要求是合法 NBT（防止把损坏的 _old 覆盖过去）。
pub(crate) fn restore_from_old(save_directory: &Path) -> Result<(), Error> {
    let level_dat_path = save_directory.join("level.dat");
    let old_path = save_directory.join("level.dat_old");
    if !old_path.is_file() {
        return Err(Error::Params {
            message: format!(
                "level.dat_old not found in save directory: {}",
                save_directory.display()
            ),
            source: None,
        });
    }
    let old_bytes = std::fs::read(&old_path).map_err(|e| io_err(&old_path, "读取", e))?;
    // 校验 _old 是合法 NBT（同 read 的 gzip 探测语义）
    nbt_full::read(&old_bytes).map_err(parse_err)?;
    if level_dat_path.is_file() {
        let original_bytes =
            std::fs::read(&level_dat_path).map_err(|e| io_err(&level_dat_path, "读取", e))?;
        backup_current(&level_dat_path, &original_bytes)?;
    }
    std::fs::write(&level_dat_path, &old_bytes).map_err(|e| io_err(&level_dat_path, "写入", e))?;
    Ok(())
}

// ===== 内部实现 =====

/// 全默认设置（Data 缺失时读取兜底）
fn default_settings() -> LevelDatSettings {
    LevelDatSettings {
        level_name: String::new(),
        game_type: 0,
        difficulty: 2,
        allow_commands: false,
        hardcore: false,
        difficulty_locked: false,
        time: 0,
        day_time: 0,
        raining: false,
        thundering: false,
        clear_weather_time: 0,
        rain_time: 0,
        thunder_time: 0,
        spawn_x: 0,
        spawn_y: 0,
        spawn_z: 0,
        random_seed: 0,
        wandering_trader_spawn_chance: 25,
        wandering_trader_spawn_delay: 2400,
        border_center_x: 0.0,
        border_center_z: 0.0,
        border_size: 60_000_000.0,
        border_safe_zone: 5.0,
        border_damage_per_block: 0.2,
        border_warning_blocks: 5.0,
        border_warning_time: 15.0,
        game_rules: default_game_rules(),
    }
}

/// 游戏规则默认值（对应 Minecraft 各规则游戏内默认）
fn default_game_rules() -> LevelGameRules {
    LevelGameRules {
        keep_inventory: false,
        do_daylight_cycle: true,
        do_fire_tick: true,
        mob_griefing: true,
        do_mob_spawning: true,
        do_weather_cycle: true,
        do_mob_loot: true,
        do_tile_drops: true,
        do_entity_drops: true,
        do_natural_regeneration: true,
        do_immediate_respawn: false,
        do_insomnia: true,
        do_patrol_spawning: true,
        do_trader_spawning: true,
        drowning_damage: true,
        fall_damage: true,
        fire_damage: true,
        freeze_damage: true,
        show_death_messages: true,
        announce_advancements: true,
        command_block_output: true,
        send_command_feedback: true,
        reduced_debug_info: false,
        disable_elytra_movement_check: false,
        spectators_generate_chunks: true,
        do_limited_crafting: false,
        random_tick_speed: 3,
        spawn_radius: 10,
        max_entity_cramming: 24,
    }
}

/// 难度/硬核/锁定：探测 `difficulty_settings`（过渡+重构格式）→ 新结构；否则经典键。
fn read_difficulty_block(data: &NbtCompound) -> (u8, bool, bool) {
    match data.get("difficulty_settings") {
        Some(NbtValue::Compound(ds)) => (
            difficulty_from_string(get_string(ds, "difficulty").as_deref()),
            get_byte(ds, "hardcore").map(|v| v != 0).unwrap_or(false),
            get_byte(ds, "locked").map(|v| v != 0).unwrap_or(false),
        ),
        _ => (
            get_byte(data, "Difficulty").unwrap_or(2),
            get_byte(data, "hardcore").map(|v| v != 0).unwrap_or(false),
            get_byte(data, "DifficultyLocked")
                .map(|v| v != 0)
                .unwrap_or(false),
        ),
    }
}

/// 出生点：探测 `spawn.pos`（IntArray [x,y,z]）→ 新结构；否则经典 `SpawnX/Y/Z`。
fn read_spawn_block(data: &NbtCompound) -> (i32, i32, i32) {
    match data.get("spawn") {
        Some(NbtValue::Compound(sp)) => match sp.get("pos") {
            Some(NbtValue::IntArray(pos)) if pos.len() >= 3 => (pos[0], pos[1], pos[2]),
            _ => (0, 0, 0),
        },
        _ => (
            get_int(data, "SpawnX").unwrap_or(0),
            get_int(data, "SpawnY").unwrap_or(0),
            get_int(data, "SpawnZ").unwrap_or(0),
        ),
    }
}

/// 读取精选游戏规则子集（未知规则忽略，缺失取默认值）
fn read_game_rules(data: &NbtCompound) -> LevelGameRules {
    let rules = match data.get("GameRules") {
        Some(NbtValue::Compound(rules)) => rules,
        _ => return default_game_rules(),
    };
    LevelGameRules {
        keep_inventory: rule_bool(rules, "keepInventory").unwrap_or(false),
        do_daylight_cycle: rule_bool(rules, "doDaylightCycle").unwrap_or(true),
        do_fire_tick: rule_bool(rules, "doFireTick").unwrap_or(true),
        mob_griefing: rule_bool(rules, "mobGriefing").unwrap_or(true),
        do_mob_spawning: rule_bool(rules, "doMobSpawning").unwrap_or(true),
        do_weather_cycle: rule_bool(rules, "doWeatherCycle").unwrap_or(true),
        do_mob_loot: rule_bool(rules, "doMobLoot").unwrap_or(true),
        do_tile_drops: rule_bool(rules, "doTileDrops").unwrap_or(true),
        do_entity_drops: rule_bool(rules, "doEntityDrops").unwrap_or(true),
        do_natural_regeneration: rule_bool(rules, "doNaturalRegeneration").unwrap_or(true),
        do_immediate_respawn: rule_bool(rules, "doImmediateRespawn").unwrap_or(false),
        do_insomnia: rule_bool(rules, "doInsomnia").unwrap_or(true),
        do_patrol_spawning: rule_bool(rules, "doPatrolSpawning").unwrap_or(true),
        do_trader_spawning: rule_bool(rules, "doTraderSpawning").unwrap_or(true),
        drowning_damage: rule_bool(rules, "drowningDamage").unwrap_or(true),
        fall_damage: rule_bool(rules, "fallDamage").unwrap_or(true),
        fire_damage: rule_bool(rules, "fireDamage").unwrap_or(true),
        freeze_damage: rule_bool(rules, "freezeDamage").unwrap_or(true),
        show_death_messages: rule_bool(rules, "showDeathMessages").unwrap_or(true),
        announce_advancements: rule_bool(rules, "announceAdvancements").unwrap_or(true),
        command_block_output: rule_bool(rules, "commandBlockOutput").unwrap_or(true),
        send_command_feedback: rule_bool(rules, "sendCommandFeedback").unwrap_or(true),
        reduced_debug_info: rule_bool(rules, "reducedDebugInfo").unwrap_or(false),
        disable_elytra_movement_check: rule_bool(rules, "disableElytraMovementCheck")
            .unwrap_or(false),
        spectators_generate_chunks: rule_bool(rules, "spectatorsGenerateChunks").unwrap_or(true),
        do_limited_crafting: rule_bool(rules, "doLimitedCrafting").unwrap_or(false),
        random_tick_speed: rule_int(rules, "randomTickSpeed").unwrap_or(3),
        spawn_radius: rule_int(rules, "spawnRadius").unwrap_or(10),
        max_entity_cramming: rule_int(rules, "maxEntityCramming").unwrap_or(24),
    }
}

/// 把精选字段写进 Data 复合（标准标签类型；未知键保留）。
/// 难度块/出生点按当前存档结构写：`difficulty_settings`/`spawn` 存在 → 新结构，
/// 否则经典键；另一结构的残留键不动（未知键保留）。
fn apply_settings(data: &mut NbtCompound, s: &LevelDatSettings) {
    data.insert(
        "LevelName".to_string(),
        NbtValue::String(s.level_name.clone()),
    );
    data.insert("GameType".to_string(), NbtValue::Int(s.game_type));
    apply_difficulty_block(data, s);
    data.insert(
        "allowCommands".to_string(),
        NbtValue::Byte(s.allow_commands as u8),
    );
    data.insert("Time".to_string(), NbtValue::Long(s.time));
    data.insert("DayTime".to_string(), NbtValue::Long(s.day_time));
    data.insert("raining".to_string(), NbtValue::Byte(s.raining as u8));
    data.insert("thundering".to_string(), NbtValue::Byte(s.thundering as u8));
    data.insert(
        "clearWeatherTime".to_string(),
        NbtValue::Int(s.clear_weather_time),
    );
    data.insert("rainTime".to_string(), NbtValue::Int(s.rain_time));
    data.insert("thunderTime".to_string(), NbtValue::Int(s.thunder_time));
    apply_spawn_block(data, s);
    data.insert("RandomSeed".to_string(), NbtValue::Long(s.random_seed));
    data.insert(
        "WanderingTraderSpawnChance".to_string(),
        NbtValue::Int(s.wandering_trader_spawn_chance),
    );
    data.insert(
        "WanderingTraderSpawnDelay".to_string(),
        NbtValue::Int(s.wandering_trader_spawn_delay),
    );
    data.insert(
        "BorderCenterX".to_string(),
        NbtValue::Double(s.border_center_x),
    );
    data.insert(
        "BorderCenterZ".to_string(),
        NbtValue::Double(s.border_center_z),
    );
    data.insert("BorderSize".to_string(), NbtValue::Double(s.border_size));
    data.insert(
        "BorderSafeZone".to_string(),
        NbtValue::Double(s.border_safe_zone),
    );
    data.insert(
        "BorderDamagePerBlock".to_string(),
        NbtValue::Double(s.border_damage_per_block),
    );
    data.insert(
        "BorderWarningBlocks".to_string(),
        NbtValue::Double(s.border_warning_blocks),
    );
    data.insert(
        "BorderWarningTime".to_string(),
        NbtValue::Double(s.border_warning_time),
    );
    // GameRules：已知规则写回，未知规则保留
    if !matches!(data.get("GameRules"), Some(NbtValue::Compound(_))) {
        data.insert(
            "GameRules".to_string(),
            NbtValue::Compound(NbtCompound::new()),
        );
    }
    if let Some(NbtValue::Compound(rules)) = data.get_mut("GameRules") {
        apply_game_rules(rules, &s.game_rules);
    }
}

/// 难度/硬核/锁定写入：Data 含 `difficulty_settings` → 新结构；否则经典键。
fn apply_difficulty_block(data: &mut NbtCompound, s: &LevelDatSettings) {
    if matches!(data.get("difficulty_settings"), Some(NbtValue::Compound(_))) {
        if let Some(NbtValue::Compound(ds)) = data.get_mut("difficulty_settings") {
            ds.insert(
                "difficulty".to_string(),
                NbtValue::String(difficulty_string(s.difficulty)),
            );
            ds.insert("hardcore".to_string(), NbtValue::Byte(s.hardcore as u8));
            ds.insert(
                "locked".to_string(),
                NbtValue::Byte(s.difficulty_locked as u8),
            );
        }
    } else {
        data.insert("Difficulty".to_string(), NbtValue::Byte(s.difficulty));
        data.insert("hardcore".to_string(), NbtValue::Byte(s.hardcore as u8));
        data.insert(
            "DifficultyLocked".to_string(),
            NbtValue::Byte(s.difficulty_locked as u8),
        );
    }
}

/// 出生点写入：Data 含 `spawn` → `spawn.pos` IntArray；否则经典 `SpawnX/Y/Z`。
fn apply_spawn_block(data: &mut NbtCompound, s: &LevelDatSettings) {
    if matches!(data.get("spawn"), Some(NbtValue::Compound(_))) {
        if let Some(NbtValue::Compound(sp)) = data.get_mut("spawn") {
            sp.insert(
                "pos".to_string(),
                NbtValue::IntArray(vec![s.spawn_x, s.spawn_y, s.spawn_z]),
            );
        }
    } else {
        data.insert("SpawnX".to_string(), NbtValue::Int(s.spawn_x));
        data.insert("SpawnY".to_string(), NbtValue::Int(s.spawn_y));
        data.insert("SpawnZ".to_string(), NbtValue::Int(s.spawn_z));
    }
}

/// 游戏规则写入（布尔 → "true"/"false"，数值 → 十进制字符串）
fn apply_game_rules(rules: &mut NbtCompound, g: &LevelGameRules) {
    rules.insert(
        "keepInventory".to_string(),
        NbtValue::String(bool_str(g.keep_inventory)),
    );
    rules.insert(
        "doDaylightCycle".to_string(),
        NbtValue::String(bool_str(g.do_daylight_cycle)),
    );
    rules.insert(
        "doFireTick".to_string(),
        NbtValue::String(bool_str(g.do_fire_tick)),
    );
    rules.insert(
        "mobGriefing".to_string(),
        NbtValue::String(bool_str(g.mob_griefing)),
    );
    rules.insert(
        "doMobSpawning".to_string(),
        NbtValue::String(bool_str(g.do_mob_spawning)),
    );
    rules.insert(
        "doWeatherCycle".to_string(),
        NbtValue::String(bool_str(g.do_weather_cycle)),
    );
    rules.insert(
        "doMobLoot".to_string(),
        NbtValue::String(bool_str(g.do_mob_loot)),
    );
    rules.insert(
        "doTileDrops".to_string(),
        NbtValue::String(bool_str(g.do_tile_drops)),
    );
    rules.insert(
        "doEntityDrops".to_string(),
        NbtValue::String(bool_str(g.do_entity_drops)),
    );
    rules.insert(
        "doNaturalRegeneration".to_string(),
        NbtValue::String(bool_str(g.do_natural_regeneration)),
    );
    rules.insert(
        "doImmediateRespawn".to_string(),
        NbtValue::String(bool_str(g.do_immediate_respawn)),
    );
    rules.insert(
        "doInsomnia".to_string(),
        NbtValue::String(bool_str(g.do_insomnia)),
    );
    rules.insert(
        "doPatrolSpawning".to_string(),
        NbtValue::String(bool_str(g.do_patrol_spawning)),
    );
    rules.insert(
        "doTraderSpawning".to_string(),
        NbtValue::String(bool_str(g.do_trader_spawning)),
    );
    rules.insert(
        "drowningDamage".to_string(),
        NbtValue::String(bool_str(g.drowning_damage)),
    );
    rules.insert(
        "fallDamage".to_string(),
        NbtValue::String(bool_str(g.fall_damage)),
    );
    rules.insert(
        "fireDamage".to_string(),
        NbtValue::String(bool_str(g.fire_damage)),
    );
    rules.insert(
        "freezeDamage".to_string(),
        NbtValue::String(bool_str(g.freeze_damage)),
    );
    rules.insert(
        "showDeathMessages".to_string(),
        NbtValue::String(bool_str(g.show_death_messages)),
    );
    rules.insert(
        "announceAdvancements".to_string(),
        NbtValue::String(bool_str(g.announce_advancements)),
    );
    rules.insert(
        "commandBlockOutput".to_string(),
        NbtValue::String(bool_str(g.command_block_output)),
    );
    rules.insert(
        "sendCommandFeedback".to_string(),
        NbtValue::String(bool_str(g.send_command_feedback)),
    );
    rules.insert(
        "reducedDebugInfo".to_string(),
        NbtValue::String(bool_str(g.reduced_debug_info)),
    );
    rules.insert(
        "disableElytraMovementCheck".to_string(),
        NbtValue::String(bool_str(g.disable_elytra_movement_check)),
    );
    rules.insert(
        "spectatorsGenerateChunks".to_string(),
        NbtValue::String(bool_str(g.spectators_generate_chunks)),
    );
    rules.insert(
        "doLimitedCrafting".to_string(),
        NbtValue::String(bool_str(g.do_limited_crafting)),
    );
    rules.insert(
        "randomTickSpeed".to_string(),
        NbtValue::String(g.random_tick_speed.to_string()),
    );
    rules.insert(
        "spawnRadius".to_string(),
        NbtValue::String(g.spawn_radius.to_string()),
    );
    rules.insert(
        "maxEntityCramming".to_string(),
        NbtValue::String(g.max_entity_cramming.to_string()),
    );
}

/// 写前快照当前 level.dat → `level.dat.qomicex.bak`（覆盖式：保留"最近一次写前"状态，
/// 用户总能回退到上一次写入前的设置）。备份失败 → Err（拒绝写入）。
fn backup_current(level_dat_path: &Path, original_bytes: &[u8]) -> Result<(), Error> {
    let bak = level_dat_path.with_file_name(BACKUP_FILE);
    std::fs::write(&bak, original_bytes).map_err(|e| io_err(&bak, "备份", e))
}

/// 读 level.dat 原始字节并解析（文件缺失 → Err）
fn read_root(save_directory: &Path) -> Result<NbtCompound, Error> {
    let level_dat_path = save_directory.join("level.dat");
    if !level_dat_path.is_file() {
        return Err(Error::Params {
            message: format!(
                "level.dat not found in save directory: {}",
                save_directory.display()
            ),
            source: None,
        });
    }
    let bytes = std::fs::read(&level_dat_path).map_err(|e| io_err(&level_dat_path, "读取", e))?;
    nbt_full::read(&bytes).map_err(parse_err)
}

fn get_string(data: &NbtCompound, name: &str) -> Option<String> {
    match data.get(name) {
        Some(NbtValue::String(v)) => Some(v.clone()),
        _ => None,
    }
}

fn get_int(data: &NbtCompound, name: &str) -> Option<i32> {
    match data.get(name) {
        Some(NbtValue::Int(v)) => Some(*v),
        _ => None,
    }
}

fn get_long(data: &NbtCompound, name: &str) -> Option<i64> {
    match data.get(name) {
        Some(NbtValue::Long(v)) => Some(*v),
        _ => None,
    }
}

fn get_byte(data: &NbtCompound, name: &str) -> Option<u8> {
    match data.get(name) {
        Some(NbtValue::Byte(v)) => Some(*v),
        _ => None,
    }
}

fn get_double(data: &NbtCompound, name: &str) -> Option<f64> {
    match data.get(name) {
        Some(NbtValue::Double(v)) => Some(*v),
        _ => None,
    }
}

/// difficulty_settings.difficulty（String）→ 0-3
fn difficulty_from_string(s: Option<&str>) -> u8 {
    match s {
        Some("peaceful") => 0,
        Some("easy") => 1,
        Some("hard") => 3,
        _ => 2, // normal / 未知 → 普通
    }
}

/// 0-3 → difficulty_settings.difficulty（String）
fn difficulty_string(d: u8) -> String {
    match d {
        0 => "peaceful".to_string(),
        1 => "easy".to_string(),
        3 => "hard".to_string(),
        _ => "normal".to_string(),
    }
}

/// GameRules 布尔值（String "true"/"false"）
fn rule_bool(rules: &NbtCompound, name: &str) -> Option<bool> {
    match rules.get(name) {
        Some(NbtValue::String(v)) => Some(v == "true"),
        _ => None,
    }
}

/// GameRules 数值（String 十进制数字；解析失败 → None 取默认）
fn rule_int(rules: &NbtCompound, name: &str) -> Option<i32> {
    match rules.get(name) {
        Some(NbtValue::String(v)) => v.parse::<i32>().ok(),
        _ => None,
    }
}

fn bool_str(b: bool) -> String {
    if b {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

fn io_err(path: &Path, op: &str, e: std::io::Error) -> Error {
    Error::Params {
        message: format!("{op} {} 失败: {e}", path.display()),
        source: None,
    }
}

fn parse_err(e: nbt_full::NbtError) -> Error {
    Error::Params {
        message: format!("level.dat NBT 解析失败: {e}"),
        source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::nbt_full as nbt;

    fn temp_save_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("qomicex_level_dat_test_{}", std::process::id()))
            .join(name);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 构造一份含全字段 + 未知键（BorderCenterX / unknownRule）的经典格式 gzip level.dat
    fn make_level_dat() -> Vec<u8> {
        let mut data = NbtCompound::new();
        data.insert(
            "LevelName".to_string(),
            NbtValue::String("Test World".to_string()),
        );
        data.insert("GameType".to_string(), NbtValue::Int(1));
        data.insert("Difficulty".to_string(), NbtValue::Byte(3));
        data.insert("allowCommands".to_string(), NbtValue::Byte(1));
        data.insert("Time".to_string(), NbtValue::Long(123_456));
        data.insert("DayTime".to_string(), NbtValue::Long(6_000));
        data.insert("SpawnX".to_string(), NbtValue::Int(10));
        data.insert("SpawnY".to_string(), NbtValue::Int(64));
        data.insert("SpawnZ".to_string(), NbtValue::Int(-20));
        data.insert("RandomSeed".to_string(), NbtValue::Long(987_654_321));
        data.insert("clearWeatherTime".to_string(), NbtValue::Int(1200));
        data.insert("WanderingTraderSpawnChance".to_string(), NbtValue::Int(30));
        data.insert("BorderSize".to_string(), NbtValue::Double(10_000.0));
        // 未知键：写回后必须原样保留
        data.insert("BorderCenterX".to_string(), NbtValue::Double(0.0));
        let mut rules = NbtCompound::new();
        rules.insert(
            "keepInventory".to_string(),
            NbtValue::String("true".to_string()),
        );
        rules.insert(
            "unknownRule".to_string(),
            NbtValue::String("false".to_string()),
        );
        data.insert("GameRules".to_string(), NbtValue::Compound(rules));
        let mut root = NbtCompound::new();
        root.insert("Data".to_string(), NbtValue::Compound(data));
        nbt::write_gzip(&root).expect("构造测试 level.dat 失败")
    }

    /// 构造过渡格式（difficulty_settings + spawn.pos）的 gzip level.dat
    fn make_level_dat_v2() -> Vec<u8> {
        let mut data = NbtCompound::new();
        data.insert(
            "LevelName".to_string(),
            NbtValue::String("V2 World".to_string()),
        );
        data.insert("GameType".to_string(), NbtValue::Int(1));
        data.insert("allowCommands".to_string(), NbtValue::Byte(1));
        data.insert("Time".to_string(), NbtValue::Long(10_000));
        let mut ds = NbtCompound::new();
        ds.insert(
            "difficulty".to_string(),
            NbtValue::String("hard".to_string()),
        );
        ds.insert("hardcore".to_string(), NbtValue::Byte(1));
        ds.insert("locked".to_string(), NbtValue::Byte(1));
        data.insert("difficulty_settings".to_string(), NbtValue::Compound(ds));
        let mut spawn = NbtCompound::new();
        spawn.insert(
            "dimension".to_string(),
            NbtValue::String("minecraft:overworld".to_string()),
        );
        spawn.insert("pos".to_string(), NbtValue::IntArray(vec![100, 65, -200]));
        spawn.insert("pitch".to_string(), NbtValue::Float(0.0));
        data.insert("spawn".to_string(), NbtValue::Compound(spawn));
        // 残留经典键（模拟格式迁移后未清理）：写回时必须保留（未知键保留）
        data.insert("Difficulty".to_string(), NbtValue::Byte(1));
        data.insert("SpawnX".to_string(), NbtValue::Int(999));
        let mut root = NbtCompound::new();
        root.insert("Data".to_string(), NbtValue::Compound(data));
        nbt::write_gzip(&root).expect("构造测试 level.dat 失败")
    }

    #[test]
    fn read_settings_full_roundtrip() {
        let dir = temp_save_dir("roundtrip");
        std::fs::write(dir.join("level.dat"), make_level_dat()).unwrap();
        let s = read_settings(&dir).expect("读取存档设置失败");
        assert_eq!(s.level_name, "Test World");
        assert_eq!(s.game_type, 1);
        assert_eq!(s.difficulty, 3);
        assert!(s.allow_commands);
        assert_eq!(s.time, 123_456);
        assert_eq!(s.day_time, 6_000);
        assert_eq!((s.spawn_x, s.spawn_y, s.spawn_z), (10, 64, -20));
        assert_eq!(s.random_seed, 987_654_321);
        assert_eq!(s.clear_weather_time, 1200);
        assert_eq!(s.wandering_trader_spawn_chance, 30);
        assert_eq!(s.border_size, 10_000.0);
        assert!(s.game_rules.keep_inventory);
        assert!(s.game_rules.do_daylight_cycle);
        assert!(s.game_rules.do_mob_loot);
        assert_eq!(s.game_rules.random_tick_speed, 3);
        cleanup(&dir);
    }

    #[test]
    fn read_defaults_for_missing_fields() {
        let dir = temp_save_dir("defaults");
        let mut data = NbtCompound::new();
        data.insert(
            "LevelName".to_string(),
            NbtValue::String("Only Name".to_string()),
        );
        let mut root = NbtCompound::new();
        root.insert("Data".to_string(), NbtValue::Compound(data));
        std::fs::write(dir.join("level.dat"), nbt::write_gzip(&root).unwrap()).unwrap();
        let s = read_settings(&dir).expect("读取存档设置失败");
        assert_eq!(s.level_name, "Only Name");
        assert_eq!(s.game_type, 0);
        assert_eq!(s.difficulty, 2); // 普通
        assert!(!s.allow_commands);
        assert!(!s.difficulty_locked);
        assert_eq!(s.clear_weather_time, 0);
        assert_eq!(s.wandering_trader_spawn_chance, 25);
        assert_eq!(s.wandering_trader_spawn_delay, 2400);
        assert_eq!(s.border_size, 60_000_000.0);
        assert!(!s.game_rules.keep_inventory);
        assert!(s.game_rules.do_weather_cycle);
        assert!(s.game_rules.do_mob_loot);
        assert_eq!(s.game_rules.random_tick_speed, 3);
        assert_eq!(s.game_rules.spawn_radius, 10);
        cleanup(&dir);
    }

    #[test]
    fn update_preserves_unknown_and_backs_up() {
        let dir = temp_save_dir("update");
        let original = make_level_dat();
        std::fs::write(dir.join("level.dat"), &original).unwrap();
        let mut s = read_settings(&dir).unwrap();
        s.level_name = "Renamed".to_string();
        s.game_type = 2;
        s.allow_commands = false;
        s.hardcore = true;
        s.difficulty_locked = true;
        s.raining = true;
        s.clear_weather_time = 600;
        s.border_size = 20_000.0;
        s.game_rules.keep_inventory = false;
        s.game_rules.mob_griefing = false;
        s.game_rules.do_natural_regeneration = false;
        s.game_rules.random_tick_speed = 0;
        update_settings(&dir, &s).expect("更新存档设置失败");

        // 安全备份存在且等于原始字节
        let bak = std::fs::read(dir.join("level.dat.qomicex.bak")).unwrap();
        assert_eq!(bak, original);

        // 重新读取：新值生效
        let s2 = read_settings(&dir).unwrap();
        assert_eq!(s2.level_name, "Renamed");
        assert_eq!(s2.game_type, 2);
        assert!(!s2.allow_commands);
        assert!(s2.hardcore);
        assert!(s2.difficulty_locked);
        assert!(s2.raining);
        assert_eq!(s2.clear_weather_time, 600);
        assert_eq!(s2.border_size, 20_000.0);
        assert!(!s2.game_rules.keep_inventory);
        assert!(!s2.game_rules.mob_griefing);
        assert!(!s2.game_rules.do_natural_regeneration);
        assert_eq!(s2.game_rules.random_tick_speed, 0);

        // 未知键保留（BorderCenterX Double / unknownRule String）
        let bytes = std::fs::read(dir.join("level.dat")).unwrap();
        let root = nbt::read(&bytes).expect("写回后解析失败");
        let data = match root.get("Data") {
            Some(NbtValue::Compound(d)) => d,
            _ => panic!("Data 复合缺失"),
        };
        assert!(matches!(
            data.get("BorderCenterX"),
            Some(NbtValue::Double(0.0))
        ));
        let rules = match data.get("GameRules") {
            Some(NbtValue::Compound(r)) => r,
            _ => panic!("GameRules 复合缺失"),
        };
        assert!(matches!(rules.get("unknownRule"), Some(NbtValue::String(v)) if v == "false"));
        cleanup(&dir);
    }

    #[test]
    fn v2_format_difficulty_and_spawn_roundtrip() {
        let dir = temp_save_dir("v2");
        std::fs::write(dir.join("level.dat"), make_level_dat_v2()).unwrap();
        let s = read_settings(&dir).expect("读取存档设置失败");
        // 新结构优先：难度 hard=3、硬核、锁定；出生点 pos
        assert_eq!(s.difficulty, 3);
        assert!(s.hardcore);
        assert!(s.difficulty_locked);
        assert_eq!((s.spawn_x, s.spawn_y, s.spawn_z), (100, 65, -200));

        // 更新后写回新结构，残留经典键保留
        let mut s2 = s.clone();
        s2.difficulty = 0;
        s2.hardcore = false;
        s2.difficulty_locked = false;
        s2.spawn_x = 1;
        s2.spawn_y = 2;
        s2.spawn_z = 3;
        update_settings(&dir, &s2).expect("更新存档设置失败");

        let bytes = std::fs::read(dir.join("level.dat")).unwrap();
        let root = nbt::read(&bytes).unwrap();
        let data = match root.get("Data") {
            Some(NbtValue::Compound(d)) => d,
            _ => panic!("Data 复合缺失"),
        };
        let ds = match data.get("difficulty_settings") {
            Some(NbtValue::Compound(ds)) => ds,
            _ => panic!("difficulty_settings 缺失"),
        };
        assert!(matches!(ds.get("difficulty"), Some(NbtValue::String(v)) if v == "peaceful"));
        assert!(matches!(ds.get("hardcore"), Some(NbtValue::Byte(0))));
        assert!(matches!(ds.get("locked"), Some(NbtValue::Byte(0))));
        let spawn = match data.get("spawn") {
            Some(NbtValue::Compound(sp)) => sp,
            _ => panic!("spawn 缺失"),
        };
        assert!(matches!(spawn.get("pos"), Some(NbtValue::IntArray(v)) if v == &[1, 2, 3]));
        // 残留经典键保留（未知键保留原则）
        assert!(matches!(data.get("Difficulty"), Some(NbtValue::Byte(1))));
        assert!(matches!(data.get("SpawnX"), Some(NbtValue::Int(999))));

        // 重新读取确认新值
        let s3 = read_settings(&dir).unwrap();
        assert_eq!(s3.difficulty, 0);
        assert!(!s3.hardcore);
        assert!(!s3.difficulty_locked);
        assert_eq!((s3.spawn_x, s3.spawn_y, s3.spawn_z), (1, 2, 3));
        cleanup(&dir);
    }

    #[test]
    fn missing_level_dat_errors() {
        let dir = temp_save_dir("missing");
        let err = read_settings(&dir).expect_err("缺失 level.dat 应报错");
        assert!(err.to_string().contains("level.dat not found"));
        // update 走 IO 错误路径（读取失败），只断言报错即可
        assert!(update_settings(&dir, &default_settings()).is_err());
        cleanup(&dir);
    }

    #[test]
    fn corrupt_level_dat_errors() {
        let dir = temp_save_dir("corrupt");
        std::fs::write(dir.join("level.dat"), b"this is not nbt at all").unwrap();
        let err = read_settings(&dir).expect_err("损坏 level.dat 应报错");
        assert!(err.to_string().contains("解析失败"));
        cleanup(&dir);
    }

    #[test]
    fn restore_from_old_works() {
        let dir = temp_save_dir("restore");
        let current = make_level_dat();
        std::fs::write(dir.join("level.dat"), &current).unwrap();
        // 构造一份不同的 _old（模拟上一会话）
        let mut data = NbtCompound::new();
        data.insert(
            "LevelName".to_string(),
            NbtValue::String("Old World".to_string()),
        );
        data.insert("GameType".to_string(), NbtValue::Int(0));
        let mut root = NbtCompound::new();
        root.insert("Data".to_string(), NbtValue::Compound(data));
        let old_bytes = nbt::write_gzip(&root).unwrap();
        std::fs::write(dir.join("level.dat_old"), &old_bytes).unwrap();

        restore_from_old(&dir).expect("恢复失败");
        // 恢复后 level.dat == _old 内容，且当前内容已备份
        assert_eq!(std::fs::read(dir.join("level.dat")).unwrap(), old_bytes);
        assert_eq!(
            std::fs::read(dir.join("level.dat.qomicex.bak")).unwrap(),
            current
        );
        let s = read_settings(&dir).unwrap();
        assert_eq!(s.level_name, "Old World");
        cleanup(&dir);
    }

    #[test]
    fn restore_missing_old_errors() {
        let dir = temp_save_dir("restore_missing_old");
        std::fs::write(dir.join("level.dat"), make_level_dat()).unwrap();
        let err = restore_from_old(&dir).expect_err("缺失 _old 应报错");
        assert!(err.to_string().contains("level.dat_old not found"));
        cleanup(&dir);
    }
}
