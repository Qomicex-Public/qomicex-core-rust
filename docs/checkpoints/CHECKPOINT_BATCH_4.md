# CHECKPOINT_BATCH_4.md — GameCore Facade + 异步 trait 定案

- 日期：2026-08-06
- 分支：migrate/b4
- 范围：B4 core.rs（GameCore Facade）+ 异步 trait 架构定案
- 状态：✅ 完成（cargo check 零警告，17 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P21 | Core/DefaultGameCore.cs | src/core.rs（GameCore：11 字段 Arc<dyn> 注入 + new() + 访问器） | ✅ |

## 架构定案：异步 trait 方案（重要）

**问题**：B3 的 `async fn in trait` 与架构 D2（`Arc<dyn Trait>` Facade）根本冲突：
- `async fn in trait` → trait 非 dyn-compatible（E0038）
- RPITIT（`-> impl Future + Send`）→ 同样非 dyn-compatible（E0038）
- 只有 **`#[async_trait]` 宏**（返回 `Pin<Box<dyn Future>>`）支持 dyn 分发

**定案**：8 个含异步方法的 trait 全部加 `#[async_trait]`（auth/launch/version/java/installer/download/server/expansion）；
api/local.rs、api/options.rs 纯同步方法无需。`async-trait = "0.1"` 引入。
验证：`cargo check` 通过（Arc<dyn> 成立）+ 最小复现 crate 排除环境问题。

**排障记录**（防重蹈）：
1. 多轮正则脚本叠加导致 `use async_trait::async_trait;` **重复** → "cannot determine resolution for the import"（E0432）
   → 已全部去重
2. core.rs 访问器返回类型需 `&(dyn Trait + Send + Sync)`（与字段一致）

## GameCore 结构

- 11 字段：version/auth/launch/java_provider/installer_provider/locator/download_manager/local_resource_provider（Arc<dyn>）+ options/server（Option<Arc<dyn>>）+ game_root（String）
- 访问器方法：`version()`、`auth()`、`launch()`、`options()`、`server()` 等返回 trait 引用
- 待补（B9/B13）：installer_factory 字段（InstallerFactory trait B3 未翻译，B9 补）、CreateModrinthSource 等 3 工厂方法（B13）、HttpClient（B13 reqwest 共享 client）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：17/17（回归通过）

## 下一步

B5：services/auth（DefaultAuthProvider/MicrosoftAuthProvider/YggdrasilAuthProvider）
