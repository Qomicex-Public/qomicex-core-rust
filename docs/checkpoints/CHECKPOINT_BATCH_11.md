# CHECKPOINT_BATCH_11.md — 服务器域实现

- 日期：2026-08-06
- 分支：migrate/b11
- 范围：B11 services/server（servers.dat / MC Ping / LAN / SRV / DNS）
- 状态：✅ 完成（cargo check 零警告，35 测试通过）

## 交付内容

| 原子包 | 源（.NET） | 目标（Rust） | 状态 |
|--------|-----------|-------------|------|
| P50 | ServerManager.cs servers.dat 部分 | servers_dat.rs（CRUD + NBT + gzip + _old 回退） | ✅ |
| P51a | Modern Ping（3 次重试后成功） | mc_ping.rs（握手/状态/测量/JSON 解析） | ✅ |
| P51b | Legacy Ping | legacy_ping.rs（FE/FE01 双模式 + UTF-16BE + § 解析） | ✅ |
| P52 | LAN + SRV + 自定义 DNS | lan_discovery.rs（563 行：UDP 多播 + DNS 二进制） | ✅ |
| P53 | 编排 + trait 聚合 | server.rs（15 方法聚合 + modern→legacy 回退） | ✅ |

## 特殊兼容点

1. **MC 协议**：握手（protocol 47 + host/port/nextState）+ status + ping 往返测量；**Modern→Legacy 回退**（ShouldFallbackToLegacy 错误分类）
2. **Legacy 双模式**：PING_HOST（FE 01 FA + "MC|PingHost" UTF-16BE + 码元数长度）+ FE01；`§1\0` 扩展格式 ≥6 段解析
3. **自定义 DNS 客户端**：12 字节头（无 RD 位）+ QNAME + SRV(33)/IN(1) + 0xC0 压缩指针；系统 DNS 读取（win: ipconfig 近似 / unix: resolv.conf）
4. **LAN 多播**：224.0.2.60:4445 + socket2 SO_REUSEADDR（std UDP 无此 API）+ [MOTD]/[AD] 报文解析
5. **servers.dat**：NBT 结构（name/ip/icon String + acceptTextures/hidden Byte）+ gzip 魔数探测 + servers.dat_old 回退

## 已知偏差（已记录）

- SRV 端口丢失（trait resolve_srv 只返回目标，ConnectPort 取 25565）
- DNS ID 初值 0（源 Random 1-65535）；Windows DNS 用 ipconfig 近似（locale 脆弱）
- 错误消息文本近似（.NET/Winsock 异常文本无法逐字）
- 同步方法需 tokio runtime 上下文（Handle::current）

## 依赖

socket2 0.5（SO_REUSEADDR）；tokio-util（CancellationToken——B3 的 ct: () 占位正式替换）

## 验证

- `cargo check`：0 error 0 warning
- `cargo test`：35/35（回归）

## 下一步

B12：游戏设置实现（OptionsProvider / options.txt）
