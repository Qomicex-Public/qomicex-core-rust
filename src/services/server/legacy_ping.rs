//! Legacy（旧协议）服务器 Ping（B11, P51b）
//!
//! 对应源文件：Services/Options/ServerManager.cs（Qomicex.Core.AOT，`#region Minecraft 协议通信`
//! 中 Legacy 部分：`QueryLegacyServerState` / `QueryLegacyPingHost` / `QueryLegacyFe01` /
//! `SendLegacyPingHostRequest` / `ReadLegacyResponse` / `ParseLegacyResponse` /
//! `ParseLegacyPlayerCount` / `PopulateStateFromLegacyResponse` / `LegacyServerResponse` record，
//! 以及 `CreateStatusClient` 的 Legacy 用法）。
//!
//! 覆盖的方法（源全部为 static 私有方法，不触碰实例字段）：
//! - `QueryLegacyServerState(StatusEndpoint, ServerState, string priorError)` →
//!   `pub(crate) async fn query_legacy_server_state(&StatusEndpoint, ServerState, &str) -> ServerState`
//!   （模块级函数；**本文件不定义 ServerManager struct**——定义于 servers_dat.rs，
//!   P51a mc_ping.rs 集成 trait 实现时经 `super::legacy_ping::*` 引用本文件）
//! - `QueryLegacyPingHost` / `QueryLegacyFe01` / `SendLegacyPingHostRequest` /
//!   `ReadLegacyResponse` / `ParseLegacyResponse` / `ParseLegacyPlayerCount` /
//!   `PopulateStateFromLegacyResponse` → 模块级辅助函数
//! - `StatusEndpoint` / `LegacyServerResponse` record struct → 本文件对应 struct
//!   （pub(crate)；StatusEndpoint 供 P51a 现代 Ping/SRV 复用，避免重复类型）
//!
//! Legacy 协议字节（逐字节保留，详见翻译日志 p51b-legacy-ping.md）：
//! - MC|PingHost（1.6+）：`FE 01 FA` + u16BE(11) + "MC|PingHost" UTF-16BE(22B) +
//!   u16BE(7 + hostBytes) + 0x7F(127) + u16BE(host 码元数) + host UTF-16BE + i32BE(port)
//! - FE01（1.4–1.5）：`FE 01`
//! - 响应：u8(0xFF) + u16BE(字符数) + UTF-16BE 文本（字符数 × 2 字节）；
//!   扩展格式 `§1\0{proto}\0{version}\0{motd}\0{online}\0{max}`（split '\0' ≥ 6 段）或
//!   旧格式 `{motd}§{online}§{max}`（split '§' ≥ 3 段）
//! - MOTD 逐字存入 description，**不**做 § 颜色码剥离（FlattenDescription 属于现代 JSON
//!   响应路径——P51a mc_ping.rs 范围；源 Legacy 路径同样不做剥离）
//!
//! 语义差异（详见翻译日志）：
//! - 源为同步阻塞（sync-over-async）；本实现 async + tokio::net::TcpStream
//!   （Android 兼容：纯 Rust，无 FFI、无平台专用库）
//! - 异常流：源 catch 过滤器 = InvalidDataException / IOException / SocketException /
//!   TimeoutException → 命中则 FE01 回退；连接 CTS 5s 到点抛 OperationCanceledException
//!   （不落过滤器，上抛由外层 GetServerStateByAddress 置离线）→ 本实现以等价离线结果收敛
//! - 超时：连接 5s（源 connectCts）；每次读写 5s（源 ReceiveTimeout/SendTimeout = 5000ms）
//! - 本文件函数均无 CancellationToken 参数（源 Legacy 路径无 ct，仅 CreateStatusClient
//!   内部自建 5s CTS）
//!
//! 调用点：C# 中 Legacy 回退仅存在于 GetServerStateByAddress（同步路径）的 catch
//! （`tcpConnected && ShouldFallbackToLegacy(ex)` 时），PingAsync（异步路径）无回退——
//! P51a mc_ping.rs 集成时按此接线。

use std::io::ErrorKind;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::models::local::ServerState;

/// 连接超时（源：`new CancellationTokenSource(TimeSpan.FromSeconds(5))`，连接 CTS 5s）
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 单次读写超时（源：`client.ReceiveTimeout = 5000` / `client.SendTimeout = 5000`，毫秒）
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// FE01 请求字节（源 QueryLegacyFe01：`stream.WriteByte(0xFE); stream.WriteByte(0x01);`）
const FE01_REQUEST: [u8; 2] = [0xFE, 0x01];

/// MC|PingHost 插件通道名（源 SendLegacyPingHostRequest：UTF-16BE 编码，长度 11 码元）
const PING_HOST_CHANNEL: &str = "MC|PingHost";

/// MC|PingHost 协议版本字节（源：`stream.WriteByte(127)`，1.6+ 握手）
const PING_HOST_PROTOCOL_VERSION: u8 = 127;

/// 状态查询端点（源：`private readonly record struct StatusEndpoint(
/// string ConnectHost, ushort ConnectPort, string HandshakeHost)`）。
///
/// 定义于本文件（P51b），P51a mc_ping.rs 的现代 Ping/SRV 解析复用同一定义
/// （经 `super::legacy_ping::StatusEndpoint` 引用），避免重复类型。
#[derive(Debug, Clone)]
pub(crate) struct StatusEndpoint {
    /// 连接用主机（源 ConnectHost；SRV 解析后的目标主机，用于建立 TCP 连接）
    pub(crate) connect_host: String,
    /// 连接端口（源 ConnectPort）
    pub(crate) connect_port: u16,
    /// 握手用主机（源 HandshakeHost；SRV 解析前的原域名，握手包内携带）
    pub(crate) handshake_host: String,
}

/// Legacy 响应（源：`private readonly record struct LegacyServerResponse(
/// string Motd, int OnlinePlayers, int MaxPlayers, string VersionName)`）
#[derive(Debug, Clone)]
pub(crate) struct LegacyServerResponse {
    /// MOTD 描述（源 Motd；含 § 颜色码的原始文本，不做剥离——同源行为）
    pub(crate) motd: String,
    /// 在线玩家数（源 OnlinePlayers）
    pub(crate) online_players: i32,
    /// 最大玩家数（源 MaxPlayers）
    pub(crate) max_players: i32,
    /// 版本名（源 VersionName；旧 § 分隔格式响应无版本字段 → 空串，同源）
    pub(crate) version_name: String,
}

/// Legacy Ping 错误（模块私有；对应源异常类型与 catch 过滤器映射见 `is_catchable`）。
#[derive(Debug, thiserror::Error)]
enum LegacyPingError {
    /// 源 InvalidDataException（协议/格式错误；落 Legacy catch 过滤器 → FE01 回退）
    #[error("{0}")]
    Protocol(String),
    /// 源 SocketException / IOException（TCP 层错误；落过滤器 → FE01 回退；
    /// 含 EOF：源 EndOfStreamException 是 IOException 子类）
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// 源 TimeoutException（ReceiveTimeout/SendTimeout 到点；落过滤器 → FE01 回退；
    /// 消息文本为 .NET TimeoutException 默认文本，见翻译日志 ⚠️ UNMAPPED）
    #[error("The operation has timed out.")]
    Timeout,
    /// 源 OperationCanceledException（连接 CTS 5s 到点；**不**落过滤器，C# 中继续上抛、
    /// 由外层 GetServerStateByAddress 置离线——本实现等价收敛；
    /// 消息文本近似，见翻译日志 ⚠️ UNMAPPED）
    #[error("The operation was canceled.")]
    ConnectCanceled,
}

/// Legacy 服务器状态查询（源：QueryLegacyServerState）。
///
/// 流程（逐字）：先 MC|PingHost（1.6+）查询；失败（落过滤器错误）→ FE01（1.4–1.5）回退；
/// 两者皆失败 → 离线 + 回退错误消息（IsNullOrWhiteSpace → priorError）。
/// 成功 → 在线 + ErrorMessage 清空（Ping 由 PopulateStateFromLegacyResponse 置 0）。
/// `endpoint` / `state` / `prior_error` 参数一一对应源。
///
/// ⚠️ 语义差异：源中连接 CTS 5s 到点抛 OperationCanceledException，不落 Legacy catch
/// 过滤器、继续上抛，由外层 GetServerStateByAddress 的 catch 置离线（ErrorMessage =
/// 取消异常消息）；本实现以相同离线结果收敛，函数不可失败（API 边界可观测行为一致）。
pub(crate) async fn query_legacy_server_state(
    endpoint: &StatusEndpoint,
    state: ServerState,
    prior_error: &str,
) -> ServerState {
    match query_legacy_ping_host(endpoint).await {
        Ok(response) => populate_and_mark_online(state, response),
        // 源：连接超时（OCE）不落 catch 过滤器 → 上抛 → 外层 catch 置离线
        Err(err) if !is_catchable(&err) => mark_offline(state, legacy_error_message(&err)),
        // 源：catch (InvalidDataException or IOException or SocketException or TimeoutException)
        Err(_) => match query_legacy_fe01(endpoint).await {
            Ok(response) => populate_and_mark_online(state, response),
            // 源：fallbackEx 为 OCE → 不落内层过滤器 → 上抛 → 外层 catch 置离线
            Err(fallback_err) if !is_catchable(&fallback_err) => {
                mark_offline(state, legacy_error_message(&fallback_err))
            }
            // 源：fallbackEx.Message 为空白 → priorError；否则 fallbackEx.Message
            Err(fallback_err) => {
                let message = legacy_error_message(&fallback_err);
                mark_offline(
                    state,
                    if message.trim().is_empty() {
                        prior_error.to_string()
                    } else {
                        message
                    },
                )
            }
        },
    }
}

/// MC|PingHost 查询（源：QueryLegacyPingHost）：
/// 建立状态查询连接 → 发送 MC|PingHost 握手请求 → 读取响应。
async fn query_legacy_ping_host(
    endpoint: &StatusEndpoint,
) -> Result<LegacyServerResponse, LegacyPingError> {
    let mut stream = create_status_client(endpoint).await?;
    send_legacy_ping_host_request(&mut stream, &endpoint.handshake_host, endpoint.connect_port)
        .await?;
    read_legacy_response(&mut stream).await
}

/// FE01 查询（源：QueryLegacyFe01）：
/// 建立状态查询连接 → 发送 `FE 01` 两字节 → 读取响应。
async fn query_legacy_fe01(
    endpoint: &StatusEndpoint,
) -> Result<LegacyServerResponse, LegacyPingError> {
    let mut stream = create_status_client(endpoint).await?;
    write_all_timed(&mut stream, &FE01_REQUEST).await?;
    read_legacy_response(&mut stream).await
}

/// 建立状态查询 TCP 连接（源：CreateStatusClient）。
///
/// 源：`new TcpClient` → ReceiveTimeout/SendTimeout = 5000 → 连接（CTS 5s）；
/// 连接失败 → Dispose 后重抛（SocketException/OCE 语义保留，见 `query_legacy_server_state`）。
/// ⚠️ 差异：源 CTS 5s 到点 → OperationCanceledException；本实现返回 `ConnectCanceled` 变体。
async fn create_status_client(endpoint: &StatusEndpoint) -> Result<TcpStream, LegacyPingError> {
    let connect = tokio::time::timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((endpoint.connect_host.as_str(), endpoint.connect_port)),
    )
    .await;
    match connect {
        // 源：connectCts 到点 → OperationCanceledException（不落过滤器）
        Err(_elapsed) => Err(LegacyPingError::ConnectCanceled),
        // 源：SocketException（DNS 解析失败/拒绝连接等，落过滤器）
        Ok(Err(err)) => Err(LegacyPingError::Io(err)),
        Ok(Ok(stream)) => Ok(stream),
    }
}

/// 发送 MC|PingHost 握手请求（源：SendLegacyPingHostRequest，字节布局逐字）：
///
/// ```text
/// FE 01 FA | u16BE(11) | "MC|PingHost" UTF-16BE(22B) | u16BE(7 + hostBytes.len())
///         | 0x7F(127) | u16BE(host 码元数) | host UTF-16BE | i32BE(port)
/// ```
///
/// - payloadLength = `(ushort)(7 + hostBytes.Length)`（7 = 1 协议版本 + 2 主机长度 + 4 端口）
/// - host 长度字段取 UTF-16 码元数（C# `host.Length` == hostBytes.Length / 2）
/// - port 以 4 字节大端 int 写出（源 WriteInt32BigEndian(stream, port)：ushort → int 隐式转换）
/// ⚠️ 差异：源逐段 WriteByte/Write（每段受 SendTimeout）；本实现组装完整包后一次
/// write_all（线上字节一致；超时粒度从"每段"变为"整包"，见日志）。
async fn send_legacy_ping_host_request(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
) -> Result<(), LegacyPingError> {
    // 源：Encoding.BigEndianUnicode.GetBytes(host)——UTF-16BE，码元数 × 2 字节
    let host_units: Vec<u16> = host.encode_utf16().collect();
    let mut host_bytes = Vec::with_capacity(host_units.len() * 2);
    for unit in &host_units {
        host_bytes.extend_from_slice(&unit.to_be_bytes());
    }
    // 源：payloadLength = (ushort)(7 + hostBytes.Length)（截断语义同 `as u16`）
    let payload_length: u16 = (7 + host_bytes.len()) as u16;

    let mut packet = Vec::with_capacity(3 + 2 + 22 + 2 + 1 + 2 + host_bytes.len() + 4);
    packet.push(0xFE);
    packet.push(0x01);
    packet.push(0xFA);
    packet.extend_from_slice(&11u16.to_be_bytes());
    for unit in PING_HOST_CHANNEL.encode_utf16() {
        packet.extend_from_slice(&unit.to_be_bytes());
    }
    packet.extend_from_slice(&payload_length.to_be_bytes());
    packet.push(PING_HOST_PROTOCOL_VERSION);
    packet.extend_from_slice(&(host_units.len() as u16).to_be_bytes());
    packet.extend_from_slice(&host_bytes);
    // 源 WriteInt32BigEndian(stream, port)：ushort → int 隐式转换 → 4 字节大端
    packet.extend_from_slice(&(port as u32).to_be_bytes());

    write_all_timed(stream, &packet).await
}

/// 读取 Legacy 响应（源：ReadLegacyResponse，字节布局逐字）：
///
/// ```text
/// u8(0xFF 头) | u16BE(字符数) | UTF-16BE 文本（字符数 × 2 字节，读满为止）
/// ```
///
/// 头非 0xFF → 源 InvalidDataException 文本逐字；首字节 EOF 时 C# ReadByte 返回 -1
/// （消息 "-1"，异常路径之外）→ 本实现以同文本 Protocol 错误还原。
async fn read_legacy_response(
    stream: &mut TcpStream,
) -> Result<LegacyServerResponse, LegacyPingError> {
    let header = match read_byte_optional(stream).await? {
        Some(byte) => byte as i32,
        // 源：ReadByte() 在 EOF 返回 -1（不抛异常），随后落入 header != 0xFF 分支
        None => -1,
    };
    if header != 0xFF {
        return Err(LegacyPingError::Protocol(format!(
            "Unexpected legacy response header {header}."
        )));
    }

    let length = read_u16_be(stream).await?;
    // 源：new byte[length * 2]（length 为 u16，最大 131070 字节）
    let mut bytes = vec![0u8; length as usize * 2];
    read_exact_timed(
        stream,
        &mut bytes,
        "Unexpected end of stream while reading Legacy server response.",
    )
    .await?;
    let text = decode_utf16be(&bytes);

    parse_legacy_response(&text)
}

/// 解析 Legacy 响应文本（源：ParseLegacyResponse，逐字）：
///
/// - 扩展格式（1.4+）：文本以 `§1\0` 开头 → `split('\0')` 需 ≥ 6 段：
///   `[0]="§1" [1]=协议版本(忽略) [2]=版本名 [3]=MOTD [4]=在线 [5]=最大`
/// - 旧格式（1.3 及更早）：`split('§')` 需 ≥ 3 段：`[0]=MOTD [1]=在线 [2]=最大`，
///   版本名为空串（源 VersionName 无字段）
/// - 段数不足/玩家数非法 → 源 InvalidDataException 文本逐字
fn parse_legacy_response(text: &str) -> Result<LegacyServerResponse, LegacyPingError> {
    // 源：text.StartsWith("§1\0", StringComparison.Ordinal)
    if text.starts_with("§1\0") {
        let parts: Vec<&str> = text.split('\0').collect();
        if parts.len() < 6 {
            return Err(LegacyPingError::Protocol(
                "Legacy server response does not contain all expected fields.".to_string(),
            ));
        }
        return Ok(LegacyServerResponse {
            motd: parts[3].to_string(),
            online_players: parse_legacy_player_count(parts[4], "online players")?,
            max_players: parse_legacy_player_count(parts[5], "max players")?,
            version_name: parts[2].to_string(),
        });
    }

    let segments: Vec<&str> = text.split('§').collect();
    if segments.len() < 3 {
        return Err(LegacyPingError::Protocol(
            "Legacy server response format is not recognized.".to_string(),
        ));
    }
    Ok(LegacyServerResponse {
        motd: segments[0].to_string(),
        online_players: parse_legacy_player_count(segments[1], "online players")?,
        max_players: parse_legacy_player_count(segments[2], "max players")?,
        version_name: String::new(),
    })
}

/// 解析玩家数（源：ParseLegacyPlayerCount）：
/// `int.TryParse`（NumberStyles.Integer，允许首尾空白）→ trim 后 parse 等价；
/// 失败 → 源 InvalidDataException 文本逐字。
fn parse_legacy_player_count(text: &str, value_name: &str) -> Result<i32, LegacyPingError> {
    match text.trim().parse::<i32>() {
        Ok(value) => Ok(value),
        Err(_) => Err(LegacyPingError::Protocol(format!(
            "Legacy server response contains an invalid {value_name} value '{text}'."
        ))),
    }
}

/// 用 Legacy 响应填充服务器状态（源：PopulateStateFromLegacyResponse，逐字）：
/// Description = Motd（§ 颜色码原样保留）、OnlinePlayers、MaxPlayers、Version、
/// Ping = 0（源置零，不测延迟）。
fn populate_state_from_legacy_response(
    mut state: ServerState,
    response: LegacyServerResponse,
) -> ServerState {
    state.description = response.motd;
    state.online_players = response.online_players;
    state.max_players = response.max_players;
    state.version = response.version_name;
    state.ping = 0;
    state
}

/// 查询成功收束（源：`state.IsOnline = true; state.ErrorMessage = string.Empty;`）
fn populate_and_mark_online(state: ServerState, response: LegacyServerResponse) -> ServerState {
    let mut state = populate_state_from_legacy_response(state, response);
    state.is_online = true;
    state.error_message = String::new();
    state
}

/// 查询失败收束（源：`state.IsOnline = false; state.ErrorMessage = …;`）
fn mark_offline(mut state: ServerState, message: String) -> ServerState {
    state.is_online = false;
    state.error_message = message;
    state
}

/// 源 Legacy catch 过滤器：`ex is InvalidDataException or IOException or SocketException
/// or TimeoutException` → 命中则 FE01 回退；`ConnectCanceled`（源 OperationCanceledException）
/// 不命中（上抛，由外层置离线）。
fn is_catchable(err: &LegacyPingError) -> bool {
    matches!(
        err,
        LegacyPingError::Protocol(_) | LegacyPingError::Io(_) | LegacyPingError::Timeout
    )
}

/// 取错误消息文本（源 `Exception.Message`；文本近似见 ⚠️ UNMAPPED）
fn legacy_error_message(err: &LegacyPingError) -> String {
    err.to_string()
}

/// 读单字节（源 `Stream.ReadByte()`）：EOF → None（C# 返回 -1，不抛异常）
async fn read_byte_optional(stream: &mut TcpStream) -> Result<Option<u8>, LegacyPingError> {
    let mut buf = [0u8; 1];
    match tokio::time::timeout(IO_TIMEOUT, stream.read(&mut buf)).await {
        Err(_elapsed) => Err(LegacyPingError::Timeout),
        Ok(Err(err)) => Err(LegacyPingError::Io(err)),
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(buf[0])),
    }
}

/// 读 u16 大端（源：ReadUInt16BigEndian；EOF → 源 EndOfStreamException
/// "Unexpected end of stream while reading UInt16." → 本实现同文本 Protocol 错误）
async fn read_u16_be(stream: &mut TcpStream) -> Result<u16, LegacyPingError> {
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
/// 超时（源 ReceiveTimeout 到点 → IOException/SocketException）→ Timeout；
/// EOF（源 EndOfStreamException，IOException 子类）→ Protocol（文本逐字）；
/// 其余 io 错误 → Io。
async fn read_exact_timed(
    stream: &mut TcpStream,
    buf: &mut [u8],
    eof_message: &str,
) -> Result<(), LegacyPingError> {
    match tokio::time::timeout(IO_TIMEOUT, stream.read_exact(buf)).await {
        Err(_elapsed) => Err(LegacyPingError::Timeout),
        Ok(Err(err)) if err.kind() == ErrorKind::UnexpectedEof => {
            Err(LegacyPingError::Protocol(eof_message.to_string()))
        }
        Ok(Err(err)) => Err(LegacyPingError::Io(err)),
        Ok(Ok(_)) => Ok(()),
    }
}

/// 写满缓冲（源：stream.Write* 系列；超时 → 源 SendTimeout 到点 → Timeout；其余 io 错误 → Io）
async fn write_all_timed(stream: &mut TcpStream, buf: &[u8]) -> Result<(), LegacyPingError> {
    match tokio::time::timeout(IO_TIMEOUT, stream.write_all(buf)).await {
        Err(_elapsed) => Err(LegacyPingError::Timeout),
        Ok(Err(err)) => Err(LegacyPingError::Io(err)),
        Ok(Ok(_)) => Ok(()),
    }
}

/// 解码 UTF-16BE 文本（源：Encoding.BigEndianUnicode.GetString）：
/// 非法 UTF-16 序列 → U+FFFD 替换（String::from_utf16_lossy 同源替换语义；
/// 字节数恒为偶数（length × 2），chunks_exact 不丢尾字节）
fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

