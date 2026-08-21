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
