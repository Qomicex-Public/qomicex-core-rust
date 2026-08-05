# CHECKPOINT_BATCH_12.md — 游戏设置实现

- 日期：2026-08-06
- 分支：migrate/b12
- 范围：B12 services/options（OptionsProvider / options.txt）
- 状态：✅ 完成（cargo check 零警告，35 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P54 | OptionsProvider.cs（257 行） | options_txt.rs（13 方法全同步） | ✅ |

## 关键实现

1. **options.txt 读写**：读（空行/# 注释跳过、`=` 或 `:` 首分隔符、BOM 剥离）；写（恒 `key:value`+`\n`、UTF-8 无 BOM、**有序 Vec + upsert 保写回顺序**）
2. **版本可用性检查**：version manifest 解析 + introducedVersion 比较（RangePattern 正则逐字、日期 days_from_civil + 时区还原 UTC 总序、解析失败 ≡ DateTime.MinValue）
3. **多语言描述**：回退链 language→en-US→`(无描述)`
4. **ValueKind 推断**：Boolean → Range（正则含 en-dash）→ Enum（含逗号）→ Text（顺序逐字）

## 已知偏差

- 裸 `\r` 分隔不识别（.NET ReadAllLines 识别）
- 非字符串 JSON 值宽容取空串（源抛异常）
- SetOption 不可用 panic!（契约无 Result，消息逐字）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：35/35（回归）

## 下一步

**B13（最后批次）**：扩展平台（Modrinth/CurseForge/FTB）+ Modpack 安装器 + CreateXxxSource + builder 完整组装（P22）+ 移除全部过渡 allow + push 远端
