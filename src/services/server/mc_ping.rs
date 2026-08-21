//! Modern（现代协议）服务器 Ping 与状态查询（B11, P51a）
//!
//! 对应源文件：Services/Options/ServerManager.cs（Qomicex.Core.AOT，`#region Minecraft 协议通信`
//! 的 Modern 部分 + `#region 二进制读写辅助`）：
//! - `QueryModernServerStateAsync(string host, int port, ServerState, CancellationToken)` →
//!   `pub(crate) async fn query_modern_server_state(&StatusEndpoint, ServerState,
//!   &tokio_util::sync::CancellationToken) -> ServerState`（模块级入口，不可失败）
//! - `SendHandshake` / `SendStatusRequest` / `ReadStatusResponse` / `MeasurePing` →
//!   模块级私有 async 辅助
//! - `PopulateStateFromResponse` / `FlattenDescription` / `FlattenDescriptionObject` →
//!   模块级私有纯函数（serde_json::Value）
//! - `ReadVarInt` / `ReadString` / `ReadInt64BigEndian` / `ReadUInt16BigEndian` /
//!   `ReadExactly` → 模块级私有 async 读取辅助；`WriteVarInt` / `WriteString` /
//!   `WritePacket` / `WriteInt32BigEndian` / `WriteUInt16BigEndian` → 缓冲组装辅助
//!
//! 源同步方法 `QueryModernServerState`（含 `out bool tcpConnected` 与 Legacy 回退接线）、
//! Legacy 协议部分见 legacy_ping.rs（P51b）；SRV 解析/局域网发现见 lan_discovery.rs（P52）。
//! `StatusEndpoint`（connect_host/connect_port/handshake_host）定义于 legacy_ping.rs
//! （源 record struct 对应类型，P51b 定案），本文件经 `super::legacy_ping::StatusEndpoint` 复用。
//!
//! Modern 协议字节（逐字节保留，详见翻译日志 p51a-modern-ping.md）：
//! - 握手（SendHandshake）：`VarInt(0) VarInt(47) VarInt(len) UTF-8(host) u16BE(port) VarInt(1)`，
//!   整包以 VarInt 帧长度前缀封装（源 WritePacket）
//! - 状态请求（SendStatusRequest）：`VarInt(0)`（即单字节 0x00），帧前缀后线上为 `01 00`
//! - 状态响应（ReadStatusResponse）：帧长度 VarInt（忽略）→ packet id（须 0）→
//!   VarInt 长度 + UTF-8 JSON 字符串 → serde_json::Value
//! - Ping（MeasurePing）：请求 `VarInt(1) + i64BE(时间戳回声)`；响应 packet id 须 1 +
//!   i64BE 回声（值不校验）
//! - favicon：`data:image/png;base64,` 前缀剥离后存入 IconBase64
//!
//! 语义差异（详见翻译日志）：
//! - 源为同步 NetworkStream 读写（sync-over-async）；本实现 async + tokio::net::TcpStream
//!   （Android 兼容：纯 Rust，无 FFI、无平台专用库）
//! - 超时：连接 5s（源 `timeoutCts.CancelAfter(TimeSpan.FromSeconds(5))`）；每次读写 5s
//!   （源 `ReceiveTimeout = 5000` / `SendTimeout = 5000`）
//! - ct 取消：源链接令牌可中止任意 await（OperationCanceledException → PingAsync catch
//!   置离线）；本实现连接阶段 `tokio::select!` 竞速 `ct.cancelled()`，其余阶段以
//!   `is_cancelled()` 检查点收敛（读写受 5s 超时保护），API 边界可观测行为一致
//! - 写包粒度：源逐字段写 MemoryStream 后一次 WritePacket；本实现组装 Vec<u8> 后一次
//!   write_all（线上字节一致；超时粒度从"每字段"变为"整包"）
//! - 入口不可失败：所有错误（源 PingAsync catch 过滤器的 7 类异常）→ 离线 + ErrorMessage

use std::io::ErrorKind;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::legacy_ping::StatusEndpoint;
use crate::models::local::ServerState;

/// 连接超时（源：`timeoutCts.CancelAfter(TimeSpan.FromSeconds(5))`，链接令牌 5s 到点）
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 单次读写超时（源：`client.ReceiveTimeout = 5000` / `client.SendTimeout = 5000`，毫秒）
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// 握手包 packet id（源：`WriteVarInt(packetStream, 0)`，逐字）
const PACKET_ID_HANDSHAKE: i32 = 0;

/// 握手协议版本（源：`WriteVarInt(packetStream, 47)`，协议 47 = 1.8 及以下）
const HANDSHAKE_PROTOCOL_VERSION: i32 = 47;

/// 握手 next state（源：`WriteVarInt(packetStream, 1)`；1 = status）
const NEXT_STATE_STATUS: i32 = 1;

/// 状态请求 packet id（源：`WriteVarInt(packetStream, 0)`）
const PACKET_ID_STATUS_REQUEST: i32 = 0;

/// 状态响应 packet id（源：ReadStatusResponse `if (packetId != 0)` 校验，逐字）
const PACKET_ID_STATUS_RESPONSE: i32 = 0;

/// Ping 请求 packet id（源：MeasurePing `WriteVarInt(packetStream, 1)`）
const PACKET_ID_PING: i32 = 1;

/// Pong 响应 packet id（源：MeasurePing `if (packetId != 1)` 校验，逐字）
const PACKET_ID_PONG: i32 = 1;

/// favicon 前缀（源：`StartsWith("data:image/png;base64,", StringComparison.Ordinal)`）
const FAVICON_PREFIX: &str = "data:image/png;base64,";

/// Modern Ping 错误（模块私有；对应源 PingAsync catch 过滤器：SocketException or
/// IOException or InvalidDataException or JsonException or FormatException or TimeoutException
/// or OperationCanceledException。FormatException 由调用方 ParseServerAddress/ParsePort
/// 抛出，本文件方法不产生——见翻译日志 ⚠️ UNMAPPED）。
#[derive(Debug, thiserror::Error)]
enum ModernPingError {
    /// 源 InvalidDataException / EndOfStreamException（协议/格式错误；消息文本逐字）
    #[error("{0}")]
    Protocol(String),
    /// 源 JsonException（源 JsonDocument.Parse 失败；serde_json 错误文本为近似，见日志）
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// 源 SocketException / IOException（TCP 层错误；含 EOF 读失败）
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// 源 TimeoutException（ReceiveTimeout/SendTimeout 到点；消息文本近似，见日志 ⚠️ UNMAPPED）
    #[error("The operation has timed out.")]
    Timeout,
    /// 源 OperationCanceledException（连接 5s CancelAfter 到点 / ct 已取消；
    /// 消息文本近似，见日志 ⚠️ UNMAPPED）
    #[error("The operation was canceled.")]
    Canceled,
}

/// 现代协议服务器状态查询（源：QueryModernServerStateAsync，async 路径；逐字流程）。
///
/// 流程：TCP 连接（5s 超时）→ `SendHandshake` → `SendStatusRequest` → `ReadStatusResponse`
/// → `PopulateStateFromResponse` → `MeasurePing` → `IsOnline = true; ErrorMessage = ""`。
/// 任何错误 → 离线收敛（`IsOnline = false`，`ErrorMessage = 错误消息`）——对应源 PingAsync
/// 的 catch 过滤器（SocketException / IOException / InvalidDataException / JsonException /
/// FormatException / TimeoutException / OperationCanceledException）。
///
/// 参数映射：源 async 方法签名为 (string host, int port, ServerState, CancellationToken)，
/// 本实现按任务契约以 `StatusEndpoint` 代替（复用 legacy_ping.rs 类型）：
/// - 连接目标 = `connect_host:connect_port`（源 `ConnectAsync(host, port)`）
/// - 握手包主机 = `handshake_host`、端口 = `connect_port`
///   （源同步路径 `SendHandshake(stream, endpoint.HandshakeHost, endpoint.ConnectPort)`；
///   SRV 解析前的原域名写入握手包——与真实 MC 客户端行为一致）
pub(crate) async fn query_modern_server_state(
    endpoint: &StatusEndpoint,
    mut state: ServerState,
    ct: &CancellationToken,
) -> ServerState {
    let result = run_modern_query(endpoint, &mut state, ct).await;
    match result {
        // 源：state.IsOnline = true; state.ErrorMessage = string.Empty; return state;
        Ok(()) => {
            state.is_online = true;
            state.error_message = String::new();
            state
        }
        // 源：catch → state.IsOnline = false; state.ErrorMessage = ex.Message; return state;
        Err(err) => mark_offline(state, err.to_string()),
    }
}

/// 现代查询主流程（源：QueryModernServerStateAsync 方法体逐字步骤；返回 Result 由
/// `query_modern_server_state` 统一收束）。
async fn run_modern_query(
    endpoint: &StatusEndpoint,
    state: &mut ServerState,
    ct: &CancellationToken,
) -> Result<(), ModernPingError> {
    // 源：using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
    //      timeoutCts.CancelAfter(TimeSpan.FromSeconds(5));
    //      await client.ConnectAsync(host, port, timeoutCts.Token);
    // 本实现：ct 取消 / 5s 超时 / 连接失败 竞速（select!），等价链接令牌语义
    let connect = tokio::select! {
        _ = ct.cancelled() => return Err(ModernPingError::Canceled),
        result = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect((endpoint.connect_host.as_str(), endpoint.connect_port)),
        ) => result,
    };
    let mut stream = match connect {
        // 源：CancelAfter 5s 到点 → OperationCanceledException
        Err(_elapsed) => return Err(ModernPingError::Canceled),
        // 源：SocketException（DNS 解析失败/拒绝连接等）
        Ok(Err(err)) => return Err(ModernPingError::Io(err)),
        Ok(Ok(stream)) => stream,
    };

    // 源：SendHandshake(stream, host, (ushort)port)（同步路径用 endpoint.HandshakeHost）
    check_canceled(ct)?;
    send_handshake(&mut stream, &endpoint.handshake_host, endpoint.connect_port).await?;
    // 源：SendStatusRequest(stream)
    check_canceled(ct)?;
    send_status_request(&mut stream).await?;
    // 源：using var responseDocument = ReadStatusResponse(stream);
    //      PopulateStateFromResponse(state, responseDocument.RootElement);
    check_canceled(ct)?;
    let response = read_status_response(&mut stream).await?;
    check_canceled(ct)?;
    populate_state_from_response(state, &response);
    // 源：state.Ping = MeasurePing(stream);
    check_canceled(ct)?;
    state.ping = measure_ping(&mut stream).await?;
    Ok(())
}

/// 发送握手包（源：SendHandshake；字节布局逐字）：
///
/// ```text
/// 帧(VarInt 长度) | VarInt(0) | VarInt(47) | VarInt(len) UTF-8(host) | u16BE(port) | VarInt(1)
/// ```
///
/// - packet id = 0（Handshake）、协议版本 = 47、next state = 1（status）
/// - 源先写 MemoryStream 再 `WritePacket(stream, payload)`（帧 = VarInt(payload.Length) + payload）；
///   本实现组装完整包后一次 write_all（线上字节一致，见文件头）
async fn send_handshake(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
) -> Result<(), ModernPingError> {
    let mut payload = Vec::with_capacity(host.len() + 12);
    // 源：WriteVarInt(packetStream, 0)
    write_var_int(&mut payload, PACKET_ID_HANDSHAKE);
    // 源：WriteVarInt(packetStream, 47)
    write_var_int(&mut payload, HANDSHAKE_PROTOCOL_VERSION);
    // 源：WriteString(packetStream, host)（UTF-8，VarInt 长度前缀）
    write_string(&mut payload, host);
    // 源：WriteUInt16BigEndian(packetStream, port)
    write_uint16_be(&mut payload, port);
    // 源：WriteVarInt(packetStream, 1)（next state = status）
    write_var_int(&mut payload, NEXT_STATE_STATUS);
    // 源：WritePacket(stream, packetStream.ToArray())
    write_packet_timed(stream, &payload).await
}

/// 发送状态请求（源：SendStatusRequest；字节布局逐字）：
///
/// ```text
/// 帧(VarInt 长度=1) | VarInt(0)   —— 线上字节：01 00
/// ```
async fn send_status_request(stream: &mut TcpStream) -> Result<(), ModernPingError> {
    // 源：WriteVarInt(packetStream, 0) —— packet id 0（单字节 0x00）
    let mut payload = Vec::with_capacity(1);
    write_var_int(&mut payload, PACKET_ID_STATUS_REQUEST);
    // 源：WritePacket(stream, packetStream.ToArray())
    write_packet_timed(stream, &payload).await
}

/// 读取状态响应（源：ReadStatusResponse；字节布局逐字）：
///
/// ```text
/// 帧长度 VarInt（忽略）→ packet id VarInt（须 0）→ VarInt(长度) + UTF-8 JSON 字符串
/// ```
///
/// - packet id ≠ 0 → 源 InvalidDataException 文本逐字
/// - JSON 解析失败 → 源 JsonException（本实现 serde_json::Error，落错误变体收敛）
async fn read_status_response(stream: &mut TcpStream) -> Result<Value, ModernPingError> {
    // 源：_ = ReadVarInt(stream)（帧长度，忽略）
    read_var_int(stream).await?;
    let packet_id = read_var_int(stream).await?;
    if packet_id != PACKET_ID_STATUS_RESPONSE {
        return Err(ModernPingError::Protocol(format!(
            "Unexpected status response packet id {packet_id}."
        )));
    }
    // 源：var json = ReadString(stream); return JsonDocument.Parse(json);
    let json = read_string(stream).await?;
    let response: Value = serde_json::from_str(&json)?;
    Ok(response)
}

/// Ping 往返测量（源：MeasurePing；字节布局逐字）：
///
/// ```text
/// 请求：帧(VarInt 长度=9) | VarInt(1) | i64BE(时间戳回声)
/// 响应：帧长度 VarInt（忽略）→ VarInt(1) → i64BE(回声，忽略)
/// ```
///
/// - 时间戳生成于计时开始**之前**（源先 `Stopwatch.GetTimestamp()` 后 `Stopwatch.StartNew()`）
/// - 计时范围 = 发送 + 三次读取（帧长度/包 id/回声）全程（源 stopwatch 范围一致）
/// - 返回值 = `stopwatch.ElapsedMilliseconds`（毫秒截断），状态字段 `state.ping`（i64）同源 long
/// - packet id ≠ 1 → 源 InvalidDataException 文本逐字
async fn measure_ping(stream: &mut TcpStream) -> Result<i64, ModernPingError> {
    // 源：WriteVarInt(packetStream, 1) —— packet id 1（Ping）
    let mut packet = Vec::with_capacity(9);
    write_var_int(&mut packet, PACKET_ID_PING);
    // 源：var timestamp = Stopwatch.GetTimestamp();
    //      BitConverter.TryWriteBytes(payload, timestamp) + IsLittleEndian → Reverse
    //      == 8 字节大端 i64。⚠️ 源为 .NET 高频计数器原始值（仅回声载荷，服务端原样回传、
    //      本实现不校验），Rust std 无等价 API → 以 UNIX 纪元纳秒近似（见翻译日志 UNMAPPED）
    let timestamp: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    packet.extend_from_slice(&timestamp.to_be_bytes());

    // 源：var stopwatch = Stopwatch.StartNew();（发送前启动，组装不计入）
    let stopwatch = Instant::now();
    // 源：WritePacket(stream, packetStream.ToArray())
    write_packet_timed(stream, &packet).await?;

    // 源：_ = ReadVarInt(stream)（帧长度，忽略）
    read_var_int(stream).await?;
    let packet_id = read_var_int(stream).await?;
    if packet_id != PACKET_ID_PONG {
        return Err(ModernPingError::Protocol(format!(
            "Unexpected pong response packet id {packet_id}."
        )));
    }
    // 源：_ = ReadInt64BigEndian(stream)（回声载荷，忽略）
    read_int64_be(stream).await?;
    // 源：stopwatch.Stop(); return stopwatch.ElapsedMilliseconds;
    Ok(stopwatch.elapsed().as_millis() as i64)
}

/// 用状态响应 JSON 填充服务器状态（源：PopulateStateFromResponse，逐字）：
///
/// - `version.name`（非字符串 → GetString() null → 空串，覆盖写入）
/// - `players.online` / `players.max`：`TryGetInt32` 成功才写入（失败保持原值；
///   非整数语法/超出 Int32 范围均不写入）
/// - `description` → `FlattenDescription(description).Trim()`（源 .NET Trim，见日志近似）
/// - `favicon`：字符串且以 `data:image/png;base64,` 开头 → `IconBase64 = 前缀之后部分`
fn populate_state_from_response(state: &mut ServerState, response: &Value) {
    // 源：TryGetProperty("version") && TryGetProperty("name") →
    //      state.Version = versionName.GetString() ?? string.Empty
    if let Some(version) = response.get("version") {
        if let Some(version_name) = version.get("name") {
            state.version = version_name.as_str().unwrap_or("").to_string();
        }
    }

    // 源：TryGetProperty("players") 且成功解析（TryGetInt32 失败不修改字段）
    if let Some(players) = response.get("players") {
        if let Some(online) = players.get("online") {
            // 源：onlinePlayers.TryGetInt32(out var online) —— 仅 Int32 范围内的整数语法
            if let Some(value) = online.as_i64().and_then(|v| i32::try_from(v).ok()) {
                state.online_players = value;
            }
        }
        if let Some(max) = players.get("max") {
            if let Some(value) = max.as_i64().and_then(|v| i32::try_from(v).ok()) {
                state.max_players = value;
            }
        }
    }

    // 源：state.Description = FlattenDescription(description).Trim();
    if let Some(description) = response.get("description") {
        state.description = flatten_description(description).trim().to_string();
    }

    // 源：TryGetProperty("favicon") → GetString() 非空且 StartsWith(前缀, Ordinal)
    //      → state.IconBase64 = faviconStr[前缀长度..]
    if let Some(favicon) = response.get("favicon") {
        if let Some(favicon_str) = favicon.as_str() {
            if let Some(rest) = favicon_str.strip_prefix(FAVICON_PREFIX) {
                state.icon_base64 = Some(rest.to_string());
            }
        }
    }
}

/// 拍平描述 JSON（源：FlattenDescription，逐字）：
///
/// - String → 原字符串（GetString() ?? 空串）
/// - Object → `FlattenDescriptionObject`
/// - Array → `string.Concat(逐元素 FlattenDescription)`（无分隔符）
/// - 其余（Null/Number/Bool）→ 空串
fn flatten_description(description: &Value) -> String {
    match description {
        Value::String(text) => text.clone(),
        Value::Object(_) => flatten_description_object(description),
        Value::Array(elements) => elements.iter().map(flatten_description).collect(),
        _ => String::new(),
    }
}

/// 拍平描述对象（源：FlattenDescriptionObject，逐字）：
///
/// - `text` 属性：存在则追加（`GetString()` 非字符串 → null → StringBuilder.Append(null)
///   追加空——等价 `as_str()` None 跳过）
/// - `extra` 属性：存在且为数组 → 逐元素递归追加
fn flatten_description_object(description: &Value) -> String {
    let mut builder = String::new();

    // 源：if (description.TryGetProperty("text", out var text)) builder.Append(text.GetString());
    if let Some(text) = description.get("text") {
        if let Some(s) = text.as_str() {
            builder.push_str(s);
        }
    }

    // 源：TryGetProperty("extra") && extra.ValueKind == JsonValueKind.Array →
    //      foreach 逐元素 Append(FlattenDescription(element))
    if let Some(extra) = description.get("extra") {
        if let Value::Array(elements) = extra {
            for element in elements {
                builder.push_str(&flatten_description(element));
            }
        }
    }

    builder
}

/// 读 VarInt（源：ReadVarInt，逐字）：
///
/// - EOF → 源 EndOfStreamException "Unexpected end of stream while reading VarInt." → 同文本
/// - 第 6 字节仍带延续位 → 源 InvalidDataException "VarInt is too large." → 同文本
/// - 移位溢出语义：C# unchecked `(currentByte & 0x7F) << position`（int 截断回绕）；
///   本实现以 u64 中间量 + `as i32` 截断还原相同位模式（避免 debug 模式溢出 panic）
async fn read_var_int(stream: &mut TcpStream) -> Result<i32, ModernPingError> {
    let mut value: i32 = 0;
    let mut position = 0;
    loop {
        let byte = match read_byte_optional(stream).await? {
            // 源：stream.ReadByte() < 0 → EndOfStreamException
            None => {
                return Err(ModernPingError::Protocol(
                    "Unexpected end of stream while reading VarInt.".to_string(),
                ));
            }
            Some(byte) => byte,
        };
        // 源：value |= (currentByte & 0x7F) << position（position ≤ 28，u64 无溢出）
        value |= (((byte & 0x7F) as u64) << position) as i32;
        // 源：if ((currentByte & 0x80) == 0) return value;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        position += 7;
        // 源：if (position >= 35) throw new InvalidDataException("VarInt is too large.");
        if position >= 35 {
            return Err(ModernPingError::Protocol(
                "VarInt is too large.".to_string(),
            ));
        }
    }
}

/// 读字符串（源：ReadString，逐字）：
/// VarInt 长度 → 负数 → 源 InvalidDataException "String length cannot be negative."；
/// 读满 length 字节（EOF → 源 "Unexpected end of stream while reading String."）；
/// UTF-8 解码（非法序列 U+FFFD 替换，同 .NET Encoding.UTF8.GetString 语义）。
/// ⚠️ 源无最大长度校验（恶意服务端可致大分配），逐字保留（见日志）。
async fn read_string(stream: &mut TcpStream) -> Result<String, ModernPingError> {
    let length = read_var_int(stream).await?;
    if length < 0 {
        return Err(ModernPingError::Protocol(
            "String length cannot be negative.".to_string(),
        ));
    }
    let mut bytes = vec![0u8; length as usize];
    read_exact_timed(
        stream,
        &mut bytes,
        "Unexpected end of stream while reading String.",
    )
    .await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 读 i64 大端（源：ReadInt64BigEndian；EOF → 源
/// "Unexpected end of stream while reading Int64." → 同文本）
async fn read_int64_be(stream: &mut TcpStream) -> Result<i64, ModernPingError> {
    let mut bytes = [0u8; 8];
    read_exact_timed(
        stream,
        &mut bytes,
        "Unexpected end of stream while reading Int64.",
    )
    .await?;
    Ok(i64::from_be_bytes(bytes))
}

/// 读 u16 大端（源：ReadUInt16BigEndian；EOF → 源
/// "Unexpected end of stream while reading UInt16." → 同文本）。
/// ⚠️ 源该辅助仅被 Legacy 路径（ReadLegacyResponse）使用；P51b legacy_ping.rs 已内联等价
/// 实现（read_u16_be），本文件为任务清单要求的完整移植，当前 Modern 路径无调用点 →
/// `#[allow(dead_code)]`（见翻译日志）。
#[allow(dead_code)]
async fn read_u16_be(stream: &mut TcpStream) -> Result<u16, ModernPingError> {
    let mut bytes = [0u8; 2];
    read_exact_timed(
        stream,
        &mut bytes,
        "Unexpected end of stream while reading UInt16.",
    )
    .await?;
    Ok(u16::from_be_bytes(bytes))
}

/// 读满缓冲（源：ReadExactly，等价语义）：
/// 超时（源 ReceiveTimeout 到点 → SocketException TimedOut/TimeoutException）→ Timeout；
/// EOF（源 EndOfStreamException，IOException 子类）→ Protocol（文本逐字）；
/// 其余 io 错误 → Io。
async fn read_exact_timed(
    stream: &mut TcpStream,
    buf: &mut [u8],
    eof_message: &str,
) -> Result<(), ModernPingError> {
    match tokio::time::timeout(IO_TIMEOUT, stream.read_exact(buf)).await {
        Err(_elapsed) => Err(ModernPingError::Timeout),
        Ok(Err(err)) if err.kind() == ErrorKind::UnexpectedEof => {
            Err(ModernPingError::Protocol(eof_message.to_string()))
        }
        Ok(Err(err)) => Err(ModernPingError::Io(err)),
        Ok(Ok(_)) => Ok(()),
    }
}

/// 读单字节（源 `Stream.ReadByte()`）：EOF → None（C# 返回 -1，不抛异常）
async fn read_byte_optional(stream: &mut TcpStream) -> Result<Option<u8>, ModernPingError> {
    let mut buf = [0u8; 1];
    match tokio::time::timeout(IO_TIMEOUT, stream.read(&mut buf)).await {
        Err(_elapsed) => Err(ModernPingError::Timeout),
        Ok(Err(err)) => Err(ModernPingError::Io(err)),
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(buf[0])),
    }
}

/// 写满缓冲（源：stream.Write 系列；超时 → 源 SendTimeout 到点 → Timeout；其余 io 错误 → Io）
async fn write_all_timed(stream: &mut TcpStream, buf: &[u8]) -> Result<(), ModernPingError> {
    match tokio::time::timeout(IO_TIMEOUT, stream.write_all(buf)).await {
        Err(_elapsed) => Err(ModernPingError::Timeout),
        Ok(Err(err)) => Err(ModernPingError::Io(err)),
        Ok(Ok(_)) => Ok(()),
    }
}

/// 写帧（源：WritePacket = `WriteVarInt(payload.Length)` + payload，一次 write_all 发送）
async fn write_packet_timed(stream: &mut TcpStream, payload: &[u8]) -> Result<(), ModernPingError> {
    let mut frame = Vec::with_capacity(payload.len() + 5);
    write_var_int(&mut frame, payload.len() as i32);
    frame.extend_from_slice(payload);
    write_all_timed(stream, &frame).await
}

/// 写 VarInt（源：WriteVarInt，逐字）：`unchecked((uint)value)` 无符号补码写出，
/// 负数最多 5 字节；每字节 7 位有效 + 延续位（最高字节为 0x80 语义）。
fn write_var_int(buf: &mut Vec<u8>, value: i32) {
    // 源：var unsignedValue = unchecked((uint)value);
    let mut unsigned_value = value as u32;
    // 源：do { … } while (unsignedValue != 0);
    loop {
        let mut byte = (unsigned_value & 0x7F) as u8;
        unsigned_value >>= 7;
        if unsigned_value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if unsigned_value == 0 {
            break;
        }
    }
}

/// 写字符串（源：WriteString = `Encoding.UTF8.GetBytes` + `WriteVarInt(len)` + Write）
fn write_string(buf: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    write_var_int(buf, bytes.len() as i32);
    buf.extend_from_slice(bytes);
}

/// 写 u16 大端（源：WriteUInt16BigEndian）
fn write_uint16_be(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_be_bytes());
}

/// 写 i32 大端（源：WriteInt32BigEndian）。
/// ⚠️ 源该辅助仅被 Legacy 路径（SendLegacyPingHostRequest 的 port 字段）使用；P51b
/// legacy_ping.rs 已内联等价实现（`(port as u32).to_be_bytes()`），本文件为任务清单要求的
/// 完整移植，当前 Modern 路径无调用点 → `#[allow(dead_code)]`（见翻译日志）。
#[allow(dead_code)]
fn write_int32_be(buf: &mut Vec<u8>, value: i32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

/// 查询失败收束（源：`state.IsOnline = false; state.ErrorMessage = ex.Message;`）
fn mark_offline(mut state: ServerState, message: String) -> ServerState {
    state.is_online = false;
    state.error_message = message;
    state
}

/// ct 取消检查点（源：链接令牌已取消 → 后续 `await client.ConnectAsync(...)` 等
/// 抛 OperationCanceledException → PingAsync catch 置离线；本实现以相同结果收敛）
fn check_canceled(ct: &CancellationToken) -> Result<(), ModernPingError> {
    if ct.is_cancelled() {
        Err(ModernPingError::Canceled)
    } else {
        Ok(())
    }
}
