# Qomicex Core (Rust) 架构设计

> 移植自 [Qomicex.Core.AOT](https://github.com/Qomicex-Public/Qomicex.Core.AOT)（.NET 10 Native AOT，164 文件 / ~15,890 行）
> 目标：rlib 标准架构的 Rust 启动核心库，供 Qomicex.Tauri 后端消费。
> 状态：**已确认**（2026-08-06），进入逐模块移植阶段。

## 1. 顶层形态

```
qomicex-core：纯库 crate
├── crate-type = ["rlib", "cdylib"]   # rlib 供 Rust 宿主复用；cdylib 预留 FFI
├── 无 bin；不提供 CLI
├── 公开面 = api/ traits + models + builder，其余 pub(crate)
├── feature flags：default = minimal（认证/版本/下载/启动/安装器）
│                     + "expansion"（Modrinth/CurseForge/FTB）
│                     + "java-download"（Java 在线下载）
│                     + "lan"（局域网发现）
└── 异步模型：tokio + async fn（对应 C# async/await），本地 IO 走 spawn_blocking
```

## 2. 模块树

```
src/
├── lib.rs                      # pub use 重导出，收敛公开 API
├── builder.rs                  # GameCoreBuilder：唯一入口（Builder 模式）
├── core.rs                     # GameCore Facade：持有 Arc<dyn ...> 服务引用
├── api/                        # 全部公开 trait（对应源 Public/）
│   ├── auth.rs                 # AuthProvider
│   ├── launch.rs               # LaunchExecutor
│   ├── version.rs              # VersionManagementService + VersionManifestService + VersionLocator + ResourceCompleter
│   ├── java.rs                 # JavaProvider
│   ├── installer.rs            # InstallerProvider + InstallerFactory
│   ├── download.rs             # DownloadSourceManager
│   ├── local.rs                # LocalResourcesFactory
│   ├── server.rs               # ServerManager
│   └── options.rs              # OptionsProvider
├── services/                   # pub(crate) 实现层（对应源 Services/，禁止外部可见）
│   ├── auth/                   # microsoft.rs(设备码流) / yggdrasil.rs / offline.rs
│   ├── version/                # manifest.rs(远端清单+缓存) / locator.rs(本地扫描) / completer.rs(资源补全)
│   ├── download/               # mirror.rs(镜像 URL 转换) / retry.rs / checksum.rs
│   ├── java/                   # scanner.rs(注册表/环境变量/PATH/多源) / recommend.rs / download.rs
│   ├── launch/                 # jvm_args.rs(参数组装) / process.rs(进程启动+输出事件)
│   ├── installers/             # forge/ neoforge/ fabric/ quilt/ liteloader/ optifine/  + factory.rs
│   ├── local/                  # mods.rs / saves.rs / resourcepacks.rs / shaders.rs / datapacks.rs / screenshots.rs
│   ├── server/                 # servers_dat.rs / mc_ping.rs / lan_discovery.rs
│   ├── options/                # options_txt.rs
│   └── expansion/              # modrinth/ curseforge/ ftb/
├── models/                     # 全部数据模型（serde derive；对应 Models/ + JsonContext/）
├── error.rs                    # Error enum（thiserror；对应 Exceptions/）
├── event.rs                    # CoreEvent 枚举 + mpsc 通道（对应 IProgress<T> + Events/）
└── util/                       # murmurhash2.rs / nbt.rs / launcher_profiles.rs / version_json.rs / platform.rs
```

## 3. 依赖清单（待锁定版本）

| 用途 | crate | 对应 .NET |
|------|-------|-----------|
| 序列化 | serde + serde_json | System.Text.Json SourceGen |
| 异步 | tokio (rt-multi-thread, fs, process, sync) | Task / HttpClient |
| HTTP | reqwest (json, stream, gzip) | HttpClient |
| 错误 | thiserror | 自定义异常 |
| TOML | toml | Tomlyn |
| 哈希 | sha1, sha2, base64 | 内置 SHA1/MD5 |
| 压缩 | zip, bzip2 | System.IO.Compression |
| 测试 | tokio-test（可选） | xUnit |

不引入：任何反射/运行时动态机制（rlib 天然无反射）。

## 4. 关键设计决策

| # | 决策 | 理由 |
|---|------|------|
| D1 | 接口在 api/（pub trait），实现全 pub(crate) | 对齐 .NET "接口 public + 实现 internal"，公开面极简 |
| D2 | Facade 持有 `Arc<dyn Trait>` | 对应 C# DI 注入，解耦实现替换 |
| D3 | 进度/事件用 `mpsc::channel<CoreEvent>` | 避免回调闭包生命周期问题；跨线程安全 |
| D4 | 网络全 async，本地 IO 用 spawn_blocking | 阻塞调用不污染运行时 |
| D5 | serde derive 全部模型，零自定义反射 | 对齐 AOT 无反射约束 |
| D6 | 下载层统一走 DownloadSourceManager（镜像切换）| 单点改造镜像 URL |
| D7 | 安装器统一 InstallerFactory 创建，返回 dyn Installer | 对齐源工厂模式 |
| D8 | feature flags 裁剪扩展平台 | 核心体积最小化，扩展可选 |
| D9 | 错误统一 Error enum + 上下文 | 替代 .NET 异常层级 |
| D10 | builder.rs 唯一入口，参数集中 CoreOptions | 调用方只学一个入口 |

## 5. 核心流程（启动链路）

```
GameCore::builder()
  .game_root(...) .auth_mode(...) .download_mirror(...)
  .build() -> Arc<GameCore>

auth   : offline | microsoft(设备码流) | yggdrasil
          -> 统一产出 AccessToken + UUID
version: manifest(远端+缓存) -> locator(本地扫描) -> install(自动补全资源)
java   : scanner(注册表/PATH/多源) -> recommend(打分) -> 缺失则 download
launch : jvm_args(隔离/GC/内存/分辨率/服务器) -> spawn -> 输出转 CoreEvent
```

## 6. 移植批次规划（依赖 DAG）

| 批 | 内容 | 依赖 |
|----|------|------|
| B1 | error.rs, models/（纯数据模型） | 无 |
| B2 | util/（murmurhash2, nbt, version_json, launcher_profiles） | B1 |
| B3 | api/ traits + event.rs | B1 |
| B4 | builder.rs + core.rs（Facade 骨架） | B1-B3 |
| B5 | services/auth | B3 |
| B6 | services/download + services/version | B5 |
| B7 | services/java | B6 |
| B8 | services/launch | B6 |
| B9 | services/installers | B7 |
| B10 | services/local | B6 |
| B11 | services/server | B6 |
| B12 | services/options | B6 |
| B13 | services/expansion | B6 |
| B14 | 快照比对 + 边界测试 | 全部 |

每批 ≤200 行变更；每批结束 cargo build + 检查点 commit；QA 快照比对验证行为一致性。

## 7. 已知风险

- Forge 安装器（Legacy+New）逻辑复杂：拆分 forge_legacy / forge_new 子模块
- NBT/servers.dat 二进制格式：纯移植 util/nbt.rs，无现成 crate
- CurseForge 指纹（MurmurHash2 变体）：纯移植 util/murmurhash2.rs
- MC 协议 Ping + LAN UDP 多播：std UdpSocket + 手写协议解析
- Windows 注册表扫描（Java 检测）：cfg(windows) + winreg crate（待定）或 Registry 命令回退
- 大文件流式下载：reqwest 流 + tokio fs，校验 SHA1

## 8. 与上游的接口对齐

| C# 公开 API | Rust 公开 API |
|-------------|---------------|
| `new GameCoreBuilder()` | `GameCore::builder()` |
| `core.Auth.AuthenticateAsync(...)` | `core.auth().authenticate(...)` |
| `core.Version.GetRemoteVersionsAsync()` | `core.version().get_remote_versions()` |
| `core.JavaProvider.SearchAsync(...)` | `core.java().search(...)` |
| `core.Installer.CreateForge(...)` | `core.installer().create_forge(...)` |
| `core.Launch.ExecuteAsync(...)` | `core.launch().execute(...)` |
| `core.LocalResourceProvider.CreateMods(...)` | `core.local().mods(...)` |
| `new ServerManager(...)` | `core.server()` |
| `new OptionsProvider(...)` | `core.options()` |
| `core.CreateModrinthSource()` | `core.modrinth()` |
