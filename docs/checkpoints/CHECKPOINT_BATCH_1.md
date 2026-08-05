# CHECKPOINT_BATCH_1.md — 模型层移植

- 日期：2026-08-06
- 分支：migrate/b1
- 范围：B1 数据模型层（error.rs + src/models/ 全部）
- 状态：✅ 完成（cargo check 零警告，9/9 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P1 | Exceptions/*.cs ×5 | src/error.rs（Error 枚举 + thiserror） | ✅ |
| P2 | Models/Auth、UserAuth、IAuthProvider 内 4 类、CoreOptions 内 AuthMode/AuthOptions/DownloadMirror、Download/*、ResourceType | src/models/auth.rs + download.rs | ✅ |
| P3 | Models/VersionManifest ×3、Models/Local ×2、ParamsMeta/ParamEntry + Config/Arguments | version_manifest.rs + local.rs + params_meta.rs | ✅ |
| P4 | Models/VersionMetadata ×7 + JsonConverters/VersionArgumentsConverter.cs | version_metadata.rs（含自定义 Serialize/Deserialize） | ✅ |
| P5 | Public/Models（LaunchResult/JavaResult/MissFileInfo/ModLoaderResult）+ CoreOptions 内 LaunchOptions/JavaOptions + OptiFineVersionInfo | launch.rs + java.rs + installer.rs | ✅ |
| P6 | Models/Expansion/Local ×6 | expansion/local.rs | ✅ |
| P7 | Models/Expansion/Modrinth ×9 | expansion/modrinth.rs（字符串枚举） | ✅ |
| P8 | Models/Expansion/CurseForge ×14 | expansion/curseforge.rs（字符串枚举） | ✅ |
| P9 | Models/Expansion/FeedTheBeast ×3 | expansion/ftb.rs | ✅ |

## 特殊兼容点（已实现 + 测试覆盖）

1. **VersionArguments 新旧格式**（VersionArgumentsConverter）：
   - 旧形态 `"args string"` → `VersionArguments::Old`
   - 新形态 `{"game":[...],"jvm":[...]}` → `New`；jvm 缺失 → 空 Vec
   - 元素：字符串 → String；对象 → value/rules；单值 value 压缩为字符串；rules 空省略键
   - 序列化 Old → 报错（镜像源 NotSupportedException）
   - 测试：`tests/b1_models.rs` 4 个用例

2. **枚举序列化双轨**：
   - Modrinth/CurseForge/FTB（UseStringEnumConverter=true）→ 字符串枚举，逐变体 rename 对齐（如 `NeoForge → "neoForge"`、`LiteLoader → "liteLoader"`）
   - 其余（默认数字）→ serde_repr + repr(i32)，值 0 递增
   - 测试：2 个用例

3. **类型冲突处理**：`type` → `r#type`（序列化名不变）；`Type` → `r#type`；Dictionary<string,X> → HashMap；Dictionary<long,X> → HashMap<i64>

## 已知偏差（⚠️ UNMAPPED → 定案）

| 项 | 源 | 定案 |
|----|----|------|
| 时间字段（DateTimeOffset/DateTime，全批约 18 处） | DateTimeOffset? | `String` 原始文本保真；引入 chrono 的决策推迟到 B6（网络层）确认 |
| LaunchResult.Exception / OnOutput / OnError / OnExit | .NET 异常/委托 | 省略；B8 启动批次由 event.rs CoreEvent 承接 |
| ModInfo.Active | get-only 计算属性 | `is_active()` 方法（不参与序列化） |
| VersionArguments 畸形输入（非对象/无 game 键） | 返回 null 静默 | 报错（真实 Mojang JSON 不触发，行为等价于 null 传播） |

## 依赖

B1 引入：serde(derive) 1.0.229、serde_json 1.0.151、thiserror 2.0.19、serde_repr 0.1.21

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：9/9 通过（VersionArguments 兼容 ×4、字符串枚举 ×1、数字枚举 ×1、manifest 往返 ×2、结构构造 ×1）

## 下一步

B2：util/（murmurhash2、nbt、version_json、launcher_profiles、platform）
