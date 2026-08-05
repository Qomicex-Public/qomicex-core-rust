# CHECKPOINT_BATCH_3.md — API 层 + 事件契约

- 日期：2026-08-06
- 分支：migrate/b3
- 范围：B3 公开 API 层（api/ 10 traits）+ event.rs + 服务器/选项模型补齐
- 状态：✅ 完成（cargo check 零警告，17 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P14 | Public/IAuthProvider.cs | api/auth.rs（3 必需 + 4 默认方法） | ✅ |
| P15 | Public/ILaunchExecutor + Public/Core/IDownloadSourceManager | api/launch.rs + api/download.rs | ✅ |
| P16 | 4 个版本域接口（IVersionManagementService 等） | api/version.rs（4 traits） | ✅ |
| P17 | IJavaProvider + IInstallerProvider | api/java.rs + api/installer.rs | ✅ |
| P18 | ILocalResourcesFactory + IServerManager + IOptionsProvider | api/local.rs + api/server.rs + api/options.rs | ✅ |
| P19 | IModrinthSource + ICurseForgeSource + IFTBSource | api/expansion.rs（3 traits） | ✅ |
| P20 | Services/Options/ServiceTypes.cs | models/local.rs 追加 9 类型（GameOption/ServerEntry/ServerState 等） | ✅ |
| 契约 | Public/Events/* + IProgress<T> | src/event.rs（CoreEvent + ProgressReporter + ChannelProgressReporter） | ✅ |

## 关键决策

1. **事件契约**：CoreEvent 枚举（DownloadProgress/InstallProgress/Log/State）+ ProgressReporter trait + ChannelProgressReporter（tokio::sync::mpsc::Sender）——ADR-001 D3 落地，tokio 仅启用 sync feature
2. **签名映射**：Task<T> → Result<T, Error>；可空 → Option；IProgress? → Option<&dyn ProgressReporter>；重载 → _from_json 后缀（4 组）
3. **默认接口方法**：C# 默认方法（StartDeviceCodeAsync 等）→ trait 默认实现，语义保留（Ok(None)/固定错误）
4. **async_fn_in_trait**：edition 2024 lint → lib.rs `#![allow]` 暂缓；B4 Facade 若需跨线程 spawn 再转 RPITIT + Send（技术债登记）
5. **CancellationToken**（IServerManager 4 处参数）→ `()` 占位，B11 服务器批次定案（tokio-util 或移除）
6. **MAPPING_TABLE api 段修正**：README 与源代码签名差异（SearchAsync→Search、ExecuteAsync→LaunchAsync、LoginAsync→StartDeviceCodeAsync 等）已全部以源文件为准回填

## 已知占位（后续批次承接）

| 占位 | 承接批次 |
|------|---------|
| Box<dyn ModsManager 等 6 个空 trait> | B10（ILocalResourcesFactory 子接口） |
| ServerManager/OptionsProvider 引用的 models/local 新类型 | 本批已补齐 ✅ |
| api/expansion 3 traits 无实现 | B13 |

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：17/17（B1+B2 回归通过）

## 下一步

B4：builder.rs（GameCoreBuilder）+ core.rs（GameCore Facade，Arc<dyn Trait> 注入）
