# 使用指南（Usage）

本库是 Minecraft 启动核心的 Rust 移植版。核心思路与 .NET 版一致：**Builder 模式唯一入口 → GameCore Facade → 各域服务**。全部网络操作是异步的（tokio），进度与事件通过 `mpsc::channel<CoreEvent>` 消费。

## 1. 快速开始

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
    // core: Arc<GameCore>
}
```

## 2. 认证（core.auth()）

```rust
use qomicex_core::models::auth::{AuthMode, AuthRequest};

// 离线认证
let result = core.auth().authenticate(AuthRequest {
    username: Some("Steve".to_string()),
    ..Default::default()
}).await?;

// Microsoft 设备码流（builder 配 UseMicrosoftAuth(client_id) 后）
let device = core.auth().start_device_code().await?.expect("设备码");
// 用户访问 device.verification_uri 输入 device.user_code
let poll = core.auth().poll_for_token(&device.device_code).await?;
if poll.as_ref().is_some_and(|p| p.is_completed) {
    let token = poll.unwrap().access_token.unwrap();
    let result = core.auth().complete_login(&token, "").await?;
}
```

## 3. 版本管理（core.version()）

```rust
let remote = core.version().get_available_versions(false).await?;  // 远端清单
let local  = core.version().get_installed_versions();              // 本地版本

// 安装（自动补全资源，可传进度回调）
core.version().install_version("1.20.4", None).await?;

// 缺失文件检查（locator）
let miss = core.locator().get_miss_files(&metadata).await?;
```

## 4. Java 检测（core.java_provider()）

```rust
use qomicex_core::models::java::{JavaSearchMode, JavaSearchOptions};

let list = core.java_provider().search(&JavaSearchOptions {
    mode: JavaSearchMode::Deep,
    ..Default::default()
}).await?;

let recommended = core.java_provider()
    .recommand(&list, &metadata).await?;   // 按版本元数据推荐
```

## 5. 安装 ModLoader（core.installer()）

```rust
use qomicex_core::api::installer::InstallerFactory;

// 工厂创建安装器（9 种 + 3 种整合包）
let forge = core.installer_factory().create_forge(0, ".minecraft", "1.20.4");

// 先查缺失库，再执行安装
let misses = forge.get_miss_libraries(None, None, None).await?;
forge.install("1.20.4-forge-49.0.0", &vanilla_json, Some(&java_path), None, None, None).await?;
```

## 6. 启动游戏（core.launch()）

```rust
use qomicex_core::models::launch::{JavaOptions, LaunchOptions};

let result = core.launch().launch(LaunchOptions {
    version: "1.20.4-forge-49.0.0".to_string(),
    version_isolation: true,
    java_options: Some(JavaOptions {
        java_path: "C:/Java/jdk-17/bin/java.exe".to_string(),
        max_memory_mb: 4096,
        ..Default::default()
    }),
    ..Default::default()
}).await?;

// 终止进程
core.launch().kill(result.process_id).await?;
```

## 7. 事件与进度

```rust
use qomicex_core::event::{CoreEvent, LogLevel};
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel::<CoreEvent>(64);
// 将 tx 传入需要进度上报的调用（如 VersionManagement::install_version 的
// Option<&dyn ProgressReporter> —— 用 ChannelProgressReporter::new(tx) 包装）

while let Some(event) = rx.recv().await {
    match event {
        CoreEvent::DownloadProgress(p) => println!("下载 {} {:.0}%", p.file_name, p.percentage),
        CoreEvent::InstallProgress(p)   => println!("安装 {:.0}%", p.percentage),
        CoreEvent::Log { level: LogLevel::Warn, message } => eprintln!("⚠ {message}"),
        _ => {}
    }
}
```

## 8. 本地内容管理（core.local_resource_provider()）

```rust
// Mod 列表 + 启禁
let mods = core.local_resource_provider().create_mods("1.20.4", true, "");
let list = mods.get_mod_list(None).await?;
mods.disable_mod(&list[0].file_name);

// 存档
let saves = core.local_resource_provider().create_saves("1.20.4", true, "");
saves.backup_save(&save_dir);

// 服务器
let server = core.server().expect("builder 默认创建");
let state = server.get_server_state_by_address("mc.example.com:25565");
if state.is_online {
    println!("在线: {}/{}", state.online_players, state.max_players);
}
```

## 9. 扩展平台（core.create_*_source()）

```rust
let modrinth = core.create_modrinth_source();
let results = modrinth.search("sodium", 0, 20, None, None, None, None, None).await?;

let cf = core.create_curseforge_source("YOUR_API_KEY");
let fingerprint = qomicex_core::util::murmurhash2::curse_forge_fingerprint(&jar_bytes);
let hit = cf.get_info_from_hashes(&[fingerprint]).await?;
```

## 10. Android（移动平台）集成

- 依赖链纯 Rust（rustls-tls / deflate-miniz），`cargo build --target aarch64-linux-android` 可直接构建
- **必须设置 `QOMICEX_HOME`** 环境变量指向数据目录（沙盒内无 HOME/XDG 变量）
- Java 启动需传**绝对路径**（`JavaOptions.java_path`），不能依赖 PATH 上的 `java`
- `os.name` 规则按 linux 语义处理（B8 定案）

## 11. 常见问题

| 问题 | 处理 |
|------|------|
| build() panic "InstallerProvider..." | 已修复：默认创建（B13） |
| 需要 Windows 隐藏控制台窗口 | 已内置 CREATE_NO_WINDOW（B8 用户检查项） |
| SRV 端口不生效 | 已修复：保留 SRV 端口（TD-3） |
| 400 判定 | 结构化 status 字段（TD-1） |
