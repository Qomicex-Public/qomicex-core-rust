//! Qomicex Core — Minecraft 启动核心库（Rust 移植版）
//!
//! 移植自 [Qomicex.Core.AOT](https://github.com/Qomicex-Public/Qomicex.Core.AOT)
//! （.NET 10 Native AOT）。全链路覆盖：认证 → 版本管理 → ModLoader 安装 →
//! 资源下载 → 游戏启动 → 内容管理 → 扩展平台。
//!
//! 架构设计见 `docs/architecture.md`，类型映射见 `MAPPING_TABLE.yaml`。

// async fn in trait（RPITIT 决策）：B3 阶段保持 async fn 可读性；
// 若 B4 Facade 集成出现跨线程 spawn 需求，批量转 `-> impl Future + Send`。
#![allow(async_fn_in_trait)]

pub mod api;
pub mod builder;
pub mod core;
pub mod error;
pub mod event;
pub mod models;
pub mod services;
pub mod util;
