//! JNI 桥接层：把 qomicex-core 的版本扫描能力暴露给 Android Kotlin。
//!
//! 导出符号与 Kotlin `CoreBridge` 的 external 函数一一对应
//! （`Java_com_qomicex_launcher_core_CoreBridge_*`，无 native 前缀）。
//!
//! `scanVersions(gameRoot)` 扫描 `<gameRoot>/versions/` 下的已装版本目录：
//! 每个目录含同名 `<name>.json`（版本元数据），JSON 解析对齐
//! `src-backend/Qomicex.Launcher.Backend.Neo/Endpoints/VersionEndpoints.cs` 的
//! scan 逻辑（gameVersion 多级 fallback + loader 检测），输出结构对齐前端
//! `ScanVersionsResponse`：`{path, versions:[{name, gameVersion, state, stateDescribe, loaders:[{type,version}]}], noJsonDirs}`。
//! 纯文件扫描，不依赖完整 GameCore（无需构建整个 DI 门面）。
//!
//! 认证系列（`*Auth*` 函数）：复用 `services/auth/` 的三个 provider（microsoft/yggdrasil/offline），
//! 每个 JNI 函数用 tokio current-thread runtime 阻塞执行 async 认证逻辑，
//! 返回对齐前端 `src/api/account.ts` 端点契约的 JSON 字符串。
//! 微软流程（client_id 由 Kotlin 侧传入，取自 `appsettings.json` 的 `Microsoft:ClientId`）：
//!   deviceCode → poll → completeLogin（Xbox→XSTS→Minecraft 全链）→ profile。

use std::collections::HashSet;
use std::path::Path;
use std::pin::Pin;

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use serde_json::{json, Value};

use crate::api::auth::AuthProvider;
use crate::api::expansion::{CurseForgeSource, FtbSource, ModrinthSource};
use crate::api::local::LocalResourcesFactory;
use crate::models::auth::{AuthRequest, AuthResult};
use crate::models::expansion::curseforge::{CurseForgeFilePageItem, CurseForgeInfo, CurseForgeSearchResult};
use crate::models::expansion::ftb::{ModpackInfo, VersionInfo as FtbVersionInfo};
use crate::models::expansion::modrinth::{DependenciesInfo, ProjectVersionInfo, SearchResultInfo};
use crate::services::auth::microsoft::MicrosoftAuthProvider;
use crate::services::auth::yggdrasil::{YggdrasilAuthProvider, YggdrasilProfilesOutcome};
use crate::services::expansion::curseforge::query::CurseForgeBase;
use crate::services::expansion::ftb::query::FtbBase;
use crate::services::expansion::modrinth::query::ModrinthBase;
use crate::services::local::factory::DefaultLocalResourcesFactory;

/// Android logcat 日志（tag QomicexCore），宿主编译时为空实现。
#[cfg(target_os = "android")]
mod android_log {
    use std::ffi::{c_char, c_int, CString};

    const ANDROID_LOG_INFO: c_int = 4;
    const ANDROID_LOG_ERROR: c_int = 6;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_print(prio: c_int, tag: *const c_char, fmt: *const c_char, ...) -> c_int;
    }

    fn log(prio: c_int, msg: &str) {
        let Ok(body) = CString::new(msg.replace('\0', " ")) else {
            return;
        };
        unsafe {
            let tag = b"QomicexCore\0".as_ptr().cast::<c_char>();
            let fmt = b"%s\0".as_ptr().cast::<c_char>();
            __android_log_print(prio, tag, fmt, body.as_ptr());
        }
    }

    pub fn info(msg: &str) {
        log(ANDROID_LOG_INFO, msg);
    }

    pub fn error(msg: &str) {
        log(ANDROID_LOG_ERROR, msg);
    }
}

#[cfg(not(target_os = "android"))]
mod android_log {
    pub fn info(_msg: &str) {}
    pub fn error(_msg: &str) {}
}

fn get_string(env: &mut JNIEnv, s: JString) -> String {
    env.get_string(&s).map(|j| j.into()).unwrap_or_default()
}

fn to_jstring(env: &mut JNIEnv, s: String) -> jstring {
    env.new_string(s)
        .map(|v| v.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// 解析版本 JSON，返回 (id, inheritsFrom, minecraftVersion, clientVersion, mainClass)。
fn parse_version_json(path: &Path, fallback_name: &str) -> Option<(String, Option<String>, Option<String>, Option<String>, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    let get_str = |key: &str| root.get(key).and_then(|v| v.as_str()).map(|s| s.to_string());
    let id = get_str("id").unwrap_or_else(|| fallback_name.to_string());
    let inherits_from = get_str("inheritsFrom");
    let mc_version = get_str("minecraftVersion");
    let client_version = get_str("clientVersion");
    let main_class = get_str("mainClass").unwrap_or_default();
    Some((id, inherits_from, mc_version, client_version, main_class))
}

/// 游戏版本多级 fallback（对齐后端 ResolveGameVersion，跳过 JAR 解析层级）。
fn resolve_game_version(
    root: &Value,
    id: &str,
    inherits_from: Option<&str>,
    client_version: Option<&str>,
    mc_version: Option<&str>,
) -> String {
    if let Some(v) = client_version {
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(v) = mc_version {
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(v) = inherits_from {
        if !v.is_empty() {
            return v.to_string();
        }
    }
    // --fml.mcVersion from arguments.game (Forge 1.13+)
    if let Some(game_args) = root
        .get("arguments")
        .and_then(|a| a.get("game"))
        .and_then(|g| g.as_array())
    {
        for i in 0..game_args.len().saturating_sub(1) {
            if game_args[i].as_str() == Some("--fml.mcVersion") {
                if let Some(v) = game_args[i + 1].as_str() {
                    if !v.is_empty() && !v.starts_with("--") {
                        return v.to_string();
                    }
                }
            }
        }
    }
    // Regex extract from id（^(\d+\.\d+(?:\.\d+)?)）
    let re = regex::Regex::new(r"^(\d+\.\d+(?:\.\d+)?)").expect("valid version regex");
    if let Some(cap) = re.captures(id) {
        return cap[1].to_string();
    }
    id.to_string()
}

/// 加载器检测（对齐后端 DetectLoaders：libraries → arguments → mainClass → id 猜测）。
fn detect_loaders(
    root: &Value,
    main_class: &str,
    id: &str,
    inherits_from: Option<&str>,
) -> Vec<(String, String)> {
    let mut fabric_ver: Option<String> = None;
    let mut quilt_ver: Option<String> = None;
    let mut forge_ver: Option<String> = None;
    let mut neo_ver: Option<String> = None;
    let mut lite_ver: Option<String> = None;
    let mut opti_ver: Option<String> = None;
    let mut cleanroom_ver: Option<String> = None;
    let mut has_legacy_fabric = false;
    let mut has_babric = false;

    if let Some(libs) = root.get("libraries").and_then(|l| l.as_array()) {
        for lib in libs {
            let name = lib.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let parts: Vec<&str> = name.split(':').collect();
            if parts.len() < 3 {
                continue;
            }
            let (group, artifact, version) = (parts[0], parts[1], parts[2]);
            if group.contains("legacyfabric") {
                has_legacy_fabric = true;
            }
            if group.eq_ignore_ascii_case("babric") {
                has_babric = true;
            }
            if cleanroom_ver.is_none() && artifact == "cleanroom" {
                cleanroom_ver = Some(version.to_string());
            }
            if fabric_ver.is_none() && (artifact == "fabric" || artifact == "fabric-loader") {
                fabric_ver = Some(version.to_string());
            }
            if quilt_ver.is_none() && (artifact == "quilt" || artifact == "quilt-loader") {
                quilt_ver = Some(version.to_string());
            }
            if lite_ver.is_none() && artifact == "liteloader" {
                lite_ver = Some(version.to_string());
            }
            if opti_ver.is_none() && artifact == "optifine" {
                opti_ver = Some(version.to_string());
            }
            if forge_ver.is_none() && artifact == "fmlloader" {
                let vv: Vec<&str> = version.split('-').collect();
                forge_ver = if vv.len() >= 2 {
                    Some(vv[1].to_string())
                } else {
                    Some(version.to_string())
                };
            }
            if neo_ver.is_none() && artifact == "neoforge" {
                neo_ver = Some(version.to_string());
            }
        }
    }

    // --fml.neoForgeVersion / --fml.forgeVersion from arguments.game
    if let Some(game_args) = root
        .get("arguments")
        .and_then(|a| a.get("game"))
        .and_then(|g| g.as_array())
    {
        for i in 0..game_args.len() {
            let next = game_args.get(i + 1).and_then(|x| x.as_str());
            let next = next.filter(|v| !v.is_empty() && !v.starts_with("--"));
            match game_args[i].as_str() {
                Some("--fml.neoForgeVersion") => {
                    if let Some(v) = next {
                        if neo_ver.is_none() {
                            neo_ver = Some(v.to_string());
                        }
                        break;
                    }
                }
                Some("--fml.forgeVersion") => {
                    if let Some(v) = next {
                        if forge_ver.is_none() {
                            forge_ver = Some(v.to_string());
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    let mut loaders: Vec<(String, String)> = Vec::new();
    if let Some(v) = fabric_ver {
        if has_legacy_fabric {
            loaders.push(("LegacyFabric".to_string(), v));
        } else if has_babric {
            loaders.push(("Babric".to_string(), v));
        } else {
            loaders.push(("Fabric".to_string(), v));
        }
    }
    if let Some(v) = cleanroom_ver {
        loaders.push(("Cleanroom".to_string(), v));
    }
    if let Some(v) = quilt_ver {
        loaders.push(("Quilt".to_string(), v));
    }
    if let Some(v) = lite_ver {
        loaders.push(("LiteLoader".to_string(), v));
    }
    if let Some(v) = opti_ver {
        loaders.push(("OptiFine".to_string(), v));
    }
    if let Some(v) = forge_ver {
        loaders.push(("Forge".to_string(), v));
    }
    if let Some(v) = neo_ver {
        loaders.push(("NeoForge".to_string(), v));
    }

    // mainClass 兜底
    if loaders.is_empty() {
        let mc = main_class.to_lowercase();
        if mc.contains("fabricmc") {
            if id.contains("babric") {
                loaders.push(("Babric".to_string(), String::new()));
            } else if id.contains("legacyfabric") {
                loaders.push(("LegacyFabric".to_string(), String::new()));
            } else {
                loaders.push(("Fabric".to_string(), String::new()));
            }
        } else if mc.contains("outlands") {
            loaders.push(("Cleanroom".to_string(), String::new()));
        } else if mc.contains("quiltmc") {
            loaders.push(("Quilt".to_string(), String::new()));
        } else if mc.contains("neoforge") || mc.contains("cpw.mods") {
            loaders.push(("NeoForge".to_string(), String::new()));
        } else if mc.contains("minecraftforge") || mc.contains("forge") {
            loaders.push(("Forge".to_string(), String::new()));
        }
    }

    // inheritsFrom != id 时按 id 命名猜测
    if loaders.is_empty() && matches!(inherits_from, Some(v) if v != id) {
        let lower = id.to_lowercase();
        let guess = if lower.contains("-forge-") {
            Some("Forge")
        } else if lower.contains("-fabric-") {
            Some("Fabric")
        } else if lower.contains("-quilt-") {
            Some("Quilt")
        } else if lower.contains("-neoforge-") {
            Some("NeoForge")
        } else if lower.contains("-cleanroom") {
            Some("Cleanroom")
        } else if lower.contains("-legacyfabric-") {
            Some("LegacyFabric")
        } else if lower.contains("-babric-") {
            Some("Babric")
        } else {
            None
        };
        if let Some(g) = guess {
            loaders.push((g.to_string(), id.to_string()));
        }
    }

    loaders
}

/// 扫描指定游戏根目录下的已装版本。返回对齐前端 ScanVersionsResponse 的 JSON 字符串。
fn scan_versions_impl(game_root: &str) -> Value {
    let versions_dir = Path::new(game_root).join("versions");
    let mut versions: Vec<Value> = Vec::new();
    let mut no_json_dirs: Vec<String> = Vec::new();

    android_log::info(&format!("scanVersions: game_root={game_root} versionsDir={} exists={}", versions_dir.display(), versions_dir.is_dir()));

    if versions_dir.is_dir() {
        match std::fs::read_dir(&versions_dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let name = dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let json_path = dir.join(format!("{name}.json"));
                    if !json_path.is_file() {
                        android_log::info(&format!("scanVersions: {name} no json, corrupted"));
                        no_json_dirs.push(name.clone());
                        versions.push(json!({
                            "name": name,
                            "gameVersion": name,
                            "state": "Corrupted",
                            "stateDescribe": "版本文件缺失",
                            "loaders": null,
                            "modpack": null
                        }));
                        continue;
                    }

                    if let Some((id, inherits_from, mc_version, client_version, main_class)) =
                        parse_version_json(&json_path, &name)
                    {
                        let text = std::fs::read_to_string(&json_path).unwrap_or_default();
                        let root: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

                        let game_version = resolve_game_version(
                            &root,
                            &id,
                            inherits_from.as_deref(),
                            client_version.as_deref(),
                            mc_version.as_deref(),
                        );
                        let loaders = detect_loaders(&root, &main_class, &id, inherits_from.as_deref());
                        let loaders_json: Vec<Value> = loaders
                            .iter()
                            .map(|(t, v)| json!({"type": t, "version": v}))
                            .collect();
                        android_log::info(&format!("scanVersions: found {name} gameVersion={game_version} loaders={}", loaders.len()));

                        versions.push(json!({
                            "name": id,
                            "gameVersion": game_version,
                            "state": "Available",
                            "stateDescribe": "",
                            "loaders": if loaders_json.is_empty() { Value::Null } else { Value::Array(loaders_json) },
                            "modpack": null
                        }));
                    }
                }
            }
            Err(e) => {
                android_log::error(&format!("scanVersions: read_dir failed: {e}"));
            }
        }
    } else {
        android_log::error(&format!("scanVersions: versions dir does not exist: {}", versions_dir.display()));
    }

    android_log::info(&format!("scanVersions: returning {} versions, {} noJsonDirs", versions.len(), no_json_dirs.len()));
    json!({
        "path": game_root,
        "versions": versions,
        "noJsonDirs": no_json_dirs
    })
}

/// 对应 Kotlin CoreBridge.scanVersions(gameRoot: String): String
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_scanVersions(
    mut env: JNIEnv,
    _class: JClass,
    game_root: JString,
) -> jstring {
    let game_root = get_string(&mut env, game_root);
    let result = scan_versions_impl(&game_root).to_string();
    to_jstring(&mut env, result)
}

// ============================== 认证 JNI ==============================

/// 阻塞执行 async 认证逻辑（每个 JNI 调用创建独立 current-thread runtime）。
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime build")
        .block_on(future)
}

/// AuthResult → JSON（camelCase，对齐前端 Account/AuthResponse 契约）。
fn auth_result_json(a: &AuthResult) -> Value {
    json!({
        "success": a.success,
        "username": a.username,
        "accessToken": a.access_token,
        "uuid": a.uuid,
        "refreshToken": a.refresh_token,
        "userType": a.user_type,
        "errorMessage": a.error_message
    })
}

/// YggdrasilProfilesOutcome → JSON（对齐前端 YggdrasilProfilesResponse）。
fn yggdrasil_outcome_json(o: &YggdrasilProfilesOutcome) -> Value {
    let profiles: Vec<Value> = o
        .profiles
        .iter()
        .map(|(id, name)| json!({"id": id, "name": name}))
        .collect();
    json!({
        "success": o.success,
        "accessToken": o.access_token,
        "clientToken": o.client_token,
        "profiles": if profiles.is_empty() { Value::Null } else { Value::Array(profiles) },
        "errorMessage": o.error_message
    })
}

/// 对应 Kotlin CoreBridge.yggdrasilAuth(username, password, serverUrl): String
/// 认证并返回全部可用角色（对齐 POST /auth/yggdrasil）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_yggdrasilAuth(
    mut env: JNIEnv,
    _class: JClass,
    username: JString,
    password: JString,
    server_url: JString,
) -> jstring {
    let username = get_string(&mut env, username);
    let password = get_string(&mut env, password);
    let server_url = get_string(&mut env, server_url);
    let provider = YggdrasilAuthProvider::new(reqwest::Client::new(), server_url);
    let request = AuthRequest {
        username: Some(username),
        password: Some(password),
        access_token: None,
        server_url: None,
        is_offline: false,
    };
    let result: Value = match block_on(provider.authenticate_with_profiles(request)) {
        Ok(outcome) => yggdrasil_outcome_json(&outcome),
        Err(e) => json!({
            "success": false,
            "accessToken": null,
            "clientToken": null,
            "profiles": null,
            "errorMessage": format!("认证失败: {e}")
        }),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.microsoftDeviceCode(clientId: String): String
/// 对齐 POST /auth/microsoft/device-code（C# 侧包装在 AuthResponse 中）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_microsoftDeviceCode(
    mut env: JNIEnv,
    _class: JClass,
    client_id: JString,
) -> jstring {
    let client_id = get_string(&mut env, client_id);
    let provider = MicrosoftAuthProvider::new(reqwest::Client::new(), client_id);
    let result: Value = match block_on(provider.start_device_code()) {
        Ok(Some(dc)) => json!({
            "success": true,
            "deviceCode": dc.device_code,
            "userCode": dc.user_code,
            "verificationUri": dc.verification_uri,
            "interval": dc.interval,
            "expiresIn": dc.expires_in,
            "userType": "microsoft"
        }),
        Ok(None) => json!({"success": false, "errorMessage": "设备码登录暂不可用"}),
        Err(e) => json!({"success": false, "errorMessage": format!("获取设备码失败: {e}")}),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.microsoftPoll(clientId: String, deviceCode: String): String
/// 对齐 POST /auth/microsoft/poll 的单次轮询（返回 PollTokenResult）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_microsoftPoll(
    mut env: JNIEnv,
    _class: JClass,
    client_id: JString,
    device_code: JString,
) -> jstring {
    let client_id = get_string(&mut env, client_id);
    let device_code = get_string(&mut env, device_code);
    let provider = MicrosoftAuthProvider::new(reqwest::Client::new(), client_id);
    let result: Value = match block_on(provider.poll_for_token(&device_code)) {
        Ok(Some(p)) => json!({
            "accessToken": p.access_token,
            "refreshToken": p.refresh_token,
            "error": p.error,
            "isCompleted": p.is_completed,
            "isPending": p.is_pending
        }),
        _ => json!({
            "accessToken": null,
            "refreshToken": null,
            "error": "poll failed",
            "isCompleted": false,
            "isPending": true
        }),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.microsoftCompleteLogin(clientId, accessToken, refreshToken): String
/// 设备码登录完成：Xbox → XSTS → Minecraft 链式认证 + 角色档案（对齐 poll 完成分支的 AuthResponse）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_microsoftCompleteLogin(
    mut env: JNIEnv,
    _class: JClass,
    client_id: JString,
    access_token: JString,
    refresh_token: JString,
) -> jstring {
    let client_id = get_string(&mut env, client_id);
    let access_token = get_string(&mut env, access_token);
    let refresh_token = get_string(&mut env, refresh_token);
    let provider = MicrosoftAuthProvider::new(reqwest::Client::new(), client_id);
    let result: Value = match block_on(provider.complete_login(&access_token, &refresh_token)) {
        Ok(a) => auth_result_json(&a),
        Err(e) => json!({
            "success": false,
            "username": null,
            "accessToken": null,
            "uuid": null,
            "refreshToken": null,
            "userType": null,
            "errorMessage": format!("{e}")
        }),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.microsoftProfile(accessToken: String): String
/// 取 Minecraft 角色档案（对齐 POST /auth/microsoft/info 的 profile 来源）。
/// 无档案 / 网络失败 → JSON null。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_microsoftProfile(
    mut env: JNIEnv,
    _class: JClass,
    access_token: JString,
) -> jstring {
    let access_token = get_string(&mut env, access_token);
    let provider = MicrosoftAuthProvider::new(reqwest::Client::new(), String::new());
    let result: Value = match block_on(provider.get_minecraft_profile(&access_token)) {
        Ok(Some((id, name))) => json!({"id": id, "name": name}),
        _ => Value::Null,
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.microsoftRefresh(clientId: String, refreshToken: String): String
/// 刷新令牌并续期（对齐 POST /auth/microsoft/refresh）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_microsoftRefresh(
    mut env: JNIEnv,
    _class: JClass,
    client_id: JString,
    refresh_token: JString,
) -> jstring {
    let client_id = get_string(&mut env, client_id);
    let refresh_token = get_string(&mut env, refresh_token);
    let provider = MicrosoftAuthProvider::new(reqwest::Client::new(), client_id);
    let result: Value = match block_on(provider.refresh_login(&refresh_token)) {
        Ok(a) => auth_result_json(&a),
        Err(e) => json!({
            "success": false,
            "username": null,
            "accessToken": null,
            "uuid": null,
            "refreshToken": null,
            "userType": null,
            "errorMessage": format!("{e}")
        }),
    };
    to_jstring(&mut env, result.to_string())
}

// ============================== 资源中心（Modrinth）JNI ==============================
// 对应 Kotlin CoreBridge 的 resourceSearch / resourceDetail / resourceVersions /
// resourceVersionDownloads / resourceDependencies / instanceFilesMetadata。
// 返回对齐前端 src/types/index.ts 资源中心契约的 JSON 字符串；
// 出错时返回 {"__error__":"..."}，由 Kotlin 侧转 BridgeException。

/// 错误包装：统一 {"__error__": message} 结构。
fn err_json(message: String) -> Value {
    json!({ "__error__": message })
}

fn mr_index(sort: &str) -> &'static str {
    match sort.to_lowercase().as_str() {
        "downloads" => "downloads",
        "updated" => "updated",
        "newest" => "newest",
        _ => "relevance",
    }
}

// ============================== CurseForge / FTB 映射 ==============================
// 对齐桌面 ResourceCenterEndpoints.cs 的 MapCfClassId / MapCfSort / MapCfLoader /
// MapFtSort / MapCfUrlSlug / ExtractCFLoaders。

/// category → CurseForge classId（源：MapCfClassId）。
fn cf_class_id(category: &str) -> Option<i32> {
    match category.to_lowercase().as_str() {
        "mod" => Some(6),
        "modpack" => Some(4471),
        "shader" => Some(6552),
        "resourcepack" => Some(12),
        "datapack" => Some(6945),
        "save" => Some(17),
        _ => None,
    }
}

/// category → 站点 URL 段（源：MapCfUrlSlug）。
fn cf_url_slug(category: &str) -> &'static str {
    match category.to_lowercase().as_str() {
        "modpack" => "modpacks",
        "shader" => "shaders",
        "resourcepack" => "texture-packs",
        "datapack" => "data-packs",
        "save" => "worlds",
        _ => "mc-mods",
    }
}

/// sort → CurseForge sortField（源：MapCfSort）。
fn cf_sort(sort: &str) -> i32 {
    match sort.to_lowercase().as_str() {
        "downloads" => 6,
        "updated" => 3,
        "name" => 4,
        "newest" => 11,
        _ => 6,
    }
}

/// loader → CurseForge modLoaderTypes 字符串（源：MapCfLoader + CfModLoaderType 常量）。
fn cf_loader(loader: &str) -> Option<&'static str> {
    match loader.to_lowercase().as_str() {
        "forge" => Some("Forge"),
        "fabric" => Some("Fabric"),
        "quilt" => Some("Quilt"),
        "neoforge" => Some("NeoForge"),
        "liteloader" => Some("LiteLoader"),
        _ => None,
    }
}

/// sort → FTB 排序键（源：MapFtSort）。
fn ftb_sort(sort: &str) -> &'static str {
    match sort.to_lowercase().as_str() {
        "downloads" => "downloads",
        "updated" => "updated",
        "newest" => "released",
        "name" => "name",
        "relevance" => "featured",
        _ => "downloads",
    }
}

/// 文件记录提取加载器（源：ExtractCFLoaders）：
/// gameVersions 中的 forge/fabric/quilt/neoforge/liteloader 字符串 + modLoader 数值映射。
fn extract_cf_loaders(game_versions: Option<&[String]>, mod_loader: i32) -> Vec<String> {
    let mut loaders: Vec<String> = Vec::new();
    if let Some(gvs) = game_versions {
        for gv in gvs {
            let s = gv.to_lowercase();
            if matches!(s.as_str(), "forge" | "fabric" | "quilt" | "neoforge" | "liteloader") {
                loaders.push(s);
            }
        }
    }
    match mod_loader {
        2 => loaders.push("forge".to_string()),
        4 => loaders.push("fabric".to_string()),
        5 => loaders.push("quilt".to_string()),
        6 => loaders.push("neoforge".to_string()),
        _ => {}
    }
    loaders
}

/// epoch 毫秒 → ISO 8601 UTC（对齐 C# `DateTimeOffset.FromUnixTimeMilliseconds(v.Updated).ToString("o")`）。
fn epoch_millis_to_iso(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// CurseForge 搜索结果 → 前端 ResourceItem。
fn cf_item_json(r: &CurseForgeSearchResult) -> Value {
    json!({
        "id": r.id,
        "title": r.name,
        "description": r.summary,
        "author": r.authors.first().map(|a| a.name.clone()).unwrap_or_default(),
        "iconUrl": r.icon_url,
        "downloads": r.download_count.parse::<i64>().unwrap_or(0),
        "projectType": "",
        "source": "curseforge",
        "categories": r.categories.iter().map(|c| c.slug.clone().unwrap_or_else(|| c.name.clone())).collect::<Vec<_>>(),
        "projectUrl": format!("https://www.curseforge.com/minecraft/mc-mods/{}", r.slug),
        "slug": r.slug,
        "latestVersion": ""
    })
}

/// CurseForge 模组详情 → 前端 ResourceDetail。
fn cf_detail_json(info: &CurseForgeInfo, category: &str) -> Value {
    json!({
        "id": info.id.to_string(),
        "title": info.name,
        "description": info.summary.clone().unwrap_or_default(),
        "author": info.authors.as_ref().and_then(|a| a.first()).map(|a| a.name.clone()).unwrap_or_default(),
        "iconUrl": info.screenshots.as_ref().and_then(|s| s.first()).and_then(|s| s.thumbnail_url.clone()).unwrap_or_default(),
        "downloads": info.download_count,
        "projectType": "",
        "source": "curseforge",
        "categories": info.categories.as_ref().map(|c| c.iter().map(|c| c.slug.clone().unwrap_or_else(|| c.name.clone())).collect::<Vec<_>>()).unwrap_or_default(),
        "projectUrl": format!("https://www.curseforge.com/minecraft/{}/{}", cf_url_slug(category), info.slug.clone().unwrap_or_else(|| info.id.to_string())),
        "slug": info.slug.clone().unwrap_or_else(|| info.id.to_string()),
        "latestVersion": "",
        "body": ""
    })
}

/// CurseForge 文件条目 → 前端 ResourceVersion。
fn cf_version_json(f: &CurseForgeFilePageItem) -> Value {
    let game_versions: Vec<String> = f
        .game_versions
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|v| !v.is_empty())
        .collect();
    let loaders = extract_cf_loaders(Some(game_versions.as_slice()), f.mod_loader);
    let deps: Vec<Value> = f
        .dependencies
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.r#type == 3)
        .map(|d| {
            json!({
                "versionId": Value::Null,
                "projectId": d.mod_id.to_string(),
                "fileName": Value::Null,
                "dependencyType": "required"
            })
        })
        .collect();
    json!({
        "id": f.id.to_string(),
        "name": f.display_name.clone().unwrap_or_else(|| f.file_name.clone().unwrap_or_default()),
        "versionNumber": f.file_name.clone().unwrap_or_default(),
        "gameVersions": game_versions,
        "loaders": loaders,
        "downloads": [json!({
            "url": f.download_url.clone().unwrap_or_default(),
            "fileName": f.file_name.clone().unwrap_or_default(),
            "size": f.file_length
        })],
        "dependencies": deps,
        "datePublished": f.file_date
    })
}

/// FTB 整合包 → 前端 ResourceItem。
fn ftb_item_json(p: &ModpackInfo) -> Value {
    let slug = p.slug.clone().unwrap_or_else(|| p.id.to_string());
    json!({
        "id": p.id.to_string(),
        "title": p.name,
        "description": p.synopsis.clone().unwrap_or_default(),
        "author": p.authors.as_ref().map(|a| a.iter().take(2).map(|a| a.name.clone()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        "iconUrl": p.art.as_ref().and_then(|a| a.iter().find(|a| a.r#type.as_deref() == Some("square"))).map(|a| a.url.clone()).unwrap_or_default(),
        "downloads": p.installs,
        "projectType": "modpack",
        "source": "ftb",
        "categories": p.tags.as_ref().map(|t| t.iter().map(|t| t.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "projectUrl": format!("https://www.feed-the-beast.com/modpacks/{slug}"),
        "slug": slug,
        "latestVersion": ""
    })
}

/// FTB 整合包详情 → 前端 ResourceDetail。
fn ftb_detail_json(p: &ModpackInfo) -> Value {
    let slug = p.slug.clone().unwrap_or_else(|| p.id.to_string());
    json!({
        "id": p.id.to_string(),
        "title": p.name,
        "description": p.synopsis.clone().unwrap_or_default(),
        "author": p.authors.as_ref().map(|a| a.iter().take(2).map(|a| a.name.clone()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        "iconUrl": p.art.as_ref().and_then(|a| a.iter().find(|a| a.r#type.as_deref() == Some("square"))).map(|a| a.url.clone()).unwrap_or_default(),
        "downloads": p.installs,
        "projectType": "modpack",
        "source": "ftb",
        "categories": p.tags.as_ref().map(|t| t.iter().map(|t| t.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "projectUrl": format!("https://www.feed-the-beast.com/modpacks/{slug}"),
        "slug": slug,
        "latestVersion": "",
        "body": p.description.clone().unwrap_or_default()
    })
}

/// FTB 版本 → 前端 ResourceVersion（对齐桌面 {id}/versions 的 ftb 分支）。
fn ftb_version_json(v: &FtbVersionInfo) -> Value {
    let mc_version = v.targets.as_ref().and_then(|t| {
        t.iter()
            .find(|t| t.r#type.as_deref() == Some("game") || t.name.as_deref() == Some("minecraft"))
            .and_then(|t| t.version.clone())
    });
    let mc_versions: Vec<String> = mc_version.map(|m| vec![m]).unwrap_or_default();
    let loaders: Vec<String> = v
        .targets
        .as_ref()
        .map(|t| {
            t.iter()
                .filter(|t| {
                    t.r#type.as_deref() == Some("modloader")
                        || matches!(t.name.as_deref(), Some("forge") | Some("fabric") | Some("neoforge"))
                })
                .filter_map(|t| t.version.clone().or_else(|| t.name.clone()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": v.id.to_string(),
        "name": v.name,
        "versionNumber": v.name,
        "gameVersions": mc_versions,
        "loaders": loaders,
        "downloads": [],
        "dependencies": Value::Null,
        "datePublished": epoch_millis_to_iso(v.updated)
    })
}

/// CurseForge 全量文件列表（分页循环至 totalCount，对齐桌面 {id}/versions 的 cf 分支）。
async fn cf_fetch_all_files(
    cf: &CurseForgeBase,
    id: &str,
    game_version: Option<&str>,
) -> Result<Vec<CurseForgeFilePageItem>, crate::error::Error> {
    let first = cf.get_file_page(id, 0, 50, game_version).await?;
    let mut items = first.files;
    let mut index = 50;
    let total = first.total_count as i64;
    while (index as i64) < total {
        let page = cf.get_file_page(id, index, 50, game_version).await?;
        items.extend(page.files);
        index += 50;
    }
    Ok(items)
}

/// CurseForge 版本列表 JSON（拉全量后按 gameVersion/loader 客户端过滤，对齐桌面端点）。
async fn cf_versions_json(cf: &CurseForgeBase, id: &str, game_version: &str, loader: &str) -> Value {
    let files = match cf_fetch_all_files(
        cf,
        id,
        if game_version.is_empty() { None } else { Some(game_version) },
    )
    .await
    {
        Ok(f) => f,
        Err(e) => return err_json(format!("{e}")),
    };
    let mut dtos: Vec<Value> = files.iter().map(cf_version_json).collect();
    if !game_version.is_empty() {
        dtos.retain(|v| {
            v["gameVersions"]
                .as_array()
                .is_some_and(|gvs| gvs.iter().any(|g| g.as_str() == Some(game_version)))
        });
    }
    if !loader.is_empty() {
        let norm = loader.trim().to_lowercase();
        dtos.retain(|v| {
            let ls = v["loaders"].as_array().cloned().unwrap_or_default();
            ls.is_empty() || ls.iter().any(|l| l.as_str().is_some_and(|s| s.eq_ignore_ascii_case(&norm)))
        });
    }
    json!(dtos)
}

/// FTB 版本列表 JSON（get_pack_detail 后按 targets 过滤，对齐桌面端点）。
async fn ftb_versions_json(ftb: &FtbBase, id: &str, game_version: &str, loader: &str) -> Value {
    let Ok(ftb_id) = id.parse::<i32>() else {
        return json!([]);
    };
    let Ok(Some(pack)) = ftb.get_pack_detail(ftb_id).await else {
        return json!([]);
    };
    let mut versions: Vec<FtbVersionInfo> = pack.versions.clone().unwrap_or_default();
    if !game_version.is_empty() {
        versions.retain(|v| {
            v.targets
                .as_ref()
                .is_some_and(|t| t.iter().any(|t| t.version.as_deref() == Some(game_version)))
        });
    }
    if !loader.is_empty() {
        versions.retain(|v| {
            v.targets.as_ref().is_some_and(|t| {
                t.iter()
                    .any(|t| t.version.as_deref().is_some_and(|tv| tv.eq_ignore_ascii_case(loader)))
            })
        });
    }
    json!(versions.iter().map(ftb_version_json).collect::<Vec<_>>())
}

/// 搜索结果条目 → 前端 ResourceItem。
fn mr_item_json(r: &SearchResultInfo) -> Value {
    let slug = r.slug.clone().unwrap_or_else(|| r.id.clone());
    json!({
        "id": r.id,
        "title": r.name,
        "description": r.description,
        "author": r.author,
        "iconUrl": r.icon_url.clone().unwrap_or_default(),
        "downloads": r.download_count,
        "projectType": r.r#type.clone().unwrap_or_default(),
        "source": "modrinth",
        "categories": r.categories.clone().unwrap_or_default(),
        "projectUrl": format!("https://modrinth.com/project/{slug}"),
        "slug": slug,
        "latestVersion": ""
    })
}

/// 项目详情 → 前端 ResourceDetail（author 经 v3 team 接口补全，失败置空）。
async fn mr_detail_json(mr: &ModrinthBase, http: &reqwest::Client, id: &str) -> Value {
    let info = match mr.get_project_info(id).await {
        Ok(i) => i,
        Err(e) => return err_json(format!("{e}")),
    };
    let slug = info.slug.clone().unwrap_or_else(|| info.id.clone());
    let mut author = String::new();
    if let Some(team) = info.team.clone() {
        let url = format!("https://api.modrinth.com/v3/team/{team}/members");
        if let Ok(resp) = http.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&body) {
                        if let Some(first) = arr.first() {
                            author = first
                                .get("user")
                                .and_then(|u| u.get("username"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                        }
                    }
                }
            }
        }
    }
    json!({
        "id": info.id,
        "title": info.name,
        "description": info.description,
        "author": author,
        "iconUrl": info.icon_url.clone().unwrap_or_default(),
        "downloads": info.download_count,
        "projectType": info.project_type.clone().unwrap_or_default(),
        "source": "modrinth",
        "categories": info.categories.clone().unwrap_or_default(),
        "projectUrl": format!("https://modrinth.com/project/{slug}"),
        "slug": slug,
        "latestVersion": "",
        "body": info.full_description.clone().unwrap_or_default()
    })
}

/// 版本列表条目 → 前端 ResourceVersion。
fn mr_version_json(v: &ProjectVersionInfo) -> Value {
    let files: Vec<Value> = v
        .files
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|f| {
            json!({
                "url": f.download_url,
                "fileName": f.filename,
                "size": f.size
            })
        })
        .collect();
    let deps: Vec<Value> = v
        .dependencies_infos
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            json!({
                "versionId": d.version_id,
                "projectId": d.project_id.unwrap_or_default(),
                "fileName": d.file_name,
                "dependencyType": d.dependency_type.unwrap_or_default()
            })
        })
        .collect();
    json!({
        "id": v.id,
        "name": v.name,
        "versionNumber": v.version_number.clone().unwrap_or_else(|| v.name.clone()),
        "gameVersions": v.game_version_ids.clone().unwrap_or_default(),
        "loaders": v.loaders.clone().unwrap_or_default(),
        "downloads": files,
        "dependencies": deps,
        "datePublished": v.published_at
    })
}

/// 对应 Kotlin CoreBridge.resourceSearch(requestJson: String): String
/// request: {"source","category","keyword","page","pageSize","sort","gameVersion","loader","apiKey"}
/// 支持 modrinth / curseforge / ftb 三源，输出对齐前端 ResourceSearchResponse。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_resourceSearch(
    mut env: JNIEnv,
    _class: JClass,
    request_json: JString,
) -> jstring {
    let request_json = get_string(&mut env, request_json);
    let req: Value = serde_json::from_str(&request_json).unwrap_or(Value::Null);
    let source = req.get("source").and_then(|s| s.as_str()).unwrap_or("modrinth");
    let category = req.get("category").and_then(|s| s.as_str()).unwrap_or("mod");
    let keyword = req.get("keyword").and_then(|s| s.as_str()).unwrap_or("");
    let page = req.get("page").and_then(|p| p.as_i64()).unwrap_or(1) as i32;
    let page_size = req.get("pageSize").and_then(|p| p.as_i64()).unwrap_or(20) as i32;
    let sort = req.get("sort").and_then(|s| s.as_str()).unwrap_or("relevance");
    let game_version = req.get("gameVersion").and_then(|s| s.as_str()).unwrap_or("");
    let loader = req.get("loader").and_then(|s| s.as_str()).unwrap_or("");

    let result: Value = match source {
        "modrinth" => {
            let mr = ModrinthBase::new(reqwest::Client::new(), None);
            let loader_arr: Vec<String> = if loader.is_empty() {
                Vec::new()
            } else {
                vec![loader.to_string()]
            };
            let loader_slice: Option<Vec<String>> = if loader_arr.is_empty() { None } else { Some(loader_arr.clone()) };
            let gv_opt: Option<String> = if game_version.is_empty() { None } else { Some(game_version.to_string()) };

            match block_on(mr.search(
                keyword,
                Some(category),
                gv_opt.as_deref(),
                None,
                loader_slice.as_deref(),
                mr_index(sort),
                page - 1,
                page_size,
            )) {
                Ok(res) => {
                    let items: Vec<Value> = res.results.iter().map(mr_item_json).collect();
                    json!({
                        "items": items,
                        "total": res.total_results,
                        "page": page,
                        "pageSize": page_size
                    })
                }
                Err(e) => err_json(format!("{e}")),
            }
        }
        "curseforge" => {
            let api_key = req.get("apiKey").and_then(|s| s.as_str()).unwrap_or("");
            if api_key.is_empty() {
                err_json("CurseForge API Key 未配置".to_string())
            } else {
                let cf = CurseForgeBase::new(reqwest::Client::new(), api_key.to_string(), None);
                let gv_opt: Option<Vec<String>> = if game_version.is_empty() {
                    None
                } else {
                    Some(vec![game_version.to_string()])
                };
                let loader_arr: Vec<String> = cf_loader(loader)
                    .map(|l| vec![l.to_string()])
                    .unwrap_or_default();
                let loader_slice: Option<&[String]> = if loader_arr.is_empty() { None } else { Some(&loader_arr) };
                match block_on(cf.search(
                    keyword,
                    gv_opt.as_deref(),
                    None,
                    loader_slice,
                    Some(cf_sort(sort)),
                    Some(page),
                    Some(page_size),
                    cf_class_id(category),
                )) {
                    Ok(res) => {
                        let items: Vec<Value> = res.results.iter().map(cf_item_json).collect();
                        json!({
                            "items": items,
                            "total": res.total_count,
                            "page": page,
                            "pageSize": page_size
                        })
                    }
                    Err(e) => err_json(format!("{e}")),
                }
            }
        }
        "ftb" => {
            if !category.eq_ignore_ascii_case("modpack") {
                json!({ "items": [], "total": 0, "page": page, "pageSize": page_size })
            } else {
                let ftb = FtbBase::new(reqwest::Client::new(), None, None);
                let query_opt = if keyword.is_empty() { None } else { Some(keyword) };
                let gv_opt = if game_version.is_empty() { None } else { Some(game_version) };
                let loader_opt = if loader.is_empty() { None } else { Some(loader) };
                match block_on(ftb.search(
                    query_opt,
                    None,
                    gv_opt,
                    loader_opt,
                    ftb_sort(sort),
                    1000,
                )) {
                    Ok(packs) => {
                        let total = packs.len();
                        let page_idx = page.max(1);
                        let size = page_size.max(1) as usize;
                        let page_items: Vec<Value> = packs
                            .iter()
                            .skip((page_idx as usize - 1) * size)
                            .take(size)
                            .map(ftb_item_json)
                            .collect();
                        json!({
                            "items": page_items,
                            "total": total,
                            "page": page_idx,
                            "pageSize": page_size
                        })
                    }
                    Err(e) => err_json(format!("{e}")),
                }
            }
        }
        _ => err_json(format!("资源源 {source} 暂不支持")),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.resourceDetail(id: String, source: String, category: String, apiKey: String): String
/// 支持 modrinth / curseforge / ftb 三源，输出对齐前端 ResourceDetail。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_resourceDetail(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    source: JString,
    category: JString,
    api_key: JString,
) -> jstring {
    let id = get_string(&mut env, id);
    let source = get_string(&mut env, source);
    let category = get_string(&mut env, category);
    let api_key = get_string(&mut env, api_key);
    let result: Value = match source.as_str() {
        "modrinth" => {
            let mr = ModrinthBase::new(reqwest::Client::new(), None);
            block_on(mr_detail_json(&mr, &reqwest::Client::new(), &id))
        }
        "curseforge" => {
            if api_key.is_empty() {
                err_json("CurseForge API Key 未配置".to_string())
            } else {
                let cf = CurseForgeBase::new(reqwest::Client::new(), api_key, None);
                let result = match block_on(cf.get_mod_info(&id)) {
                    Ok(info) => cf_detail_json(&info, &category),
                    Err(e) => err_json(format!("{e}")),
                };
                result
            }
        }
        "ftb" => {
            let ftb = FtbBase::new(reqwest::Client::new(), None, None);
            match id.parse::<i32>() {
                Ok(ftb_id) => match block_on(ftb.get_pack_detail(ftb_id)) {
                    Ok(Some(pack)) => ftb_detail_json(&pack),
                    Ok(None) => err_json("整合包不存在".to_string()),
                    Err(e) => err_json(format!("{e}")),
                },
                Err(_) => err_json("无效的整合包 ID".to_string()),
            }
        }
        _ => err_json(format!("资源源 {source} 暂不支持")),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.resourceVersions(id: String, source: String, gameVersion: String, loader: String, apiKey: String): String
/// 支持 modrinth / curseforge / ftb 三源，返回前端 ResourceVersion 数组。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_resourceVersions(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    source: JString,
    game_version: JString,
    loader: JString,
    api_key: JString,
) -> jstring {
    let id = get_string(&mut env, id);
    let source = get_string(&mut env, source);
    let game_version = get_string(&mut env, game_version);
    let loader = get_string(&mut env, loader);
    let api_key = get_string(&mut env, api_key);
    let result: Value = match source.as_str() {
        "modrinth" => {
            let mr = ModrinthBase::new(reqwest::Client::new(), None);
            match block_on(mr.get_project_version_info(&id)) {
                Ok(versions) => {
                    let items: Vec<Value> = versions.iter().map(mr_version_json).collect();
                    json!(items)
                }
                Err(e) => err_json(format!("{e}")),
            }
        }
        "curseforge" => {
            if api_key.is_empty() {
                err_json("CurseForge API Key 未配置".to_string())
            } else {
                let cf = CurseForgeBase::new(reqwest::Client::new(), api_key, None);
                block_on(cf_versions_json(&cf, &id, &game_version, &loader))
            }
        }
        "ftb" => {
            let ftb = FtbBase::new(reqwest::Client::new(), None, None);
            block_on(ftb_versions_json(&ftb, &id, &game_version, &loader))
        }
        _ => err_json(format!("资源源 {source} 暂不支持")),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.resourceVersionDownloads(id: String, versionId: String, source: String, apiKey: String): String
/// 支持 modrinth / curseforge / ftb 三源，返回前端 ResourceFile 数组。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_resourceVersionDownloads(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    version_id: JString,
    source: JString,
    api_key: JString,
) -> jstring {
    let id = get_string(&mut env, id);
    let version_id = get_string(&mut env, version_id);
    let source = get_string(&mut env, source);
    let api_key = get_string(&mut env, api_key);
    let result: Value = match source.as_str() {
        "modrinth" => {
            let mr = ModrinthBase::new(reqwest::Client::new(), None);
            match block_on(mr.get_version_info(&version_id)) {
                Ok(v) => {
                    let files: Vec<Value> = v
                        .files
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| {
                            json!({
                                "url": f.download_url,
                                "fileName": f.filename,
                                "size": f.size
                            })
                        })
                        .collect();
                    json!(files)
                }
                Err(e) => err_json(format!("{e}")),
            }
        }
        "curseforge" => {
            if api_key.is_empty() {
                err_json("CurseForge API Key 未配置".to_string())
            } else {
                let cf = CurseForgeBase::new(reqwest::Client::new(), api_key, None);
                match block_on(cf.get_download_url(&id, &version_id)) {
                    Ok(url) => json!([json!({ "url": url, "fileName": "", "size": 0 })]),
                    Err(e) => err_json(format!("{e}")),
                }
            }
        }
        "ftb" => {
            let ftb = FtbBase::new(reqwest::Client::new(), None, None);
            match (id.parse::<i32>(), version_id.parse::<i32>()) {
                (Ok(pack_id), Ok(ver_id)) => match block_on(ftb.get_version_detail(pack_id, ver_id)) {
                    Ok(Some(detail)) => {
                        let files: Vec<Value> = detail
                            .files
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|f| f.name.to_lowercase().ends_with(".zip"))
                            .map(|f| {
                                json!({
                                    "url": f.url,
                                    "fileName": f.name,
                                    "size": f.size
                                })
                            })
                            .collect();
                        json!(files)
                    }
                    _ => json!([]),
                },
                _ => json!([]),
            }
        }
        _ => err_json(format!("资源源 {source} 暂不支持")),
    };
    to_jstring(&mut env, result.to_string())
}

/// 依赖递归解析（对齐 C# ResolveMRDeps：depth>5 截断、visited 去重、best 版本选择、required 递归）。
fn resolve_mr_deps<'a>(
    mr: &'a ModrinthBase,
    project_id: String,
    version_id: Option<String>,
    game_version: Option<String>,
    loader: Option<String>,
    visited: &'a mut HashSet<String>,
    depth: i32,
) -> Pin<Box<dyn std::future::Future<Output = Vec<Value>> + 'a>> {
    Box::pin(async move {
        if depth > 5 {
            return Vec::new();
        }
        if !visited.insert(project_id.clone()) {
            return Vec::new();
        }

        let versions = match mr.get_project_version_info(&project_id).await {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        if versions.is_empty() {
            return Vec::new();
        }

        let best = if depth == 0 {
            version_id
                .as_ref()
                .and_then(|vid| versions.iter().find(|v| &v.id == vid))
        } else {
            versions
                .iter()
                .filter(|v| {
                    let gv_ok = game_version.as_ref().map_or(true, |gv| {
                        v.game_version_ids.as_ref().is_none_or(|gvs| gvs.contains(gv))
                    });
                    let ld_ok = loader.as_ref().map_or(true, |ld| {
                        v.loaders.as_ref().is_none_or(|ls| ls.is_empty() || ls.contains(ld))
                    });
                    gv_ok && ld_ok
                })
                .max_by(|a, b| a.published_at.cmp(&b.published_at))
                .or_else(|| versions.iter().max_by(|a, b| a.published_at.cmp(&b.published_at)))
        };

        let Some(best) = best else { return Vec::new() };

        let mut result: Vec<Value> = Vec::new();

        if depth > 0 {
            let primary = best
                .files
                .as_ref()
                .and_then(|fs| fs.iter().find(|f| !f.download_url.is_empty()));
            if let Some(primary) = primary {
                let (name, icon_url, category) = match mr.get_project_info(&project_id).await {
                    Ok(proj) => (
                        proj.name.clone(),
                        proj.icon_url.clone().unwrap_or_default(),
                        match proj.project_type.as_deref() {
                            Some("resourcepack") => "resourcepacks".to_string(),
                            Some("shader") => "shaderpacks".to_string(),
                            _ => "mods".to_string(),
                        },
                    ),
                    Err(_) => (project_id.clone(), String::new(), "mods".to_string()),
                };
                result.push(json!({
                    "projectId": project_id,
                    "name": name,
                    "iconUrl": icon_url,
                    "versionId": best.id,
                    "versionNumber": best.version_number.clone().unwrap_or_default(),
                    "downloadUrl": primary.download_url,
                    "fileName": primary.filename,
                    "category": category,
                    "source": "modrinth",
                    "modrinthId": project_id
                }));
            }
        }

        if let Some(deps) = best.dependencies_infos.clone() {
            let required: Vec<DependenciesInfo> = deps
                .into_iter()
                .filter(|d| {
                    d.dependency_type.as_deref() == Some("required") && d.project_id.is_some()
                })
                .collect();
            for d in required {
                let sub = resolve_mr_deps(
                    mr,
                    d.project_id.unwrap_or_default(),
                    None,
                    game_version.clone(),
                    loader.clone(),
                    visited,
                    depth + 1,
                )
                .await;
                result.extend(sub);
            }
        }

        result
    })
}

/// 依赖递归解析（对齐 C# ResolveCFDeps：depth>8 截断、visited 去重、根节点取文件依赖、
/// 子节点取模组信息+最佳文件（fileDate 最大）+递归；relationType==3 为 required）。
fn resolve_cf_deps<'a>(
    cf: &'a CurseForgeBase,
    mod_id: String,
    file_id: Option<String>,
    game_version: Option<String>,
    visited: &'a mut HashSet<String>,
    depth: i32,
) -> Pin<Box<dyn std::future::Future<Output = Vec<Value>> + 'a>> {
    Box::pin(async move {
        if depth > 8 {
            return Vec::new();
        }
        if !visited.insert(mod_id.clone()) {
            return Vec::new();
        }

        let mut result: Vec<Value> = Vec::new();

        // 根节点：取文件依赖列表
        if depth == 0 {
            if let Some(file_id) = file_id {
                if let Ok(info) = cf.get_file_info(&mod_id, &file_id).await {
                    if let Some(deps) = info.dependencies {
                        for d in deps.into_iter().filter(|d| d.r#type == 3) {
                            let sub = resolve_cf_deps(
                                cf,
                                d.mod_id.to_string(),
                                None,
                                game_version.clone(),
                                visited,
                                depth + 1,
                            )
                            .await;
                            result.extend(sub);
                        }
                    }
                }
                return result;
            }
        }

        // 子节点：模组信息 + 最佳文件（文件列表按 fileDate 倒序取首个）
        let (mod_name, mod_icon) = match cf.get_mod_info(&mod_id).await {
            Ok(info) => (
                info.name.clone(),
                info.screenshots
                    .as_ref()
                    .and_then(|s| s.first())
                    .and_then(|s| s.thumbnail_url.clone())
                    .unwrap_or_default(),
            ),
            Err(_) => (mod_id.clone(), String::new()),
        };

        let gv_opt = game_version.as_deref().filter(|g| !g.is_empty());
        match cf.get_file_page(&mod_id, 0, 50, gv_opt).await {
            Ok(page) => {
                let best = page
                    .files
                    .into_iter()
                    .max_by(|a, b| a.file_date.cmp(&b.file_date));
                if let Some(best) = best {
                    let best_file_id = best.id.to_string();
                    result.push(json!({
                        "projectId": mod_id,
                        "name": mod_name,
                        "iconUrl": mod_icon,
                        "versionId": best_file_id,
                        "versionNumber": best.display_name.clone().unwrap_or_else(|| best.file_name.clone().unwrap_or_default()),
                        "downloadUrl": best.download_url.clone().unwrap_or_default(),
                        "fileName": best.file_name.clone().unwrap_or_else(|| best.display_name.clone().unwrap_or_default()),
                        "category": "mod",
                        "source": "curseforge",
                        "curseForgeId": mod_id
                    }));

                    if let Ok(info) = cf.get_file_info(&mod_id, &best_file_id).await {
                        if let Some(deps) = info.dependencies {
                            for d in deps.into_iter().filter(|d| d.r#type == 3) {
                                let sub = resolve_cf_deps(
                                    cf,
                                    d.mod_id.to_string(),
                                    None,
                                    game_version.clone(),
                                    visited,
                                    depth + 1,
                                )
                                .await;
                                result.extend(sub);
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }

        result
    })
}

/// 对应 Kotlin CoreBridge.resourceDependencies(id: String, source: String, versionId: String, gameVersion: String, loader: String, apiKey: String): String
/// modrinth / curseforge 支持递归解析；ftb 返回空数组。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_resourceDependencies(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    source: JString,
    version_id: JString,
    game_version: JString,
    loader: JString,
    api_key: JString,
) -> jstring {
    let id = get_string(&mut env, id);
    let source = get_string(&mut env, source);
    let version_id = get_string(&mut env, version_id);
    let game_version = get_string(&mut env, game_version);
    let loader = get_string(&mut env, loader);
    let api_key = get_string(&mut env, api_key);

    let result: Value = match source.as_str() {
        "modrinth" => {
            let mr = ModrinthBase::new(reqwest::Client::new(), None);
            let mut visited: HashSet<String> = HashSet::new();
            json!(block_on(resolve_mr_deps(
                &mr,
                id,
                if version_id.is_empty() { None } else { Some(version_id) },
                if game_version.is_empty() { None } else { Some(game_version) },
                if loader.is_empty() { None } else { Some(loader) },
                &mut visited,
                0,
            )))
        }
        "curseforge" => {
            if api_key.is_empty() {
                err_json("CurseForge API Key 未配置".to_string())
            } else {
                let cf = CurseForgeBase::new(reqwest::Client::new(), api_key, None);
                let mut visited: HashSet<String> = HashSet::new();
                json!(block_on(resolve_cf_deps(
                    &cf,
                    id,
                    if version_id.is_empty() { None } else { Some(version_id) },
                    if game_version.is_empty() { None } else { Some(game_version) },
                    &mut visited,
                    0,
                )))
            }
        }
        "ftb" => json!([]),
        _ => err_json(format!("资源源 {source} 暂不支持")),
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.curseForgeVersions(id: String, gameVersion: String, loader: String, apiKey: String): String
/// CurseForge 全量版本列表（同步拉取，/resources/{id}/versions/start-fetch 用），
/// 返回前端 ResourceVersion 数组。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_curseForgeVersions(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    game_version: JString,
    loader: JString,
    api_key: JString,
) -> jstring {
    let id = get_string(&mut env, id);
    let game_version = get_string(&mut env, game_version);
    let loader = get_string(&mut env, loader);
    let api_key = get_string(&mut env, api_key);
    let result: Value = if api_key.is_empty() {
        err_json("CurseForge API Key 未配置".to_string())
    } else {
        let cf = CurseForgeBase::new(reqwest::Client::new(), api_key, None);
        block_on(cf_versions_json(&cf, &id, &game_version, &loader))
    };
    to_jstring(&mut env, result.to_string())
}

/// 对应 Kotlin CoreBridge.ftbVersionDetail(packId: String, versionId: String): String
/// FTB 版本详情 JSON（camelCase，/resources/ftb/{id}/export 用）；失败返回 JSON null。
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_ftbVersionDetail(
    mut env: JNIEnv,
    _class: JClass,
    pack_id: JString,
    version_id: JString,
) -> jstring {
    let pack_id = get_string(&mut env, pack_id);
    let version_id = get_string(&mut env, version_id);
    let result: Value = match (pack_id.parse::<i32>(), version_id.parse::<i32>()) {
        (Ok(p), Ok(v)) => {
            let ftb = FtbBase::new(reqwest::Client::new(), None, None);
            match block_on(ftb.get_version_detail(p, v)) {
                Ok(Some(detail)) => serde_json::to_value(detail).unwrap_or(Value::Null),
                _ => Value::Null,
            }
        }
        _ => Value::Null,
    };
    to_jstring(&mut env, result.to_string())
}

/// 本地文件元数据 → 前端 ModMetadata / ResourcePackMetadata / ShaderMetadata / DataPackMetadata。
fn local_meta_json(
    file_path: &str,
    name: &str,
    description: &str,
    version: &str,
    authors: Vec<String>,
    curse_forge_id: i32,
    modrinth_id: &str,
    pack_format: i32,
) -> Value {
    let file_name = Path::new(file_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let source = if curse_forge_id > 0 {
        "curseforge"
    } else if !modrinth_id.is_empty() {
        "modrinth"
    } else {
        ""
    };
    let mut obj = json!({
        "fileName": file_name,
        "name": name,
        "description": description,
        "version": version,
        "authors": authors,
        "curseForgeId": if curse_forge_id > 0 { json!(curse_forge_id) } else { Value::Null },
        "modrinthId": if modrinth_id.is_empty() { Value::Null } else { json!(modrinth_id) },
        "source": if source.is_empty() { Value::Null } else { json!(source) },
        "iconUrl": Value::Null,
        "iconBase64": Value::Null
    });
    if pack_format > 0 {
        obj["packFormat"] = json!(pack_format);
    }
    obj
}

/// 对应 Kotlin CoreBridge.instanceFilesMetadata(gameDir, versionName, category, versionSegmented): String
/// category: "mods" | "resourcepacks" | "shaders" | "datapacks"
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_qomicex_launcher_core_CoreBridge_instanceFilesMetadata(
    mut env: JNIEnv,
    _class: JClass,
    game_dir: JString,
    version_name: JString,
    category: JString,
    version_segmented: JString,
) -> jstring {
    let game_dir = get_string(&mut env, game_dir);
    let version_name = get_string(&mut env, version_name);
    let category = get_string(&mut env, category);
    let version_segmented = get_string(&mut env, version_segmented) == "true";

    let factory = DefaultLocalResourcesFactory::new(reqwest::Client::new(), game_dir);
    let result: Value = match category.as_str() {
        "mods" => match block_on(factory.create_mods(&version_name, version_segmented, "").get_mod_list(None)) {
            Ok(list) => json!(list.iter().map(|m| {
                let mut j = local_meta_json(
                    &m.file_path,
                    &m.name,
                    &m.description,
                    &m.version,
                    m.authors.clone(),
                    m.curse_forge_id,
                    &m.modrinth_id,
                    0,
                );
                j["active"] = json!(m.is_active());
                j["mcmodId"] = Value::Null;
                j["chineseName"] = Value::Null;
                j["fileSize"] = Value::Null;
                j["lastModified"] = Value::Null;
                j
            }).collect::<Vec<Value>>()),
            Err(e) => err_json(format!("{e}")),
        },
        "resourcepacks" => match block_on(factory.create_resourcepack(&version_name, version_segmented, "").get_resource_pack_list()) {
            Ok(list) => json!(list.iter().map(|r| {
                local_meta_json(
                    &r.file_path,
                    &r.name,
                    &r.description,
                    &r.version,
                    Vec::new(),
                    r.curse_forge_id,
                    &r.modrinth_id,
                    r.pack_format,
                )
            }).collect::<Vec<Value>>()),
            Err(e) => err_json(format!("{e}")),
        },
        "shaders" => match block_on(factory.create_shaders(&version_name, version_segmented, "").get_shader_list()) {
            Ok(list) => json!(list.iter().map(|s| {
                local_meta_json(
                    &s.file_path,
                    &s.name,
                    &s.description,
                    &s.version,
                    Vec::new(),
                    s.curse_forge_id,
                    &s.modrinth_id,
                    0,
                )
            }).collect::<Vec<Value>>()),
            Err(e) => err_json(format!("{e}")),
        },
        "datapacks" => match block_on(factory.create_data_packs(&version_name, version_segmented, "").get_data_pack_list()) {
            Ok(list) => json!(list.iter().map(|d| {
                local_meta_json(
                    &d.file_path,
                    &d.name,
                    &d.description,
                    &d.version,
                    Vec::new(),
                    d.curse_forge_id,
                    &d.modrinth_id,
                    d.pack_format,
                )
            }).collect::<Vec<Value>>()),
            Err(e) => err_json(format!("{e}")),
        },
        _ => err_json(format!("未知资源分类 {category}")),
    };
    to_jstring(&mut env, result.to_string())
}
