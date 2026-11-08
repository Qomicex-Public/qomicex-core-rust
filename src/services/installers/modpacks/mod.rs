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
pub(crate) fn safe_join_under(
    base: &std::path::Path,
    relative_path: &str,
) -> Option<std::path::PathBuf> {
    let mut out = base.to_path_buf();
    for segment in relative_path.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return None;
        }
        // Windows 盘符（`C:`）不是合法子目录名，允许它就会脱离 base
        if segment.len() == 2 && segment.as_bytes()[1] == b':' {
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
        // 空段与 `.` 折叠掉，不越界
        assert_eq!(
            safe_join_under(base, "./mods//a.jar"),
            Some(base.join("mods/a.jar"))
        );
    }
}
