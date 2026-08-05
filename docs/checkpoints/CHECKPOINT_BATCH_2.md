# CHECKPOINT_BATCH_2.md — 工具层移植

- 日期：2026-08-06
- 分支：migrate/b2
- 范围：B2 工具层（src/util/ 全部 5 文件 + 2 新增）
- 状态：✅ 完成（cargo check 零警告，B1+B2 共 17 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P10 | Utils/SystemHelper + PathHelper + OfflineUuidHelper + FileHelper | util/platform.rs + util/file_helper.rs | ✅ |
| P11 | Utils/GameVersionHelper.cs（322 行，class 常量池解析） | util/version_json.rs | ✅ |
| P12 | Utils/LibHelper.cs（276 行） | util/lib_helper.rs（新增） | ✅ |
| P13 | Services/Options/NbtIO.cs + LocalResourceBase.cs 的 MurmurHash2 + Utils/JsonHelper + MinecraftDateTimeConverter | util/nbt.rs + util/murmurhash2.rs + util/json_helper.rs | ✅ |

## 特殊兼容点（已实现 + C# 参考向量验证）

1. **MurmurHash2 / CurseForgeFingerprint**（dotnet 10 参考实现生成真实向量）：
   - 小端读取 `u32::from_le_bytes`、wrapping 算术、uint→long 无符号零扩展
   - **修复 Translator 的 goto 穿透 bug**：C# `case 3 → case 2 → case 1` 是**累积**异或（len=3 执行全部三个），Rust 初版写成了互斥 match —— 由 "hello world"（3 字节尾）向量捕获并修复
   - 6+1 个真实向量全部匹配（""、"a"、"hello"、"hello world"、"Minecraft"、40 字节、含控制字符输入 + CF 指纹）

2. **class 文件常量池解析**（version_json.rs）：17 种 CONSTANT_* tag 全分支 1:1，含 LONG/DOUBLE 双索引槽、UTF8 解码（lossy 同 U+FFFD）、84 条 KNOWN_VERSIONS 哈希表 → match

3. **NBT**：源只支持 Byte(读为 bool)/String/List(仅 Compound)/Compound，不支持类型按源抛错；servers.dat 语义保留

4. **MinecraftDateTimeConverter**：yyyy-MM-dd'T'HH:mm:ssK 格式、+0800 自动补冒号、±14h 范围校验；无 chrono（轻量 MinecraftDateTime 结构）

## 已知偏差（⚠️ → 定案）

| 项 | 定案 |
|----|------|
| Environment.OSVersion.VersionString（Rule os version 匹配） | 恒 None → 不匹配（os_info crate 可选，暂不引入） |
| DateTimeOffset.TryParse 全格式矩阵（英文月份等） | 仅 RFC3339/ISO-8601 子集（Minecraft 实际数据形态） |
| 无偏移时间源按本地时区 | 按 +00:00 处理 |
| lib_helper：C# Downloads==null 分支 | B1 模型 downloads 必填 → 用 artifact+classifiers 均空近似 |
| launcher_profiles.json | **源项目 AOT 版无此实现**（上游 Qomicex.Core 有）→ 暂缓，B11 服务器批次再评估 |

## 依赖

B2 引入：sha1 0.10、md-5 0.10、zip 2、regex 1（编译验证通过）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：17/17（B1 9 + B2 8；B2 含 7 个 MurmurHash2 C# 参考向量、NBT 往返、datetime 解析、lib 坐标/去重/分类）

## 下一步

B3：api/ traits（9 个）+ event.rs（CoreEvent + mpsc 通道）
