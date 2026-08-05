# CHECKPOINT_BATCH_5.md — 认证实现

- 日期：2026-08-06
- 分支：migrate/b5
- 范围：B5 services/auth（3 种认证实现）
- 状态：✅ 完成（cargo check 零警告，20 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P22 | Services/DefaultAuthProvider.cs | services/auth/offline.rs（OfflineAuthProvider） | ✅ |
| P23 | Services/MicrosoftAuthProvider.cs（220 行） | services/auth/microsoft.rs（MicrosoftAuthProvider，7 方法全 override） | ✅ |
| P24 | Services/YggdrasilAuthProvider.cs（80 行） | services/auth/yggdrasil.rs（YggdrasilAuthProvider） | ✅ |

## 关键实现

1. **离线认证**：UUID 走 util/platform.rs generate_uuid（MD5 OfflinePlayer:{name} + v3 位），AccessToken/ClientToken 用 uuid v4，user_type="legacy"
2. **Microsoft 设备码流**：6 个端点 URL/字段逐字保留（devicecode → token 轮询 → xboxlive authenticate → xsts authorize → minecraft login_with_xbox → profile）；错误码分支照抄（declined/expired_token 终止、slow_down/pending 继续）；XBL3.0 头格式保留
3. **Yggdrasil**：base URL 处理（trim + '/'）、authserver/authenticate|validate|invalidate、userType 从 User.Properties 透传、validate 仅 204 → true
4. **trait 默认方法 override**：Microsoft 实现全部 4 个默认方法（设备码/刷新）

## 依赖

B5 引入：reqwest 0.12（json/stream/gzip）、tokio full、uuid 1（v4）

## 已知偏差/待办

| 项 | 处理 |
|----|------|
| 网络/JSON 解析异常借用 Error::DownloadFailed | B6 补 Error::Http 变体（Translator 建议，已记） |
| ExpiresAt 时间格式（无 chrono，手写 civil_from_days） | B6 chrono 决策时统一复核 |
| auth/mod.rs `#![allow(dead_code)]` | P22 builder 组装后移除 |

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：20/20（B5 离线认证 3：稳定 UUID、默认用户名、validate/invalidate + B1 9 + B2 8）
- 修复：unit struct 无 new()（直接构造）；generate_uuid 为 32 位无连字符（源行为，测试断言修正）

## 下一步

B6：services/download（mirror/retry/checksum）+ services/version（manifest/locator/completer）
