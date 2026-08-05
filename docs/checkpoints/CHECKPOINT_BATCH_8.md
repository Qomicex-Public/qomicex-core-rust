# CHECKPOINT_BATCH_8.md — 启动域实现（待用户检查）

- 日期：2026-08-06
- 分支：migrate/b8
- 范围：B8 services/launch（JVM 参数组装 + 进程启动 + natives）
- 状态：✅ 编译通过（30 测试回归），**待用户检查特殊兼容后合并**

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

## 已知偏差清单（⚠️ 检查确认）

| # | 项 | 处理 | 影响 |
|---|----|------|------|
| 1 | OS 版本检测 | cfg!(windows) 近似 Major>=10 | 低（Windows 10/11 判定） |
| 2 | CreateNoWindow | tokio spawn 无窗口隐藏选项 | 中（Windows 启动时会闪控制台窗口） |
| 3 | Unix 进程树 kill | 只杀主进程 | 低 |
| 4 | 已退出进程 kill | 返回 false（源抛异常） | 低 |
| 5 | CommandLineToArgvW 细节 | `\"`/空参数边界差异 | 低 |
| 6 | GameRoot 覆盖 | 不持久（每次 launch 传参） | 与源一致 |
| 7 | parse_game_json 重复 | process.rs 版改名 parse_game_json_config（保留两套） | 待合并清理 |
| 8 | B1 Config 模型全必填 | 版本 JSON 缺键时手工 Value 解析 + 默认值填充 | 兼容性 OK |
| 9 | 时间戳 | {UTC:O} → 秒级 UTC（无 chrono） | 低 |
| 10 | reg/pipe 无 | — | — |

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：30/30（回归）

## 下一步

用户检查通过后：B9 安装器实现
