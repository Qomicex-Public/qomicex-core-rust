//! B1 批次验证：模型层 serde 行为与源项目 JsonContext 语义对齐
//! 覆盖：VersionArguments 新旧格式兼容、字符串枚举（Modrinth）、数字枚举（serde_repr）、常规往返

use qomicex_core_rust::models::download::{DownloadMirror, DownloadStatus, ResourceType};
use qomicex_core_rust::models::expansion::curseforge::CurseForgeFileInfo;
use qomicex_core_rust::models::expansion::modrinth::ModLoaderType;
use qomicex_core_rust::models::version_manifest::{LatestVersionInfo, ManifestVersionInfo, VersionManifestRoot};
use qomicex_core_rust::models::version_metadata::{ArgumentItem, VersionArguments};

// ── 特殊兼容：VersionArguments 新旧格式 ──────────────

#[test]
fn version_arguments_old_string_form() {
    let json = r#""--username ${auth_player_name} --version ${version_name}""#;
    let parsed: VersionArguments = serde_json::from_str(json).unwrap();
    assert_eq!(
        parsed,
        VersionArguments::Old("--username ${auth_player_name} --version ${version_name}".to_string())
    );
}

#[test]
fn version_arguments_new_form_with_rules() {
    let json = r#"{
        "game": ["--username", {"value": ["--version"], "rules": [{"action": "allow"}]}],
        "jvm": ["-Xmx2G"]
    }"#;
    let parsed: VersionArguments = serde_json::from_str(json).unwrap();
    match parsed {
        VersionArguments::New { game, jvm } => {
            assert_eq!(game.len(), 2);
            assert_eq!(game[0], ArgumentItem::String("--username".to_string()));
            match &game[1] {
                ArgumentItem::Object { value, rules } => {
                    assert_eq!(value, &vec!["--version".to_string()]);
                    assert_eq!(rules.len(), 1);
                }
                _ => panic!("expected Object"),
            }
            assert_eq!(jvm, vec![ArgumentItem::String("-Xmx2G".to_string())]);
        }
        _ => panic!("expected New form"),
    }
}

#[test]
fn version_arguments_missing_jvm_defaults_empty() {
    let json = r#"{"game": ["--demo"]}"#;
    let parsed: VersionArguments = serde_json::from_str(json).unwrap();
    match parsed {
        VersionArguments::New { game, jvm } => {
            assert_eq!(game, vec![ArgumentItem::String("--demo".to_string())]);
            assert!(jvm.is_empty());
        }
        _ => panic!("expected New form"),
    }
}

#[test]
fn version_arguments_serialize_new_form() {
    let args = VersionArguments::New {
        game: vec![ArgumentItem::String("--username".to_string())],
        jvm: vec![ArgumentItem::Object {
            value: vec!["-Xmx2G".to_string()],
            rules: vec![],
        }],
    };
    let out = serde_json::to_value(&args).unwrap();
    assert_eq!(out["game"], serde_json::json!(["--username"]));
    // 单值 Object 压缩为字符串；空 rules 省略键
    assert_eq!(out["jvm"], serde_json::json!([{"value": "-Xmx2G"}]));
    assert!(!out["jvm"][0].as_object().unwrap().contains_key("rules"));
}

#[test]
fn version_arguments_serialize_old_form_errors() {
    let args = VersionArguments::Old("legacy".to_string());
    assert!(serde_json::to_value(&args).is_err());
}

// ── 字符串枚举（ModrinthJsonContext UseStringEnumConverter）──

#[test]
fn modrinth_mod_loader_type_string_values() {
    assert_eq!(serde_json::to_string(&ModLoaderType::NeoForge).unwrap(), "\"neoForge\"");
    assert_eq!(serde_json::to_string(&ModLoaderType::LiteLoader).unwrap(), "\"liteLoader\"");
    assert_eq!(serde_json::to_string(&ModLoaderType::Minecraft).unwrap(), "\"minecraft\"");
    let parsed: ModLoaderType = serde_json::from_str("\"fabric\"").unwrap();
    assert_eq!(parsed, ModLoaderType::Fabric);
}

// ── 数字枚举（serde_repr，源 context 默认数字序列化）──

#[test]
fn numeric_enums_serialize_as_numbers() {
    assert_eq!(serde_json::to_string(&DownloadStatus::Failed).unwrap(), "3");
    assert_eq!(serde_json::to_string(&ResourceType::Asset).unwrap(), "1");
    assert_eq!(serde_json::to_string(&DownloadMirror::Bmclapi).unwrap(), "1");
    let parsed: DownloadStatus = serde_json::from_str("4").unwrap();
    assert_eq!(parsed, DownloadStatus::Retrying);
}

// ── 常规模型往返 ─────────────────────────────────

#[test]
fn version_manifest_roundtrip() {
    let json = r#"{
        "latest": {"release": "1.21", "snapshot": "24w14a"},
        "versions": [
            {"id": "1.21", "type": "release", "url": "https://x/v1.21.json", "time": "2024-06-13T11:07:26Z", "releaseTime": "2024-06-13T11:07:26Z"}
        ]
    }"#;
    let root: VersionManifestRoot = serde_json::from_str(json).unwrap();
    assert_eq!(root.latest.release, "1.21");
    assert_eq!(root.versions.len(), 1);
    assert_eq!(root.versions[0].id, "1.21");
    assert_eq!(root.versions[0].r#type, "release");
    let back = serde_json::to_value(&root).unwrap();
    assert_eq!(back["latest"]["release"], "1.21");
    assert_eq!(back["versions"][0]["releaseTime"], "2024-06-13T11:07:26Z");
}

#[test]
fn manifest_creates_structs() {
    let latest = LatestVersionInfo {
        release: "1.21".to_string(),
        snapshot: "24w14a".to_string(),
    };
    let v = ManifestVersionInfo {
        id: "1.21".to_string(),
        r#type: "release".to_string(),
        url: "https://x".to_string(),
        time: "t".to_string(),
        release_time: "t".to_string(),
    };
    let root = VersionManifestRoot {
        latest,
        versions: vec![v],
    };
    assert_eq!(root.versions[0].id, "1.21");
}

// ── 特殊兼容：CurseForge id 字段的数字/字符串双形态 ──────────────

#[test]
fn cf_file_info_accepts_integer_ids_from_real_api() {
    // CurseForge API v1 的 File schema 里 id / modId 是 integer，而模型按源 C#
    // record 声明为 String。没有 de_id_as_string 时这里会失败并让 get_file_info
    // 对真实响应恒定报错。
    let json = r#"{
        "id": 6238281,
        "modId": 238222,
        "displayName": "JEI 1.20.1-15.3.0.4.jar",
        "fileName": "jei-1.20.1-forge-15.3.0.4.jar",
        "fileLength": 1234567,
        "releaseType": 1,
        "fileStatus": 4,
        "dependencies": []
    }"#;
    let parsed: CurseForgeFileInfo = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.file_id, "6238281");
    assert_eq!(parsed.mod_id, "238222");
    assert_eq!(parsed.file_name.as_deref(), Some("jei-1.20.1-forge-15.3.0.4.jar"));
    assert_eq!(parsed.file_length, 1234567);
}

#[test]
fn cf_file_info_still_accepts_string_ids() {
    let json = r#"{
        "id": "6238281",
        "modId": "238222",
        "fileName": "a.jar",
        "releaseType": 1,
        "fileStatus": 4
    }"#;
    let parsed: CurseForgeFileInfo = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.file_id, "6238281");
    assert_eq!(parsed.mod_id, "238222");
    // fileLength 缺失时走 default
    assert_eq!(parsed.file_length, 0);
}
