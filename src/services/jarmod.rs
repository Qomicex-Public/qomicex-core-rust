//! JarMod 注入（issue #180）。
//!
//! # 背景
//!
//! Technic 古董整合包（≈1.4.x 及更早、以及部分 1.5.2/1.6.4 包）以 **JarMod** 形式
//! 分发：整合包内的 `bin/modpack.jar`（真正的 Forge universal + FML）需要与游戏
//! 主 jar **合并**后启动，而不是作为普通 mod 放进 `mods/`。含 `version.json` 的
//! 标准包不需要走这条路（见 `services::technic` 的期1 管线）。
//!
//! # 为什么不能直接改写主 jar（实测否决策略）
//!
//! `services/version/locator.rs` 的 `get_miss_main_jar` 按 `downloads.client.sha1`
//! 强校验主 jar，不匹配就重新下载覆盖；该检查在**启动前**（`endpoints/instance.rs`）
//! 与**装完后**（`services/install_service.rs`）各跑一次。因此「把 jarmod 内容合并进
//! `versions/{VDN}/{VDN}.jar`」的方案会被静默抹掉（或迫使整体放弃完整性校验）。
//!
//! # 采用策略：非破坏性派生 jar
//!
//! 保持主 jar **原封不动**（它是 SHA1 校验对象），把「主 jar + jarmods」合并到一个
//! **派生文件** `versions/{VDN}/{VDN}-jarmod.jar`，启动时由
//! `services/launch/jvm_args.rs` 在该文件存在且版本 JSON 声明了 `jarmods` 时**优先
//! 选用**它。合并语义对齐 Prism `MMCZip.cpp`：
//!
//! 1. jarmod 条目**在先**，原版（主 jar）条目在后；
//! 2. **同名条目以先出现者为准**（跳重复）→ jarmod 覆盖原版同名类；
//! 3. 处理原版条目时**过滤 `META-INF/`**（签名文件与旧 Forge 的 MANIFEST 冲突）。
//!
//! # 版本 JSON 契约
//!
//! ```json
//! { "jarmods": ["jarmods/modpack.jar"] }
//! ```
//!
//! 路径**相对于版本目录**（`versions/{VDN}/`），与 version JSON 自身同基准；绝对路径
//! 也接受。空数组 / 缺失 / 全为不可读文件时视为无 jarmod（启动行为逐字不变）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::Error;

/// 版本 JSON 里的 jarmod 列表键名。
///
/// `jarmods` 是本仓库（后端 technic 导入）写入的形态；`jarMods` 是 MultiMC/Prism
/// 组件 patch 的字段名，由 `services/multimc.rs` 的 `apply_patch` 以「未知键」透传
/// 进合并后的版本 JSON。
///
/// ⚠️ **只支持字符串数组形态**（`["jarmods/x.jar"]`，路径相对版本目录）。MMC 原生
/// 写的是**库对象**（带 `name`/`MMC-hint`/`MMC-filename`，需按 maven 规则解析落盘
/// 路径），那种形态**尚未支持**——见 [`jarmods_from_json`] 的告警处理：遇到对象元素
/// 会打印明确原因而不是静默忽略（静默忽略会让用户以为 jarmod 生效了）。
pub const JARMOD_KEYS: [&str; 2] = ["jarmods", "jarMods"];

/// 单次合并允许读取的 jarmod 数量上限（防御性：畸形 JSON 塞入上千条会拖垮启动）。
const MAX_JARMODS: usize = 64;

/// 派生 jar 文件名后缀（`{VDN}-jarmod.jar`）。
const DERIVED_SUFFIX: &str = "-jarmod.jar";

/// 从版本 JSON 的 `Value` 提取 jarmod 列表（相对版本目录的路径）。
///
/// 返回 `None` 表示**无 jarmod**（键缺失/为 null/空数组/非数组），调用方据此跳过
/// 整条派生逻辑——这正是「对期1 标准包零影响」的保证点。
///
/// 对象元素（MMC 原生形态）**不被消费**，但会打一条告警说明原因：静默丢弃会让人误以为
/// MultiMC 实例的 jarmod 已生效，而那属于「装作支持」，比明确说不支持更糟。
pub fn jarmods_from_json(root: &serde_json::Value) -> Option<Vec<String>> {
    for key in JARMOD_KEYS {
        let Some(v) = root.get(key) else { continue };
        let Some(arr) = v.as_array() else { continue };
        if arr.iter().any(|x| x.is_object()) {
            eprintln!(
                "版本 JSON 的 `{key}` 含库对象元素（MultiMC 原生 jarMods 形态），当前仅支持\
                 字符串路径数组，该 jarmod 不会被注入；如需支持请将其转换为相对路径字符串"
            );
        }
        let list: Vec<String> = arr
            .iter()
            .filter_map(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .take(MAX_JARMODS)
            .map(String::from)
            .collect();
        if !list.is_empty() {
            return Some(list);
        }
    }
    None
}

/// 派生 jar 路径（`versions/{version}/{version}-jarmod.jar`）。
pub fn derived_jar_path(game_dir: &str, version: &str) -> PathBuf {
    Path::new(game_dir)
        .join("versions")
        .join(version)
        .join(format!("{version}{DERIVED_SUFFIX}"))
}

/// 把 jarmod 路径解析为绝对路径（相对路径以版本目录为基准）。
pub fn resolve_jarmod_path(game_dir: &str, version: &str, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    Path::new(game_dir)
        .join("versions")
        .join(version)
        .join(path)
}

/// 确保派生 jar 存在且**不比任何输入旧**；返回其路径。
///
/// 增量语义：输入（主 jar 或任一 jarmod）比派生 jar 新时重新合并。这样用户替换
/// jarmod 后无需手动删缓存，而重复启动不会反复做无谓的合并。
///
/// 无任何可读 jarmod → 返回 `Ok(None)`（调用方回退到原主 jar）。
pub fn ensure_derived_jar(
    game_dir: &str,
    version: &str,
    base_jar: &Path,
    jarmods: &[String],
) -> Result<Option<PathBuf>, Error> {
    let dest = derived_jar_path(game_dir, version);

    let inputs: Vec<PathBuf> = jarmods
        .iter()
        .map(|p| resolve_jarmod_path(game_dir, version, p))
        .filter(|p| p.is_file())
        .collect();
    if inputs.is_empty() {
        return Ok(None);
    }

    // 新鲜度：派生 jar 存在且不早于主 jar 与全部 jarmod。
    // 合并嵌套 `if let`（clippy::collapsible_if）：语义等价且更易读。
    let dest_time = std::fs::metadata(&dest)
        .ok()
        .and_then(|m| m.modified().ok());
    if let Some(dest_time) = dest_time {
        let newest_input = std::iter::once(base_jar)
            .chain(inputs.iter().map(PathBuf::as_path))
            .filter_map(|p| std::fs::metadata(p).ok())
            .filter_map(|m| m.modified().ok())
            .max();
        if newest_input.is_some_and(|t| dest_time >= t) {
            return Ok(Some(dest));
        }
    }

    merge_jars(base_jar, &inputs, &dest)?;
    Ok(Some(dest))
}

/// 合并「主 jar + jarmods」到 `dest`（语义见模块头注释）。
///
/// 先写 jarmod 条目（先出现者优先、跳重复），再写主 jar 条目（跳过已存在的名字，
/// 并过滤 `META-INF/`）。
pub fn merge_jars(base_jar: &Path, jarmods: &[PathBuf], dest: &Path) -> Result<(), Error> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::ResourceCompletion {
            message: format!("创建 jarmod 派生目录失败 {}: {e}", parent.display()),
            source: Some(Box::new(e)),
        })?;
    }
    // 先写临时文件再改名：中途失败不会留下半个 jar（否则下次启动会当成可用派生 jar）
    let tmp = dest.with_extension("jar.tmp");

    let result = (|| -> Result<(), Error> {
        let file = std::fs::File::create(&tmp).map_err(|e| Error::ResourceCompletion {
            message: format!("创建 jarmod 派生文件失败 {}: {e}", tmp.display()),
            source: Some(Box::new(e)),
        })?;
        let mut writer = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();

        // 1) jarmod 条目在先（先出现者优先 → 覆盖原版同名类）
        for jar in jarmods {
            let f = std::fs::File::open(jar).map_err(|e| Error::ResourceCompletion {
                message: format!("打开 jarmod 失败 {}: {e}", jar.display()),
                source: Some(Box::new(e)),
            })?;
            let mut archive = zip::ZipArchive::new(f).map_err(|e| Error::ResourceCompletion {
                message: format!("jarmod 不是有效的 zip: {}: {e}", jar.display()),
                source: Some(Box::new(e)),
            })?;
            copy_entries(&mut archive, &mut writer, opts, &mut written, false)?;
        }

        // 2) 原版条目在后，过滤 META-INF/（签名/MANIFEST 与 jarmod 冲突）
        let f = std::fs::File::open(base_jar).map_err(|e| Error::ResourceCompletion {
            message: format!("打开主 jar 失败 {}: {e}", base_jar.display()),
            source: Some(Box::new(e)),
        })?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| Error::ResourceCompletion {
            message: format!("主 jar 不是有效的 zip: {}: {e}", base_jar.display()),
            source: Some(Box::new(e)),
        })?;
        copy_entries(&mut archive, &mut writer, opts, &mut written, true)?;

        writer.finish().map_err(|e| Error::ResourceCompletion {
            message: format!("写入 jarmod 派生 jar 失败: {e}"),
            source: Some(Box::new(e)),
        })?;
        Ok(())
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // rename 到目标（Windows 上目标存在时先删——replace 语义）
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::rename(&tmp, dest).map_err(|e| Error::ResourceCompletion {
        message: format!("就位 jarmod 派生 jar 失败 {}: {e}", dest.display()),
        source: Some(Box::new(e)),
    })?;
    Ok(())
}

/// 把 `archive` 中所有条目写入 `writer`，`written` 记录已写名字以跳重复。
///
/// `filter_meta_inf` = true 时跳过 `META-INF/` 下的条目（仅用于原版主 jar）。
fn copy_entries<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    writer: &mut zip::ZipWriter<std::fs::File>,
    opts: zip::write::SimpleFileOptions,
    written: &mut std::collections::HashSet<String>,
    filter_meta_inf: bool,
) -> Result<(), Error> {
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| Error::ResourceCompletion {
            message: format!("读取 jar 条目失败: {e}"),
            source: Some(Box::new(e)),
        })?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        // 目录条目（名字以 / 结尾）与 META-INF 过滤
        if name.ends_with('/') {
            continue;
        }
        if filter_meta_inf && name.starts_with("META-INF/") {
            continue;
        }
        // 跳重复：jarmod 在先 → 它赢
        if !written.insert(name.clone()) {
            continue;
        }
        // 用 Stored/Deflated 重压：`start_file` 后逐字节拷贝，避免把大条目全读进内存
        let mut entry_opts = opts;
        entry_opts = entry_opts.compression_method(zip::CompressionMethod::Deflated);
        // 保留原始 unix 权限（可执行位对老 jar 内的 script 有意义）
        if let Some(mode) = entry.unix_mode() {
            entry_opts = entry_opts.unix_permissions(mode);
        }
        writer
            .start_file(name, entry_opts)
            .map_err(|e| Error::ResourceCompletion {
                message: format!("写入 jar 条目失败: {e}"),
                source: Some(Box::new(e)),
            })?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|e| Error::ResourceCompletion {
                    message: format!("读取 jar 条目内容失败: {e}"),
                    source: Some(Box::new(e)),
                })?;
            if n == 0 {
                break;
            }
            writer
                .write_all(&buf[..n])
                .map_err(|e| Error::ResourceCompletion {
                    message: format!("写入 jar 条目内容失败: {e}"),
                    source: Some(Box::new(e)),
                })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qml-jarmod-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 造一个 jar（entries: 名字 → 内容）。
    fn make_jar(path: &Path, entries: &[(&str, &str)]) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }

    /// 读 jar 全部条目（名字 → 内容）。
    fn read_jar(path: &Path) -> std::collections::HashMap<String, String> {
        let f = std::fs::File::open(path).unwrap();
        let mut a = zip::ZipArchive::new(f).unwrap();
        let mut out = std::collections::HashMap::new();
        for i in 0..a.len() {
            let mut e = a.by_index(i).unwrap();
            if e.is_dir() {
                continue;
            }
            let name = e.name().to_string();
            let mut s = String::new();
            e.read_to_string(&mut s).unwrap();
            out.insert(name, s);
        }
        out
    }

    #[test]
    fn parses_jarmods_key_forms() {
        // 两种键名都接受
        assert_eq!(
            jarmods_from_json(&json!({"jarmods": ["a.jar"]})),
            Some(vec!["a.jar".to_string()])
        );
        assert_eq!(
            jarmods_from_json(&json!({"jarMods": ["a.jar"]})),
            Some(vec!["a.jar".to_string()])
        );
        // 无 / null / 空数组 / 非数组 → None（保证标准包零影响）
        assert!(jarmods_from_json(&json!({})).is_none());
        assert!(jarmods_from_json(&json!({"jarmods": null})).is_none());
        assert!(jarmods_from_json(&json!({"jarmods": []})).is_none());
        assert!(jarmods_from_json(&json!({"jarmods": "a.jar"})).is_none());
        // 空白项被丢弃
        assert!(jarmods_from_json(&json!({"jarmods": ["  ", ""]})).is_none());
    }

    #[test]
    fn derived_path_is_version_scoped() {
        let p = derived_jar_path("/g", "MyPack");
        assert!(p.ends_with("versions/MyPack/MyPack-jarmod.jar"));
    }

    #[test]
    fn resolve_relative_and_absolute() {
        let rel = resolve_jarmod_path("/g", "P", "jarmods/m.jar");
        assert!(rel.ends_with("versions/P/jarmods/m.jar"));
        let abs = resolve_jarmod_path("/g", "P", "/tmp/x.jar");
        assert_eq!(abs, PathBuf::from("/tmp/x.jar"));
    }

    /// 合并语义：jarmod 条目在先且覆盖同名原版条目；原版 META-INF 被过滤。
    #[test]
    fn merge_prefers_jarmod_and_filters_meta_inf() {
        let d = temp_dir("merge");
        let base = d.join("base.jar");
        let jm = d.join("mod.jar");
        let out = d.join("out.jar");
        make_jar(
            &base,
            &[
                ("net/minecraft/Foo.class", "VANILLA"),
                ("only-in-base.txt", "base"),
                ("META-INF/MANIFEST.MF", "manifest"),
                ("META-INF/SIGN.SF", "sig"),
            ],
        );
        make_jar(
            &jm,
            &[
                ("net/minecraft/Foo.class", "JARMOD"),
                ("only-in-jarmod.txt", "jarmod"),
                // jarmod 里的 META-INF 不过滤（它可能自带有效清单）
                ("META-INF/MANIFEST.MF", "jarmod-manifest"),
            ],
        );

        merge_jars(&base, std::slice::from_ref(&jm), &out).unwrap();
        let got = read_jar(&out);

        // jarmod 覆盖同名类
        assert_eq!(got.get("net/minecraft/Foo.class").unwrap(), "JARMOD");
        // 双方的独有内容都在
        assert_eq!(got.get("only-in-base.txt").unwrap(), "base");
        assert_eq!(got.get("only-in-jarmod.txt").unwrap(), "jarmod");
        // jarmod 的 MANIFEST 保留（先出现者优先），原版签名被过滤
        assert_eq!(got.get("META-INF/MANIFEST.MF").unwrap(), "jarmod-manifest");
        assert!(
            !got.contains_key("META-INF/SIGN.SF"),
            "原版 META-INF 应被过滤"
        );

        std::fs::remove_dir_all(&d).ok();
    }

    /// 原版 META-INF 在没有 jarmod 覆盖时也被剔除（老 Forge 与签名文件冲突）。
    #[test]
    fn merge_filters_base_meta_inf_without_jarmod_counterpart() {
        let d = temp_dir("meta");
        let base = d.join("base.jar");
        let jm = d.join("mod.jar");
        let out = d.join("out.jar");
        make_jar(
            &base,
            &[
                ("a.class", "A"),
                ("META-INF/MANIFEST.MF", "m"),
                ("META-INF/CERT.RSA", "c"),
            ],
        );
        make_jar(&jm, &[("b.class", "B")]);

        merge_jars(&base, &[jm], &out).unwrap();
        let got = read_jar(&out);
        assert_eq!(got.get("a.class").unwrap(), "A");
        assert_eq!(got.get("b.class").unwrap(), "B");
        assert!(!got.contains_key("META-INF/MANIFEST.MF"));
        assert!(!got.contains_key("META-INF/CERT.RSA"));

        std::fs::remove_dir_all(&d).ok();
    }

    /// 无 jarmod → `ensure_derived_jar` 返回 None（**标准包零影响的关键assert**）。
    #[test]
    fn ensure_returns_none_when_no_readable_jarmods() {
        let d = temp_dir("none");
        let base = d.join("base.jar");
        make_jar(&base, &[("a.class", "A")]);
        // 空列表
        assert!(
            ensure_derived_jar(d.to_str().unwrap(), "V", &base, &[])
                .unwrap()
                .is_none()
        );
        // 列表里有项但文件不存在
        assert!(
            ensure_derived_jar(
                d.to_str().unwrap(),
                "V",
                &base,
                &["missing.jar".to_string()]
            )
            .unwrap()
            .is_none()
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// 派生 jar 落盘且内容正确；重复调用不重做（新鲜度检查）。
    #[test]
    fn ensure_creates_and_reuses_derived_jar() {
        let d = temp_dir("ensure");
        let version = "V";
        let vdir = d.join("versions").join(version);
        let base = vdir.join("V.jar");
        make_jar(&base, &[("vanilla.class", "V")]);
        let jm = vdir.join("jarmods").join("m.jar");
        make_jar(&jm, &[("mod.class", "M")]);

        let out = ensure_derived_jar(
            d.to_str().unwrap(),
            version,
            &base,
            &["jarmods/m.jar".to_string()],
        )
        .unwrap()
        .expect("应生成派生 jar");
        assert!(out.is_file());
        let got = read_jar(&out);
        assert_eq!(got.get("vanilla.class").unwrap(), "V");
        assert_eq!(got.get("mod.class").unwrap(), "M");

        // 第二次调用：返回同一路径，且不因重建而改变 mtime 语义（内容一致即可）
        let again = ensure_derived_jar(
            d.to_str().unwrap(),
            version,
            &base,
            &["jarmods/m.jar".to_string()],
        )
        .unwrap()
        .unwrap();
        assert_eq!(again, out);
        assert_eq!(read_jar(&again).get("mod.class").unwrap(), "M");

        std::fs::remove_dir_all(&d).ok();
    }

    /// 失败不留半个派生 jar（下次启动不得把它当可用）。
    #[test]
    fn failed_merge_leaves_no_derived_file() {
        let d = temp_dir("fail");
        let base = d.join("base.jar");
        let out = d.join("out.jar");
        // 主 jar 不是合法 zip → 合并失败
        std::fs::write(&base, b"not a zip").unwrap();
        let bad = d.join("bad.jar");
        std::fs::write(&bad, b"also not a zip").unwrap();
        assert!(merge_jars(&base, &[bad], &out).is_err());
        assert!(!out.exists(), "失败后不应留下派生 jar");
        assert!(
            !out.with_extension("jar.tmp").exists(),
            "失败后不应留下临时文件"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// 文件内容哈希（测试用，复用 crate 已有的 sha1 依赖）。
    fn content_hash(p: &Path) -> String {
        use sha1::Digest as _;
        let bytes = std::fs::read(p).unwrap();
        let mut h = sha1::Sha1::new();
        h.update(&bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// **核心集成断言**：版本 JSON 声明 jarmods 时，派生 jar 与**字节不变的主 jar** 并存，
    /// 且派生 jar 才是「主 jar + jarmod」的合并产物。
    ///
    /// 这是 `jvm_args.rs` 选用派生 jar 的前提：那段逻辑只做
    /// 「`config.jarmods` 非空 → `ensure_derived_jar(..)` 返回的路径顶替主 jar」，
    /// 因此这里断言的就是该分支真正依赖的事实。
    #[test]
    fn derived_jar_coexists_with_untouched_base_jar() {
        let d = temp_dir("coexist");
        let version = "MyAncient";
        let vdir = d.join("versions").join(version);
        let base = vdir.join(format!("{version}.jar"));
        // 原版主 jar：仅含 vanilla 类 + META-INF
        make_jar(
            &base,
            &[
                ("net/minecraft/client/Minecraft.class", "VANILLA"),
                ("META-INF/MANIFEST.MF", "base-manifest"),
            ],
        );
        let base_hash_before = content_hash(&base);

        // 古董包的 modpack.jar（含 FML 类，且与主 jar 有同名类）
        let jm = vdir.join("jarmods").join("modpack.jar");
        make_jar(
            &jm,
            &[
                ("net/minecraft/client/Minecraft.class", "JARMOD-OVERWRITE"),
                ("cpw/mods/fml/common/launcher/FMLTweaker.class", "FML"),
            ],
        );

        // 版本 JSON 声明 jarmods（后端 `install_technic_jarmod` 写入的形态）
        let json_path = vdir.join(format!("{version}.json"));
        std::fs::write(
            &json_path,
            serde_json::json!({
                "id": version,
                "mainClass": "net.minecraft.launchwrapper.Launch",
                "jarmods": ["jarmods/modpack.jar"]
            })
            .to_string(),
        )
        .unwrap();

        // 解析 + 派生（复刻 jvm_args 的两步）
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        let jarmods = jarmods_from_json(&root).expect("应解析出 jarmods");
        let derived = ensure_derived_jar(d.to_str().unwrap(), version, &base, &jarmods)
            .unwrap()
            .expect("应生成派生 jar");

        // 1) 主 jar 未被改动（SHA1 校验对象安全 —— 改动会被完整性校验判为损坏并重下）
        assert_eq!(content_hash(&base), base_hash_before, "主 jar 必须字节不变");
        // 2) 派生 jar 独立于主 jar，命名符合约定
        assert_ne!(derived, base);
        assert_eq!(derived, vdir.join(format!("{version}-jarmod.jar")));
        // 3) 派生 jar 含 jarmod 的 FML 类，且同名类被 jarmod 覆盖
        let got = read_jar(&derived);
        assert_eq!(
            got.get("cpw/mods/fml/common/launcher/FMLTweaker.class")
                .unwrap(),
            "FML",
            "派生 jar 必须含 jarmod 的 FML 类（否则古董包起不来）"
        );
        assert_eq!(
            got.get("net/minecraft/client/Minecraft.class").unwrap(),
            "JARMOD-OVERWRITE",
            "同名类应由 jarmod 覆盖原版"
        );

        std::fs::remove_dir_all(&d).ok();
    }

    /// `jarmods` 键缺失 → `jarmods_from_json` 返回 None，启动链走原逻辑
    /// （**期1 标准包零影响**的另一侧断言）。
    #[test]
    fn no_jarmods_key_means_no_derivation() {
        let root: serde_json::Value =
            serde_json::from_str(r#"{"id":"X","mainClass":"net.minecraft.client.main.Main"}"#)
                .unwrap();
        assert!(jarmods_from_json(&root).is_none());
    }

    /// MMC 原生 `jarMods` 是**库对象**数组，当前不支持 → 必须返回 None（不静默当成
    /// 空路径去拼文件系统），且不能 panic。
    ///
    /// 这条测试固化「如实不支持」的口径：与其把 `{"name":...}` 误当路径拼进文件系统，
    /// 不如不产生派生 jar，并在 stderr 说明原因（`jarmods_from_json` 内的告警）。
    #[test]
    fn mmc_object_form_is_not_silently_treated_as_path() {
        let root: serde_json::Value = serde_json::from_str(
            r#"{"jarMods":[{"name":"com.example:jarmod:1.0","MMC-hint":"local","MMC-filename":"x.jar"}]}"#,
        )
        .unwrap();
        assert!(
            jarmods_from_json(&root).is_none(),
            "MMC 对象形态不应被当成路径（否则会拿 JSON 文本去拼文件路径）"
        );
    }

    /// 混合形态：对象元素被跳过，字符串元素仍正常工作（不因一个坏元素丢掉好元素）。
    #[test]
    fn mixed_form_keeps_string_entries() {
        let root: serde_json::Value =
            serde_json::from_str(r#"{"jarmods":[{"name":"obj"}, "jarmods/real.jar"]}"#).unwrap();
        assert_eq!(
            jarmods_from_json(&root),
            Some(vec!["jarmods/real.jar".to_string()])
        );
    }
}
