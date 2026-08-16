//! 存档设置（level.dat NBT）读写服务。
//!
//! 对应「存档设置管理」功能（前端实例管理 → 存档 tab → 每存档「设置」弹窗）。
//! 精选白名单字段（`LevelDatSettings`，见 models/expansion/local.rs）：名称 / 游戏模式 /
//! 难度 / 作弊 / 硬核 / 世界时间 / 昼夜时间 / 天气 / 出生点 / 种子 + 6 项游戏规则。
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
    Ok(LevelDatSettings {
        level_name: get_string(data, "LevelName").unwrap_or_default(),
        game_type: get_int(data, "GameType").unwrap_or(0),
        difficulty: get_byte(data, "Difficulty").unwrap_or(2),
        allow_commands: get_byte(data, "allowCommands").map(|v| v != 0).unwrap_or(false),
        hardcore: get_byte(data, "hardcore").map(|v| v != 0).unwrap_or(false),
        time: get_long(data, "Time").unwrap_or(0),
        day_time: get_long(data, "DayTime").unwrap_or(0),
        raining: get_byte(data, "raining").map(|v| v != 0).unwrap_or(false),
        thundering: get_byte(data, "thundering").map(|v| v != 0).unwrap_or(false),
        spawn_x: get_int(data, "SpawnX").unwrap_or(0),
        spawn_y: get_int(data, "SpawnY").unwrap_or(0),
        spawn_z: get_int(data, "SpawnZ").unwrap_or(0),
        random_seed: get_long(data, "RandomSeed").unwrap_or(0),
        game_rules: read_game_rules(data),
    })
}

/// 更新存档设置（写前备份 + 失败回滚；未知键保留）。
pub(crate) fn update_settings(
    save_directory: &Path,
    settings: &LevelDatSettings,
) -> Result<(), Error> {
    let level_dat_path = save_directory.join("level.dat");
    let original_bytes = std::fs::read(&level_dat_path)
        .map_err(|e| io_err(&level_dat_path, "读取", e))?;

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
                })
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
        let original_bytes = std::fs::read(&level_dat_path)
            .map_err(|e| io_err(&level_dat_path, "读取", e))?;
        backup_current(&level_dat_path, &original_bytes)?;
    }
    std::fs::write(&level_dat_path, &old_bytes)
        .map_err(|e| io_err(&level_dat_path, "写入", e))?;
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
        time: 0,
        day_time: 0,
        raining: false,
        thundering: false,
        spawn_x: 0,
        spawn_y: 0,
        spawn_z: 0,
        random_seed: 0,
        game_rules: LevelGameRules {
            keep_inventory: false,
            do_daylight_cycle: true,
            do_fire_tick: true,
            mob_griefing: true,
            do_mob_spawning: true,
            do_weather_cycle: true,
        },
    }
}

/// 读取精选游戏规则子集（未知规则忽略，缺失取默认值）
fn read_game_rules(data: &NbtCompound) -> LevelGameRules {
    let rules = match data.get("GameRules") {
        Some(NbtValue::Compound(rules)) => rules,
        _ => return default_settings().game_rules,
    };
    LevelGameRules {
        keep_inventory: rule_bool(rules, "keepInventory").unwrap_or(false),
        do_daylight_cycle: rule_bool(rules, "doDaylightCycle").unwrap_or(true),
        do_fire_tick: rule_bool(rules, "doFireTick").unwrap_or(true),
        mob_griefing: rule_bool(rules, "mobGriefing").unwrap_or(true),
        do_mob_spawning: rule_bool(rules, "doMobSpawning").unwrap_or(true),
        do_weather_cycle: rule_bool(rules, "doWeatherCycle").unwrap_or(true),
    }
}

/// 把精选字段写进 Data 复合（标准标签类型；未知键保留）
fn apply_settings(data: &mut NbtCompound, s: &LevelDatSettings) {
    data.insert("LevelName".to_string(), NbtValue::String(s.level_name.clone()));
    data.insert("GameType".to_string(), NbtValue::Int(s.game_type));
    data.insert("Difficulty".to_string(), NbtValue::Byte(s.difficulty));
    data.insert("allowCommands".to_string(), NbtValue::Byte(s.allow_commands as u8));
    data.insert("hardcore".to_string(), NbtValue::Byte(s.hardcore as u8));
    data.insert("Time".to_string(), NbtValue::Long(s.time));
    data.insert("DayTime".to_string(), NbtValue::Long(s.day_time));
    data.insert("raining".to_string(), NbtValue::Byte(s.raining as u8));
    data.insert("thundering".to_string(), NbtValue::Byte(s.thundering as u8));
    data.insert("SpawnX".to_string(), NbtValue::Int(s.spawn_x));
    data.insert("SpawnY".to_string(), NbtValue::Int(s.spawn_y));
    data.insert("SpawnZ".to_string(), NbtValue::Int(s.spawn_z));
    data.insert("RandomSeed".to_string(), NbtValue::Long(s.random_seed));
    // GameRules：已知规则写回，未知规则保留
    if !matches!(data.get("GameRules"), Some(NbtValue::Compound(_))) {
        data.insert("GameRules".to_string(), NbtValue::Compound(NbtCompound::new()));
    }
    if let Some(NbtValue::Compound(rules)) = data.get_mut("GameRules") {
        rules.insert("keepInventory".to_string(), NbtValue::String(bool_str(s.game_rules.keep_inventory)));
        rules.insert("doDaylightCycle".to_string(), NbtValue::String(bool_str(s.game_rules.do_daylight_cycle)));
        rules.insert("doFireTick".to_string(), NbtValue::String(bool_str(s.game_rules.do_fire_tick)));
        rules.insert("mobGriefing".to_string(), NbtValue::String(bool_str(s.game_rules.mob_griefing)));
        rules.insert("doMobSpawning".to_string(), NbtValue::String(bool_str(s.game_rules.do_mob_spawning)));
        rules.insert("doWeatherCycle".to_string(), NbtValue::String(bool_str(s.game_rules.do_weather_cycle)));
    }
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

/// GameRules 值（String "true"/"false"）
fn rule_bool(rules: &NbtCompound, name: &str) -> Option<bool> {
    match rules.get(name) {
        Some(NbtValue::String(v)) => Some(v == "true"),
        _ => None,
    }
}

fn bool_str(b: bool) -> String {
    if b { "true".to_string() } else { "false".to_string() }
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

    /// 构造一份含全字段 + 未知键（BorderCenterX / unknownRule）的 gzip level.dat
    fn make_level_dat() -> Vec<u8> {
        let mut data = NbtCompound::new();
        data.insert("LevelName".to_string(), NbtValue::String("Test World".to_string()));
        data.insert("GameType".to_string(), NbtValue::Int(1));
        data.insert("Difficulty".to_string(), NbtValue::Byte(3));
        data.insert("allowCommands".to_string(), NbtValue::Byte(1));
        data.insert("Time".to_string(), NbtValue::Long(123_456));
        data.insert("DayTime".to_string(), NbtValue::Long(6_000));
        data.insert("SpawnX".to_string(), NbtValue::Int(10));
        data.insert("SpawnY".to_string(), NbtValue::Int(64));
        data.insert("SpawnZ".to_string(), NbtValue::Int(-20));
        data.insert("RandomSeed".to_string(), NbtValue::Long(987_654_321));
        // 未知键：写回后必须原样保留
        data.insert("BorderCenterX".to_string(), NbtValue::Double(0.0));
        let mut rules = NbtCompound::new();
        rules.insert("keepInventory".to_string(), NbtValue::String("true".to_string()));
        rules.insert("unknownRule".to_string(), NbtValue::String("false".to_string()));
        data.insert("GameRules".to_string(), NbtValue::Compound(rules));
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
        assert!(s.game_rules.keep_inventory);
        assert!(s.game_rules.do_daylight_cycle);
        cleanup(&dir);
    }

    #[test]
    fn read_defaults_for_missing_fields() {
        let dir = temp_save_dir("defaults");
        let mut data = NbtCompound::new();
        data.insert("LevelName".to_string(), NbtValue::String("Only Name".to_string()));
        let mut root = NbtCompound::new();
        root.insert("Data".to_string(), NbtValue::Compound(data));
        std::fs::write(dir.join("level.dat"), nbt::write_gzip(&root).unwrap()).unwrap();
        let s = read_settings(&dir).expect("读取存档设置失败");
        assert_eq!(s.level_name, "Only Name");
        assert_eq!(s.game_type, 0);
        assert_eq!(s.difficulty, 2); // 普通
        assert!(!s.allow_commands);
        assert!(!s.game_rules.keep_inventory);
        assert!(s.game_rules.do_weather_cycle);
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
        s.raining = true;
        s.game_rules.keep_inventory = false;
        s.game_rules.mob_griefing = false;
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
        assert!(s2.raining);
        assert!(!s2.game_rules.keep_inventory);
        assert!(!s2.game_rules.mob_griefing);

        // 未知键保留（BorderCenterX Double / unknownRule String）
        let bytes = std::fs::read(dir.join("level.dat")).unwrap();
        let root = nbt::read(&bytes).expect("写回后解析失败");
        let data = match root.get("Data") {
            Some(NbtValue::Compound(d)) => d,
            _ => panic!("Data 复合缺失"),
        };
        assert!(matches!(data.get("BorderCenterX"), Some(NbtValue::Double(0.0))));
        let rules = match data.get("GameRules") {
            Some(NbtValue::Compound(r)) => r,
            _ => panic!("GameRules 复合缺失"),
        };
        assert!(matches!(rules.get("unknownRule"), Some(NbtValue::String(v)) if v == "false"));
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
        data.insert("LevelName".to_string(), NbtValue::String("Old World".to_string()));
        data.insert("GameType".to_string(), NbtValue::Int(0));
        let mut root = NbtCompound::new();
        root.insert("Data".to_string(), NbtValue::Compound(data));
        let old_bytes = nbt::write_gzip(&root).unwrap();
        std::fs::write(dir.join("level.dat_old"), &old_bytes).unwrap();

        restore_from_old(&dir).expect("恢复失败");
        // 恢复后 level.dat == _old 内容，且当前内容已备份
        assert_eq!(std::fs::read(dir.join("level.dat")).unwrap(), old_bytes);
        assert_eq!(std::fs::read(dir.join("level.dat.qomicex.bak")).unwrap(), current);
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
