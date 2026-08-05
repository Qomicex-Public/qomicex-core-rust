# CHECKPOINT_BATCH_13.md — 扩展平台 + 整合包 + builder 完整组装（最终批次）

- 日期：2026-08-06
- 分支：migrate/b13
- 范围：B13 扩展平台 / 整合包安装器 / InstallerProvider 补齐 / builder 组装 / 清理
- 状态：✅ 完成（cargo check 零警告，35 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P55 | ModrinthBase（135 行） | expansion/modrinth/query.rs（9 方法 + escape_data_string） | ✅ |
| P56 | CurseForgeBase（250 行） | expansion/curseforge/query.rs（指纹反查/批量文件/搜索） | ✅ |
| P57 | FTBBase（213 行） | expansion/ftb/query.rs（缓存 TTL 3600s/7 种排序/详情链） | ✅ |
| P58 | Modpack 安装器 3 种（286 行） | modpacks/{curseforge,modrinth,ftb}.rs（overrides 解压/依赖下载） | ✅ |
| P60+P61 | InstallerProvider（962 行，B9 遗漏补移植） | provider.rs（1115 行，Fabric 系 7 加载器）+ provider_forge.rs（Forge/NeoForge/Cleanroom + HTML 解析 + 缓存） | ✅ |
| P59 | GameCoreBuilder.Build | builder.rs 完整组装（13 默认服务 + 11 注入 + AuthMode 分派） | ✅ |
| 整合 | DefaultGameCore | core.rs：http 字段 + CreateModrinthSource/CreateCurseForgeSource/CreateFTBSource | ✅ |

## 关键实现

1. **InstallerProvider 全 9 加载器版本查询**：Fabric/Quilt/OptiFine/LiteLoader/LegacyFabric/Babric（meta API）+ NeoForge（官方双端点/BMCLAPI）+ **Forge（BMCLAPI JSON + 官方 HTML 表格正则解析 + %TEMP%/ForgeVersionCache 24h 缓存 + 推荐版本标记）** + Cleanroom（GitHub releases）
2. **版本匹配体系**：NormalizeMinecraftVersion（快照 22+ 补 1.）/别名/SupportsMinecraftVersion/IsVersionBelowOrEqual/SortAndDeduplicate/WithTimeout(10s)
3. **builder 完整组装**：http(UA) → InstallerBase UA → 下载源 → VersionManagement → Auth（3 模式分派）→ Launch → Java → InstallerProvider → Locator → Factory → Local → Options（三路径条件）→ Server → GameCore::new 13 参
4. **Modpack 安装器**：CF/MR zip manifest + overrides/override 目录解压 + 版本隔离；FTB 走 API + CF 批量文件查询
5. **core 3 工厂方法**（CreateXxxSource）已接线

## 排障记录

- Modpack 占位替换首次失败（乱码消息不匹配）→ edit 工具重做
- dead_code 清理 22→0：删除冗余（retry.rs、parse_game_json_config 重复版、InstallType、DownloadSource、ModpackModels、占位 struct）+ 可见性修正（installer_factory pub）
- provider_forge 接线（P61 待办落地）：3 stub → 模块函数调用
- ftb/forge 字段/方法清理 15 处

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：35/35（回归）

## 下一步

**收尾**：QA 快照比对 + ADR 汇总 + push 远端
