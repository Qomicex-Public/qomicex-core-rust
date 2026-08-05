<<<<<<< HEAD
# qomicex-core-rust
Qomicex Launcher使用Rust重写的启动核心，是QML未来全平台支持的核心方向
=======
# Qomicex Core (Rust)

Qomicex Launcher 使用 Rust 重写的启动核心，QML 未来全平台支持的核心方向。以 Rust 写就的下一代 Minecraft 启动核心，是 [Qomicex.Core.AOT](https://github.com/Qomicex-Public/Qomicex.Core.AOT)（.NET 10 Native AOT）的 **Rust 重构移植版**。按 rlib 标准架构设计，纯库 crate，供 [Qomicex.Tauri](https://github.com/Qomicex-Public/Qomicex.Tauri) 后端消费。

全链路覆盖：**认证 → 版本管理 → ModLoader 安装 → 资源下载 → 游戏启动 → 内容管理 → 扩展平台**。

> ⚠️ **开发状态**：架构设计已完成（[ADR-001](docs/junsi-dev-docs/1-决策记录/ADR-001-Rust启动核心架构设计.md)），正在按批次移植中。

## 特性

- 认证：离线 / Microsoft 正版（OAuth 设备码流）/ Yggdrasil 外置登录
- 版本：清单获取与缓存、本地扫描、版本隔离、资源自动补全
- 下载：多线程、缺失检查、官方 / BMCLAPI / 自定义镜像切换
- Java：多源扫描检测（注册表 / 环境变量 / PATH）、推荐、在线下载
- 启动：JVM 参数自动组装、原生进程启动
- ModLoader：Forge（Legacy + New）/ NeoForge / Fabric / Quilt / LiteLoader / OptiFine
- 本地内容：Mod / 存档 / 资源包 / 光影 / 数据包 / 截图管理
- 服务器：servers.dat CRUD、Minecraft 协议 Ping、局域网发现
- 游戏设置：options.txt 读写
- 扩展平台：Modrinth / CurseForge / Feed The Beast API

## 架构

三层可见性 + Builder 模式唯一入口：

```
api/ (pub trait)  →  GameCore (Facade, Arc<dyn Trait> 注入)  →  services/ (pub(crate))
```

- 全模型 serde derive，零反射
- tokio + reqwest 异步模型，进度/事件走 `mpsc::channel<CoreEvent>`
- feature flags 裁剪扩展平台（`expansion` / `java-download` / `lan`）

详见 [docs/architecture.md](docs/architecture.md)。

## 快速开始

> 当前迁移中，以下为规划中的公开 API：

```rust
use qomicex_core::prelude::*;

let core = GameCore::builder()
    .game_root("/path/to/.minecraft")
    .auth_mode(AuthMode::Offline)
    .download_mirror(DownloadMirror::Bmclapi)
    .build();

let versions = core.version().get_remote_versions().await?;
```

## 开发

```bash
cargo check   # 类型检查
cargo test    # 单元测试
cargo build   # 构建 rlib
```

## 移植进度

见 [docs/architecture.md](docs/architecture.md) 第 6 节（依赖 DAG 分 14 批），当前进度记录在 `.memory/`。

## 许可证

[GPL-3.0](LICENSE)。本仓库为 Qomicex.Core.AOT（GPL-3.0）的移植衍生作品。
>>>>>>> 51bfbe0 (chore: rlib 架构目录骨架 + README + GPL-3.0 许可证修正)
