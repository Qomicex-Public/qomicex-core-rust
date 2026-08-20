//! Qomicex Core — Minecraft 启动核心库（Rust 移植版）
//!
//! 移植自 [Qomicex.Core.AOT](https://github.com/Qomicex-Public/Qomicex.Core.AOT)
//! （.NET 10 Native AOT）。全链路覆盖：认证 → 版本管理 → ModLoader 安装 →
//! 资源下载 → 游戏启动 → 内容管理 → 扩展平台。
//!
//! 架构设计见 `docs/architecture.md`，类型映射见 `MAPPING_TABLE.yaml`。

// 异步 trait 决策（B4 定案）：api/ 层全部 trait 使用 #[async_trait] 宏
// （async fn in trait / RPITIT 均非 dyn-compatible，无法用于 Arc<dyn Trait> Facade 架构）。

pub mod api;
pub mod builder;
pub mod core;
pub mod error;
pub mod event;
pub mod jni;
pub mod models;
pub mod net;
pub mod services;
pub mod util;
