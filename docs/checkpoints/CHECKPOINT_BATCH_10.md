# CHECKPOINT_BATCH_10.md — 本地内容管理实现

- 日期：2026-08-06
- 分支：migrate/b10
- 范围：B10 services/local（6 管理器 + 工厂 + 基类）
- 状态：✅ 完成（cargo check 零警告，35 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P44 | DefaultLocalModsFactory + LocalResourceBase | factory.rs（工厂 6 分派 + 基类哈希委托 + try_read_file_from_zip） | ✅ |
| P45 | Mods.cs（240 行） | mods.rs（fabric/forge/mcmod 三解析器 + 启禁 + CF 指纹） | ✅ |
| P46 | Resourcepacks.cs + Shaders.cs | resourcepacks.rs + shaders.rs（mcmeta/图标/哈希 + 目录包重压缩） | ✅ |
| P47 | DataPacks.cs + Screenshots.cs | datapacks.rs + screenshots.rs | ✅ |
| P48 | Saves.cs（518 行） | saves.rs（全量 NBT 解析器内嵌 + 重命名/备份） | ✅ |
| 契约 | — | api/local.rs 6 个 Manager trait 真实签名（主控定案，async_trait 修正） | ✅ |

## 关键实现

1. **Mod 元数据三解析器**：fabric.mod.json（authors/icon/SHA1）、META-INF/mods.toml（toml crate）、mcmod.info；`${file.jarVersion}` → MANIFEST.MF；CF 指纹（murmurhash2 委托）
2. **Saves**：level.dat 全量 NBT 解析（**内嵌 10 种 tag 解析器**——util/nbt.rs 仅 4 种类型无法解析真实 level.dat 的 TAG_Long；Vec 保插入序保证写回字节布局一致；gzip 魔数探测 + 写出恒压缩）；RenameSave（LevelName 改写失败回滚 + panic! 忠实映射源 throw）；BackupSave（_backup_ 时间戳命名，已存在直接返回）
3. **pack.mcmeta**：zip/folder 双形态（目录包递归收集 → 内存 ZIP Deflated → 哈希），图标读取，description 语义
4. **进度回调**：`Option<&mut (dyn FnMut(i32, i32) + Send)>`（P45 的 unsafe 转写已移除——trait 修正后直接调用）
5. **命名统一**：Resourcepack/Shaders → ResourcepackService/ShadersService（P44-P46 冲突解决）

## 依赖

B10 引入：toml 0.8、base64 0.22、flate2 1、chrono（clock only）

## 已知占位

- CF/MR 哈希反查 + 图标下载（B13 query.rs 接线，http/api_key 字段保留）
- 目录包 ZIP 重压缩哈希字节与 .NET 产物不保证一致（QA 关注点）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：35/35（回归）

## 下一步

B11：服务器域实现（servers.dat / MC Ping / LAN 发现）
