# CHECKPOINT_BATCH_8.md — 启动域实现（用户检查通过 + Android 兼容定案）

- 日期：2026-08-06
- 分支：migrate/b8
- 范围：B8 services/launch（JVM 参数组装 + 进程启动 + natives）+ Android 兼容性
- 状态：✅ 完成（35 测试通过，零警告，用户检查确认后合并）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P33 | LaunchExecutor.cs 参数组装（SelectParams/GetJVMParams×2/GetGameParams/GetClassPath/GetMainClass/GetDataDir/NameToUuid） | launch/jvm_args.rs（827 行） | ✅ |
| P34 | LaunchExecutor.cs 启动/进程/natives（LaunchAsync/KillAsync/GetNativePath/UnzipNatives/IsForeignArchDir/FlattenNatives/ParseJavaLibraryPath/GetNatives/ParseGameJson） | launch/process.rs（715 行） | ✅ |

## 实现要点（检查用）

### JVM/游戏参数组装（jvm_args.rs）
- 默认 JVM 参数集 10 项顺序逐字（G1GC → AdaptiveSizePolicy → OmitStackTrace → fml×2 → log4j2 → ExtraJvmArgs → Windows 适配 → launcher.brand → version=23）
- Legacy 分支 6 项在 InheritsFrom 递归之后追加
- 20 项替换令牌顺序 + NormalizeArg/FormatDirPath/TrimEnd 组合逐字（含 quirk：${assets_root}/${library_directory}/${auth_player_name} 不 NormalizeArg）
- classpath 分隔符 Windows `;` / 其他 `:`（复用 platform::get_separator）
- **Forge/NeoForge 主 jar 跳过**（bootstrap 类名 + assetIndex minor>=17 / 纯数字判定）、**OptiFine --tweakClass 改写**
- 版本隔离 → gameDir/versions/{id}；joinServer/joinWorld → --server/--port、quickPlay 支持
- NameToUuid 与 util/platform.rs generate_uuid 逐位一致 → 复用

### 进程启动（process.rs）
- GameRoot 覆盖（effective_game_dir）→ UnzipNatives → select_params → NormalizeArg(JavaPath ?? "java")
- WorkingDirectory（版本隔离）→ eprintln 回显命令 → tokio::process spawn（整串按引号分组切分）→ 后台并行读 stdout/stderr 防阻塞 + child.wait() 打印退出码
- **立即返回** LaunchResult{Success=true, ProcessId, "进程已启动: {pid}"}（源语义）
- 失败：写 launch-errors.log → Err（消息"启动失败,{ex.Message}"模式）
- 源**无环境变量设置、无进程句柄存储**（已核对，未发明）
- KillAsync：按平台杀进程

### natives 处理
- get_natives（规则单条判定 + 继承递归 + check_libs_ver）→ unzip（versions/{v}/{v}-natives）→ parse_java_library_path → 清空重解压 → FlattenNatives（.dll/.dylib/.so，两轮遍历，IsForeignArchDir 跳过异构架构）

## 已知偏差修复记录（用户检查意见：1.不能接受 2.补 CreateNoWindow）

| # | 项 | 修复 |
|---|----|------|
| 1 | OS 版本检测 | ✅ 注册表 CurrentMajorVersionNumber（reg query）精确判定 Win10+（-Dos.name/-Dos.version） |
| 2 | CreateNoWindow | ✅ tokio::process::Command::creation_flags(CREATE_NO_WINDOW)（cfg windows） |
| 3 | Unix 进程树 kill | ✅ ps -eo 递归收集后代 + 深度降序（叶子先杀）kill -9；失败保守降级单进程 |
| 4 | 时间戳精度 | ✅ 毫秒精度 .SSS（源 {UTC:O} 7 位小数，日志场景取毫秒） |
| 5 | CommandLineToArgvW | ✅ 反斜杠转义（奇/偶 `\`+`"`）+ 空参数（""）规则，5 个单元测试固化 |
| 6 | GameRoot 覆盖 | 源行为一致（每次 launch 传参），无需改 |
| 7 | parse_game_json 重复 | process.rs 版改名 parse_game_json_config（保留两套，B9 收尾清理） |

## Android 兼容性定案（subagent 分析报告落地）

| 项 | 处理 |
|----|------|
| reqwest native-tls（openssl-sys Android 不可编译）| ✅ `default-features = false` + `rustls-tls`（阻塞项，已修复，编译验证通过） |
| get_current_os_name 返回 "unknown" → natives/库规则全失效 | ✅ android → "linux"（关键运行缺口，一行修复） |
| scanner 高优先级路径/驱动器枚举 | ✅ android 归并 linux 分支（建议项） |
| zip C 后端（bzip2/zstd/lzma-sys）| ✅ 裁剪为 `deflate-miniz` + `deflate64`（纯 Rust miniz_oxide） |
| kill/ps 命令（Android toybox）| ✅ 已确认可用 |
| Java spawn 绝对路径 / QOMICEX_HOME | 宿主约定（文档记录），无需改代码 |

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：35/35（B8 launch 5：split_command_line 4 + 时间戳 1 + 回归 30）

## 下一步

B9：安装器实现（6 种 + InstallerFactory trait 补齐 + core.rs installer_factory 字段）
