//! B2 批次验证：工具层（MurmurHash2 / NBT / 时间解析 / 库坐标 / class 常量池）
//! MurmurHash2 向量由 dotnet 10 参考实现（与源同逻辑）生成

use qomicex_core_rust::models::version_metadata::{Artifact, Library, LibraryDownloads};
use qomicex_core_rust::util::file_helper::normalize_separators;
use qomicex_core_rust::util::json_helper::{format_minecraft_datetime, parse_minecraft_datetime};
use qomicex_core_rust::util::lib_helper::{
    check_libs_ver, is_class_path, is_natives, maven_to_path,
};
use qomicex_core_rust::util::murmurhash2::{curse_forge_fingerprint, murmur_hash2};
use qomicex_core_rust::util::nbt::{NbtCompound, NbtValue, read, write};

// ── MurmurHash2（C# 参考向量）────────────────────

#[test]
fn murmurhash2_matches_csharp_reference_vectors() {
    let cases: [(&[u8], i64); 7] = [
        (b"", 1540447798),
        (b"a", 626045324),
        (b"hello", 2788266382),
        (b"hello world", 2213174766),
        (b"Minecraft", 2572622835),
        (b"1234567890123456789012345678901234567890", 1670372956),
        (b"a\tb\nc\rd e", 1323094205),
    ];
    for (input, expected) in cases {
        assert_eq!(murmur_hash2(input, 1), expected, "input: {:?}", input);
    }
}

#[test]
fn curseforge_fingerprint_matches_reference() {
    let data = b"{\"id\":\"test\", \n\"size\": 123} ";
    assert_eq!(curse_forge_fingerprint(data), 2523303756);
}

#[test]
fn murmurhash2_length_boundaries() {
    // 0/1/2/3/4/5 字节不 panic 且 1-3 字节走尾部分支
    for len in 0..=5 {
        let data = vec![0xAB; len];
        let _ = murmur_hash2(&data, 1);
    }
    assert_ne!(murmur_hash2(b"abcd", 1), murmur_hash2(b"abc", 1));
    assert_eq!(murmur_hash2(b"abcd", 0), murmur_hash2(b"abcd", 0));
}

// ── NBT 读写往返 ────────────────────────────────

#[test]
fn nbt_write_read_roundtrip() {
    let mut root = NbtCompound::new();
    root.insert("name".to_string(), NbtValue::String("Player1".to_string()));
    let mut nested = NbtCompound::new();
    nested.insert("onFire".to_string(), NbtValue::Byte(true));
    root.insert("nested".to_string(), NbtValue::Compound(nested));

    let mut buf = Vec::new();
    write(&mut buf, &root).unwrap();
    let parsed = read(&mut buf.as_slice()).unwrap();
    assert_eq!(
        parsed.get("name"),
        Some(&NbtValue::String("Player1".to_string()))
    );
    match parsed.get("nested") {
        Some(NbtValue::Compound(c)) => assert_eq!(c.get("onFire"), Some(&NbtValue::Byte(true))),
        other => panic!("expected compound, got {other:?}"),
    }
}

// ── Minecraft 时间解析/格式化 ─────────────────────

#[test]
fn datetime_parse_format_roundtrip() {
    for raw in [
        "2024-06-13T11:07:26Z",
        "2024-06-13T11:07:26+08:00",
        "2024-06-13T11:07:26.123+00:00",
    ] {
        let parsed = parse_minecraft_datetime(raw).unwrap();
        let formatted = format_minecraft_datetime(&parsed);
        assert!(
            formatted.starts_with("2024-06-13T11:07:26"),
            "got {formatted}"
        );
    }
    let parsed = parse_minecraft_datetime("2024-06-13T11:07:26+0800").unwrap();
    assert_eq!(parsed.offset_minutes, 480);
    assert!(parse_minecraft_datetime("").is_err());
    assert!(parse_minecraft_datetime("not-a-date").is_err());
}

// ── 库坐标（LibHelper）──────────────────────────

#[test]
fn maven_to_path_basic() {
    assert_eq!(
        maven_to_path("org.lwjgl:lwjgl:3.3.1"),
        "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar"
    );
}

fn lib(name: &str) -> Library {
    Library {
        name: name.to_string(),
        downloads: None,
        rules: None,
        natives: None,
        extract: None,
    }
}

#[test]
fn lib_classpath_natives_classification() {
    assert!(is_class_path(&lib("org.example:mod:1.0")));
    let natives = Library {
        name: "org.example:native:1.0".to_string(),
        downloads: None,
        rules: None,
        natives: Some(
            [("windows".to_string(), "natives-windows".to_string())]
                .into_iter()
                .collect(),
        ),
        extract: None,
    };
    assert!(is_natives(&natives));
}

#[test]
fn check_libs_ver_keeps_highest() {
    let deduped = check_libs_ver(vec![lib("g:a:1.0"), lib("g:a:2.0"), lib("g:a:1.5")]);
    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].name, "g:a:2.0");
}

/// 回归（1.16.5 启动 `Failed to locate library: lwjgl.dll`）：
/// Mojang 官方 JSON 为同一坐标列出两条条目——普通 jar 与 natives 分类器 jar，
/// name 完全相同（如 org.lwjgl:lwjgl:3.2.2）。旧分组键只按 name 分组，两者版本相同
/// → 折叠成一条并保留先出现的普通条目 → natives 条目丢失 → natives jar 永不下载。
/// 修复后按 natives 角色分组，两者各自成组、都保留。
#[test]
fn check_libs_ver_keeps_natives_twin_with_same_maven_name() {
    let plain = Library {
        name: "org.lwjgl:lwjgl:3.2.2".to_string(),
        downloads: Some(LibraryDownloads {
            artifact: Some(Artifact {
                path: Some("org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2.jar".to_string()),
                url: "https://example.invalid/lwjgl-3.2.2.jar".to_string(),
                sha1: "a".to_string(),
                size: 1,
            }),
            classifiers: None,
        }),
        rules: None,
        natives: None,
        extract: None,
    };
    let native = Library {
        name: "org.lwjgl:lwjgl:3.2.2".to_string(),
        downloads: Some(LibraryDownloads {
            artifact: Some(Artifact {
                path: Some("org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2.jar".to_string()),
                url: "https://example.invalid/lwjgl-3.2.2.jar".to_string(),
                sha1: "a".to_string(),
                size: 1,
            }),
            classifiers: Some(
                [(
                    "natives-windows".to_string(),
                    Artifact {
                        path: Some(
                            "org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2-natives-windows.jar".to_string(),
                        ),
                        url: "https://example.invalid/lwjgl-3.2.2-natives-windows.jar".to_string(),
                        sha1: "b".to_string(),
                        size: 2,
                    },
                )]
                .into_iter()
                .collect(),
            ),
        }),
        rules: None,
        natives: Some(
            [("windows".to_string(), "natives-windows".to_string())]
                .into_iter()
                .collect(),
        ),
        extract: None,
    };

    let deduped = check_libs_ver(vec![plain, native]);
    assert_eq!(
        deduped.len(),
        2,
        "普通条目与同名 natives 条目必须都保留（旧实现折叠成 1 条）"
    );
    assert_eq!(deduped.iter().filter(|l| is_natives(l)).count(), 1);
}

/// 回归：natives 角色只加后缀，不改变普通条目的按版本取高语义
/// （1.12.2 的 lwjgl-platform 2.9.4 必须仍胜过 2.9.2）。
#[test]
fn check_libs_ver_still_upgrades_natives_version() {
    let mk = |v: &str| Library {
        name: format!("org.lwjgl.lwjgl:lwjgl-platform:{v}"),
        downloads: None,
        rules: None,
        natives: Some(
            [("windows".to_string(), "natives-windows".to_string())]
                .into_iter()
                .collect(),
        ),
        extract: None,
    };
    let deduped = check_libs_ver(vec![
        mk("2.9.4-nightly-20150209"),
        mk("2.9.2-nightly-20140822"),
    ]);
    assert_eq!(deduped.len(), 1);
    assert_eq!(
        deduped[0].name,
        "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209"
    );
}

#[test]
fn normalize_separators_converts_maven_slashes_for_local_paths() {
    // 回归：安装器经 path_combine 拼接的 Maven 库路径（libraries\net/...jar）在
    // Windows verbatim（\\?\ 前缀）下 '/' 非分隔符 → os error 123。normalize_separators
    // 仅在 Windows 上把 '/' 换成 '\'，非 Windows 原样保留。
    let verbatim_maven_path = r"\\?\D:\Test\.minecraft\libraries\net/minecraftforge/forge/1.12.2-14.23.5.2864/forge-1.12.2-14.23.5.2864.jar";
    let normalized = normalize_separators(verbatim_maven_path);
    if cfg!(windows) {
        assert!(
            !normalized.contains('/'),
            "Windows 上应无 '/'：{normalized}"
        );
        assert!(
            normalized.contains(
                "net\\minecraftforge\\forge\\1.12.2-14.23.5.2864\\forge-1.12.2-14.23.5.2864.jar"
            ),
            "Maven 段应转为 '\\'：{normalized}"
        );
        assert!(normalized.starts_with(r"\\?\D:\Test\.minecraft\libraries\"));
    } else {
        assert_eq!(normalized, verbatim_maven_path);
    }
}
