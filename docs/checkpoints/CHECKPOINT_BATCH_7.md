# CHECKPOINT_BATCH_7.md — Java 域实现

- 日期：2026-08-06
- 分支：migrate/b7
- 范围：B7 services/java（扫描 / 推荐 / 下载 / 聚合）
- 状态：✅ 完成（cargo check 零警告，30 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P30 | JavaProvider.cs 扫描部分（~700 行） | java/scanner.rs（JavaScanner，1171 行） | ✅ |
| P31 | Recommand/Check/GetRequireMajroVersion | java/recommend.rs（JavaRecommender） | ✅ |
| P32 | GetPackages/Adoptium/Zulu/BMCLAPI | java/download.rs（JavaDownloader） | ✅ |
| 整合 | — | java/provider.rs（JavaProviderService 聚合，实现 api trait） | ✅ |

## 关键实现

1. **扫描**：
   - BFS 目录搜索（深度/过滤/exclude 56 项/三平台路径常量 LazyLock）
   - **Windows 注册表：`reg query` 命令零依赖方案**（16 键路径逐字，退出码 1 静默=源 null 语义）；CJK 乱码风险已记录（可选 winreg）
   - 环境变量/PATH/高优先级路径/Minecraft 运行时目录扫描
   - `java -version` 输出解析（stderr，5s 超时 + kill 防悬挂）
   - 版本归一（1.8→8）、Parallel.ForEach(4) → thread::scope 原子索引并行（零依赖）
   - GetDriveTypeW FFI（kernel32，零依赖）
2. **推荐**：diff 排序、精确匹配优先、**require==8 必须精确匹配**（diff>0 抛错）、找不到 → Error::VersionNotFound（消息逐字）
3. **下载**：Adoptium/Zulu/BMCLAPI 三源 URL/字段逐字；Zulu 排序键（java_version/distro_version/openjdk_build_number 全降序 + 8 位零填充）；**BMCLAPI 恒返回空（源 bug 逐字保留）**
4. **聚合**：JavaProviderService 三件套组合实现 api trait（避免 E0119）

## 已知偏差/记录

- 源仅查 HKLM（任务描述写 HKLM/HKCU——以源码为准 HKLM-only，日志标注）
- reg 输出 OEM 码页编码（中文系统 GBK 路径乱码 ⚠️，可选 winreg crate）
- 源构造无 mirror 参数（按任务加，已注明）
- recommend.rs 测试 5 个（精确匹配/全 diff<0 抛错/8 精确/空列表/check 校验）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：30/30（B7 5 + B6 5 + B5 3 + B1 9 + B2 8）

## 下一步

**B8：启动域（LaunchExecutor）——完成后交用户检查特殊兼容**
