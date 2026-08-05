# CHECKPOINT_BATCH_9.md — 安装器实现

- 日期：2026-08-06
- 分支：migrate/b9
- 范围：B9 services/installers（9 种安装器 + 工厂 + 基础契约）
- 状态：✅ 完成（cargo check 零警告，35 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P35 | IInstaller + IInstallerFactory + InstallerBase + MissFileData | installer.rs（Installer trait/InstallType/InstallerBase 11 工具/MissFileData）+ api/installer.rs 追加 InstallerFactory trait | ✅ |
| P36 | FabricInstaller + QuiltInstaller | fabric/install.rs + quilt/install.rs | ✅ |
| P37 | LegacyFabricInstaller + BabricInstaller | legacy_fabric.rs + babric.rs | ✅ |
| P38 | ForgeInstallerBase | forge_base.rs（568 行：run_processor/ResolveUrl/SourcesList 等） | ✅ |
| P39 | ForgeInstaller（Legacy+New） | forge/install.rs（680 行：双流程/processors/lzma/BINPATCH/回滚） | ✅ |
| P40 | NeoForgeInstaller | neoforge/install.rs（717 行） | ✅ |
| P41 | LiteloaderInstaller | liteloader/install.rs（726 行） | ✅ |
| P42 | OptiFineInstaller + CleanroomInstaller | optifine/install.rs + cleanroom.rs | ✅ |
| P43 | DefaultInstallerFactory + core 字段 | factory.rs（12 分派）+ core.rs installer_factory | ✅ |

## 关键实现

1. **Forge 双流程**（特殊兼容）：New（install_profile/processors/lzma/BINPATCH.client 改写/缺失库 '|' 多源 HEAD 探测）+ Legacy（versionInfo 回退）+ BackInstall 回滚
2. **run_processor**：幂等输出预检 → jar/classpath Maven 坐标下载 → args 占位符替换 → RunInstallProcess → 输出 SHA1 复检
3. **Fabric/Quilt/LegacyFabric/Babric**：meta API 版本链 + 镜像域名替换 + merge_version_json + SHA1 缺失检查
4. **LiteLoader**：版本链（snapshots|artefacts 命中即停）+ tweakClass 缺省 + merge 深合并
5. **OptiFine**：sourceId==0 判定官方（与 Fabric 的 ==1 不同！）+ Patcher 执行（java -cp 逐行日志）+ tweakClass
6. **工厂**：12 方法分派；**Modpack 3 方法占位**（B13 实现，大声失败）
7. **InstallerFactory/Installer/MissFileData 提升 pub**（private_interfaces lint 排障）

## 依赖/可见性

- 无新依赖；Installer/InstallerFactory/MissFileData pub（api 层公开契约）
- core.rs：installer_factory 字段 Arc<dyn InstallerFactory + Send + Sync>（过渡 allow，P22 移除）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：35/35（回归）

## 下一步

B10：本地内容管理实现（6 管理器 + 占位 trait 替换）
