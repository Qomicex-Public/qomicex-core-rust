# CHECKPOINT_BATCH_6.md — 下载/版本域实现

- 日期：2026-08-06
- 分支：migrate/b6
- 范围：B6 services/download + services/version（7 原子包）
- 状态：✅ 完成（cargo check 零警告，25 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P25 | DefaultDownloadSourceManager.cs（89 行） | download/mirror.rs + retry.rs + checksum.rs | ✅ |
| P26 | VersionManifestService.cs（40 行） | version/manifest.rs | ✅ |
| P27 | DefaultResourceCompleter.cs（257 行） | version/completer.rs（612 行，单文件完整性超限已注明） | ✅ |
| P28a | DefaultVersionLocator.cs 扫描部分 | version/locator.rs（扫描 + 缓存） | ✅ |
| P28b | DefaultVersionLocator.cs 缺失检查部分 | version/locator_miss.rs（辅助函数） | ✅ |
| P29 | VersionManagementService.cs（143 行） | version/version_management.rs（编排 + 磁盘缓存） | ✅ |

## 重大排障记录

1. **tokio::task::scope 已移除**：tokio 1.44 弃用、1.53 删除（源码验证无 `pub async fn scope`）。
   处理：completer.rs 改用 `futures::future::join_all`（同任务内交错执行，借用合法，IO 并发等价）+ Semaphore 限流保留。引入 futures 0.3 依赖。
2. **跨文件 trait impl 冲突（E0119）**：两个 Translator 分别写 trait impl 同一类型 → 合并脚本把 locator_miss.rs 的 8 个方法并入 locator.rs 的 `#[async_trait] impl VersionLocator`（203 行），locator_miss.rs 保留固有辅助方法。
3. **`str::split().next_back()` 边界问题**：改用 `rsplit_once`（语义等价：C# Split 末段）。
4. **PowerShell 正则替换陷阱**：CRLF 不匹配、`n 字面量、重复 use（E0432 "cannot determine resolution"）——本轮全部以 edit 工具最终修复。

## 特殊兼容点

1. **镜像 URL 转换（bug-for-bug）**：4 条替换规则逐字保留（maven/assets/meta 前缀）。**源边界缺陷保留**：assets 直链 / piston-meta 无 `/meta/` 路径时 C# Split 末段 = 整串原 URL → 生成 `{base}/assets/{整串}` 坏 URL（源行为，测试 `*_bug_for_bug` 固化）
2. **generate_mirror_urls**：先原始 URL 再镜像（优先级升序）；Official 模式仅改首选源，不剔除 BMCLAPI 镜像（测试固化）
3. **版本识别**：locator 的 get_modloader_type 10 分支 + arguments/mainClass 判定完整移植
4. **磁盘缓存**：{gameRoot}/cache/version_manifest.json + 5 分钟过期 + force_refresh
5. **下载**：Semaphore 限流（默认 8）→ 流式写 + SHA1 校验 + 重试 3 次退避 + 进度上报（Downloading 0.5s 节流/Completed 100%/Assets 每 100 个）

## 已知偏差（源行为保留/定案）

- ProcessAssetIndexAsync 不建资源目录（源缺陷，保留）
- 客户端 jar 下载到 libraries/{path} 而检查走 versions/{id}/{id}.jar（源不一致，保留）
- check_resources_complete 不查资产索引（源同）
- Directory 创建失败静默（Rust 构造无法传播）

## 依赖

futures 0.3（join_all）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：25/25（B6 mirror 5 + B5 3 + B1 9 + B2 8）

## 下一步

B7：services/java（JavaProvider 扫描/推荐/下载）
