// Copyright (C) 2026 Qomicex Launcher Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! 整合包安装器（B13）
//!
//! 对应源目录：Qomicex.Core.AOT/Services/Installers/Modpacks/
//! - CurseForgeModpackInstaller.cs（109 行）→ curseforge.rs
//! - ModrinthModpackInstaller.cs（100 行）→ modrinth.rs
//! - FTBModpackInstaller.cs（77 行）→ ftb.rs
//! - ModpackModels.cs（40 行）→ models.rs
//!
//! 三个安装器均实现 `Installer` trait（installer.rs，B9 契约）；
//! 由 InstallerFactory（factory.rs）的 3 个 create_modpack 方法创建
//! （工厂接线在 B13 后续步骤完成，当前仍为占位实现）。
//! 本目录整体在 services/ 实现层，全部 `pub(crate)`。

pub mod curseforge;
pub mod ftb;
pub mod modrinth;

/// 把 zip 条目名解析成 `base` 之下的安全路径；越界（路径穿越 / 绝对路径）→ None。
///
/// 整合包的 overrides 条目名来自**不可信的 zip**：`overrides/../../x` 这类名字在去掉
/// 前缀后仍是相对路径，直接 `base.join()` 会写到安装目录之外。这里逐段过滤，
/// 拒绝 `..`、盘符段与空段，并在拼完后复核结果仍在 `base` 之内。
///
/// `C:payload` 这类**盘符相对**段也必须拒绝：`PathBuf::push` 遇到它会把整条路径替换成
/// 「当前 C 盘目录下的 payload」，`starts_with(base)` 拦不住（评审 sourcery 指出）。
///
/// 段尾的点与空格同样要按 Windows 口径归一后再判：`..junk`、`...`、`..  ` 在 Win32 上会
/// 被规范化成父目录段，只看字面量 `== ".."` 会漏（评审 sourcery 指出）。
pub(crate) fn safe_join_under(
    base: &std::path::Path,
    relative_path: &str,
) -> Option<std::path::PathBuf> {
    let mut out = base.to_path_buf();
    // 两种分隔符都可能是分隔符（zip 规范要求 `/`，实际样本里 `\` 也出现）
    for segment in relative_path.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        // Windows 的段规范化会剥掉尾部的点与空格，所以 `..junk`、`...`、`..  ` 都要先归一。
        // 归一后的形态只有三种可能：空（纯点/空格，等价 `.`）、以点开头（父目录段）、正常名。
        let normalized = segment.trim_end_matches(['.', ' ']);
        if normalized.is_empty() {
            // 纯点/空格段：`...` / `   ` 归一后为空，等价于 `.` → 丢弃；
            // 但 `..` 本身也会被剥成空串，不能当无害段放过，故单独确认没有两个连续点
            if segment.contains("..") {
                return None;
            }
            continue;
        }
        // `..`、`..junk` 归一后仍以 `..` 开头 → 父目录段
        if normalized.starts_with("..") {
            return None;
        }
        // 盘符前缀（`C:`、`C:payload`）与 UNC 共享名都是绝对/盘符相对的起点
        if has_drive_prefix(segment) {
            return None;
        }
        out.push(segment);
    }
    if out.starts_with(base) {
        Some(out)
    } else {
        None
    }
}

/// 该段是否带 Windows 盘符前缀（`C:` 或 `C:rest`）。
fn has_drive_prefix(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// zip 条目名是否为目录条目（按 `/` 或 `\` 结尾）。
///
/// 必须两种分隔符都认：`overrides/subdir\` 若只按 `/` 取末段会得到非空文件名，
/// 随后被当成文件用 `File::create` 创建，等它的子条目写入时才失败（评审 sourcery 指出）。
pub(crate) fn is_directory_entry(entry_name: &str) -> bool {
    entry_name.ends_with(['/', '\\'])
}

#[cfg(test)]
mod tests {
    use super::safe_join_under;
    use std::path::Path;

    #[test]
    fn safe_join_under_rejects_traversal() {
        let base = Path::new("install");
        // 正常条目照常解析
        assert_eq!(
            safe_join_under(base, "mods/a.jar"),
            Some(base.join("mods").join("a.jar"))
        );
        // 路径穿越一律拒绝（评审 sourcery 指出：source 直接 Combine 会写穿安装目录）
        for evil in [
            "../escape.jar",
            "mods/../../escape.jar",
            "mods/../../../etc/passwd",
            "..\\escape.jar",
            "mods\\..\\..\\escape.jar",
        ] {
            assert!(
                safe_join_under(base, evil).is_none(),
                "{evil:?} 属越界路径，应被拒绝"
            );
        }
        // Windows 盘符段不允许（否则 `C:x` 会脱离 base）
        assert!(safe_join_under(base, "C:/x.jar").is_none());
        // 盘符相对段（`C:payload`）也必须拒绝：PathBuf::push 会整条替换，
        // 末尾的 starts_with(base) 拦不住（评审 sourcery 指出）
        for evil in ["C:payload", "mods/D:escape.jar", "C:payload/x.jar"] {
            assert!(
                safe_join_under(base, evil).is_none(),
                "{evil:?} 属盘符相对路径，应被拒绝"
            );
        }
        // 空段与 `.` 折叠掉，不越界
        assert_eq!(
            safe_join_under(base, "./mods//a.jar"),
            Some(base.join("mods/a.jar"))
        );
    }

    #[test]
    fn safe_join_under_rejects_windows_normalized_parent_segments() {
        // Win32 的段规范化会剥掉尾部的点与空格，`..junk`、`.. `、`....` 都会被当成父目录段；
        // 只比对字面量 `== ".."` 会漏（评审 sourcery 指出）。
        let base = Path::new("install");
        for evil in [
            "..junk/x.jar",
            ".. /x.jar",
            "mods/.../../x.jar",
            "mods/.. /x.jar",
        ] {
            assert!(
                safe_join_under(base, evil).is_none(),
                "{evil:?} 归一后是父目录段，应被拒绝"
            );
        }
        // `...` 含连续两点，按 Win32 口径同样是父目录段 → 拒绝而不是放过
        assert!(
            safe_join_under(base, "mods/.../a.jar").is_none(),
            "`...` 含连续两点，应按父目录段拒绝"
        );
        // 纯空格的段归一后为空、且不含连续点 → 等价于 `.`，折叠掉
        assert!(
            safe_join_under(base, "mods/   /a.jar").is_some(),
            "纯空格段应折叠掉而不是拒绝"
        );
    }

    #[test]
    fn is_directory_entry_accepts_both_separators() {
        use super::is_directory_entry;
        // 两种分隔符结尾都算目录条目；否则 `subdir\` 会被当文件创建，
        // 等子条目写入时才失败（评审 sourcery 指出）
        assert!(is_directory_entry("overrides/subdir/"));
        assert!(is_directory_entry("overrides/subdir\\"));
        assert!(!is_directory_entry("overrides/subdir/file.jar"));
        assert!(!is_directory_entry("overrides/file.jar"));
    }

    #[test]
    fn has_drive_prefix_covers_drive_relative_segments() {
        use super::has_drive_prefix;
        // 盘符相对段（`C:payload`）必须被识别：这类段交给 PathBuf::push 会整条替换。
        // 在 Windows 上末尾的 starts_with 恰好也能拦，但依赖路径当前形态并不可靠
        // （base 为绝对路径时行为不同），故显式判定。
        for evil in ["C:", "C:payload", "D:escape.jar", "z:dir"] {
            assert!(has_drive_prefix(evil), "{evil:?} 应判为盘符前缀");
        }
        for ok in ["mods", "a.jar", "C", "dir:", ""] {
            assert!(!has_drive_prefix(ok), "{ok:?} 不应判为盘符前缀");
        }
    }
}
