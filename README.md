# Qomicex Core (Rust)

Qomicex Launcher 使用 Rust 重写的启动核心，QML 未来全平台支持的核心方向。以 Rust 写就的下一代 Minecraft 启动核心，是 [Qomicex.Core.AOT](https://github.com/Qomicex-Public/Qomicex.Core.AOT)（.NET 10 Native AOT）的 **Rust 重构移植版**。按 rlib 标准架构设计，纯库 crate，供 [Qomicex.Tauri](https://github.com/Qomicex-Public/Qomicex.Tauri) 后端消费。

全链路覆盖：**认证 → 版本管理 → ModLoader 安装 → 资源下载 → 游戏启动 → 内容管理 → 扩展平台**。

> ✅ **移植完成**（2026-08）：13 批次 / 47 原子包，源 15,890 行 C# → Rust。35 测试 + QA 快照比对 PASS。

## 特性

- 认证：离线 / Microsoft 正版（OAuth 设备码流）/ Yggdrasil 外置登录
- 版本：清单获取与缓存、本地扫描、版本隔离、资源自动补全
- 下载：并发限流、缺失检查、官方 / BMCLAPI / 自定义镜像切换
- Java：多源扫描检测（注册表 / 环境变量 / PATH）、推荐、在线下载（Adoptium / Zulu / BMCLAPI）
- 启动：JVM 参数自动组装、原生进程启动（CreateNoWindow）、natives 处理
- ModLoader：Forge（Legacy + New）/ NeoForge / Fabric / Quilt / LiteLoader / OptiFine / Cleanroom / LegacyFabric / Babric
- 整合包：CurseForge / Modrinth / FTB
- 本地内容：Mod / 存档 / 资源包 / 光影 / 数据包 / 截图管理
- 服务器：servers.dat CRUD、Minecraft 协议 Ping（Modern+Legacy 回退）、局域网发现（UDP 多播）、SRV 解析（自定义 DNS）
- 游戏设置：options.txt 读写、版本可用性、多语言描述
- 扩展平台：Modrinth / CurseForge（指纹反查）/ Feed The Beast API

## 架构

三层可见性 + Builder 模式唯一入口：

```
api/ (pub trait)  →  GameCore (Facade, Arc<dyn Trait> 注入)  →  services/ (pub(crate))
```

- 全模型 serde derive，零反射（对齐源 AOT 约束）
- tokio + reqwest 异步模型，进度/事件走 `mpsc::channel<CoreEvent>`
- 跨平台：Windows / macOS / Linux / **Android**（纯 Rust 依赖链）

详见 [docs/architecture.md](docs/architecture.md)。

## 快速开始

```rust
use qomicex_core::builder::GameCoreBuilder;
use qomicex_core::models::download::DownloadMirror;

#[tokio::main]
async fn main() {
    let core = GameCoreBuilder::new()
        .use_game_root("/path/to/.minecraft")
        .use_offline_auth("Steve")
        .use_download_mirror(DownloadMirror::Bmclapi)
        .build();

    let versions = core.version().get_available_versions(false).await.unwrap();
    for v in versions {
        println!("{}", v.id);
    }
}
```

完整示例见 [docs/usage.md](docs/usage.md)。

## 开发

```bash
cargo check   # 类型检查（0 warning 为交付标准）
cargo test    # 35 个单元测试
cargo build   # 构建 rlib
```

开发约定与依赖约束见 [docs/development.md](docs/development.md)。

## 文档

| 文档 | 说明 |
|------|------|
| [架构设计](docs/architecture.md) | 模块树 / 依赖 / 设计决策 / 批次 |
| [使用指南](docs/usage.md) | 各域 API 示例 / 事件进度 / Android 集成 |
| [开发指南](docs/development.md) | 构建测试 / 编码规范 / 技术债 |
| [决策记录](docs/junsi-dev-docs/1-决策记录/) | ADR-001（架构）/ ADR-002（移植总结）/ ADR-003（技术债清理） |
| [映射表](MAPPING_TABLE.yaml) | 源（.NET）→ 目标（Rust）全量映射 |
| [批次检查点](docs/checkpoints/) | 13 批交付明细 |

## 许可证

[GPL-3.0](LICENSE)。本仓库为 Qomicex.Core.AOT（GPL-3.0）的移植衍生作品。
