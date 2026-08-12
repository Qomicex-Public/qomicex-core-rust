//! ServerManager trait 聚合实现（B11, P53）
//!
//! 对应源文件：Services/Options/ServerManager.cs（Qomicex.Core.AOT，1247 行）。
//! 本文件聚合 `crate::api::server::ServerManager` trait 的全部 15 方法实现
//! （Rust 同一 trait 只能一个 impl 块；P50 servers_dat.rs / P51 mc_ping.rs +
//! legacy_ping.rs / P52 lan_discovery.rs 以固有方法提供能力，本文件统一接线）：
//! - 8 个 servers.dat CRUD 方法 → 委托 servers_dat.rs 固有方法
//!   （`self.load_server_list()` 等：固有方法优先于 trait 方法 → 不会递归）
//! - `get_server_state_by_name` / `get_server_state_by_address` → 本文件编排
//!   （地址解析 → StatusEndpoint（SRV）→ modern 查询 → 失败回退条件 → legacy 查询；
//!   async 编排核心 = 固有方法 `get_server_state_by_address_inner`；同步方法 block_on
//!   委托之，trait 另暴露 `get_server_state_by_address_async` 供 async 上下文直调）
//! - `ping` / `ping_entry` → 本文件编排（modern 查询；异步路径无 legacy 回退，同源）
//! - `discover_lan_servers` / `discover_lan` / `resolve_srv` → 委托 lan_discovery.rs 固有方法
//!   （⚠️ 该三方法原为 lan_discovery.rs 私有，本批次追加 pub(crate) 方可跨模块委托，见日志）
//!
//! 模块级 pub(crate) 函数（源私有方法映射）：
//! - `parse_server_address` / `parse_port`（源 ParseServerAddress / ParsePort；
//!   FormatException → panic!——源在 GetServerStateByAddress 的 try 块外调用、异常上抛）
//! - `resolve_status_endpoint`（源 ResolveStatusEndpoint；SRV 经 lan_discovery.rs `resolve_srv`
//!   固有方法，⚠️ SRV 端口丢失见 ⚠️ 清单）
//! - `should_fallback_to_legacy`（源 ShouldFallbackToLegacy；⚠️ 错误类型不可直接观察，
//!   按错误消息文本分类，见函数文档）
//! - `try_get_server_name_by_address`（源 TryGetServerNameByAddress；⚠️ catch_unwind
//!   还原吞错语义，见函数文档）
//!
//! 同步 trait 方法（get_server_state_by_name / get_server_state_by_address）内以
//! `tokio::runtime::Handle::current().block_on` 阻塞驱动 async（对应源 sync-over-async
//! `GetAwaiter().GetResult()`）：⚠️ 调用方须处于 tokio runtime 上下文（无 runtime 线程 →
//! panic）且不得在 runtime worker 线程内调用（worker 内 block_on → "Cannot start a runtime
//! from within a runtime" panic）→ tokio async 上下文（axum handler 等）改用
//! `get_server_state_by_address_async`（async 编排核心 = 固有方法
//! `get_server_state_by_address_inner`）。
//!
//! Android 兼容性：纯 Rust（tokio + std），无 FFI、无平台专用库。

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::legacy_ping::{query_legacy_server_state, StatusEndpoint};
use super::mc_ping::query_modern_server_state;
use super::servers_dat::ServerManager;
use crate::api::server::ServerManager as ServerManagerApi;
use crate::error::Error;
use crate::models::local::{LanServerEntry, ServerEntry, ServerState};

/// 默认端口（源：`const ushort defaultPort = 25565`，逐字）
const DEFAULT_PORT: u16 = 25565;

/// ping_entry 固定端口（源：`PingAsync(entry.Address, 25565, ct)`，逐字）
const PING_ENTRY_PORT: i32 = 25565;

// ===== 地址解析（源 #region 地址解析与 SRV 的 ParseServerAddress / ParsePort）=====

/// 解析端口（源：ParsePort；`ushort.TryParse` → trim 后 parse 等价——.NET 允许首尾空白）。
/// 非法端口 → 源 FormatException `Invalid server port '{text}'.` → panic!（源中由
/// ResolveStatusEndpoint 在 GetServerStateByAddress try 块外调用，异常原样上抛）。
pub(crate) fn parse_port(text: &str) -> u16 {
    match text.trim().parse::<u16>() {
        Ok(port) => port,
        Err(_) => panic!("Invalid server port '{text}'."),
    }
}

/// 解析服务器地址（源：ParseServerAddress，逐字流程）：
///
/// - 空白（trim 后为空）→ 源 FormatException "Server address cannot be empty." → panic!
/// - `[` 开头（IPv6）：`]` 缺失 → panic!（"IPv6 server address is missing a closing
///   bracket."）；`]` 为末尾 → 默认端口 25565；`]` 后非 `:` → panic!（"IPv6 server
///   address must use [host]:port format."）；否则 → `[host]` 内部 + 冒号后 ParsePort
/// - 非 `[` 开头：恰好一个 `:`（first == last）→ 冒号前 host + 冒号后 ParsePort；
///   多个冒号（IPv6 无括号形式）→ 整体视为 host + 默认端口
pub(crate) fn parse_server_address(address: &str) -> (String, u16) {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        panic!("Server address cannot be empty.");
    }

    if trimmed.starts_with('[') {
        let Some(closing_index) = trimmed.find(']') else {
            panic!("IPv6 server address is missing a closing bracket.");
        };
        // 源：trimmed[1..closingIndex]（'[' 位于 0，] 的字节偏移即字符边界）
        let host = &trimmed[1..closing_index];
        // 源：closingIndex == trimmed.Length - 1 → (host, defaultPort)
        if closing_index == trimmed.len() - 1 {
            return (host.to_string(), DEFAULT_PORT);
        }
        // 源：trimmed[closingIndex + 1] != ':' → FormatException（':' 为 ASCII，按字节比较）
        if trimmed.as_bytes()[closing_index + 1] != b':' {
            panic!("IPv6 server address must use [host]:port format.");
        }
        // 源：ParsePort(trimmed[(closingIndex + 2)..])
        return (host.to_string(), parse_port(&trimmed[closing_index + 2..]));
    }

    let first_colon_index = trimmed.find(':');
    let last_colon_index = trimmed.rfind(':');
    // 源：firstColonIndex >= 0 && firstColonIndex == lastColonIndex（恰好一个冒号）
    if let Some(first) = first_colon_index {
        if Some(first) == last_colon_index {
            return (trimmed[..first].to_string(), parse_port(&trimmed[first + 1..]));
        }
    }
    // 源：无冒号 / 多个冒号 → (trimmed, defaultPort)
    (trimmed.to_string(), DEFAULT_PORT)
}

// ===== 状态端点解析（源 ResolveStatusEndpoint）=====

/// 解析状态查询端点（源：ResolveStatusEndpoint；同步方法 → async，由同步 trait 方法
/// 阻塞驱动）。
///
/// 流程（逐字）：ParseServerAddress → 端口非 25565 **或** 主机为 IP 字面量 → 直接用原值
/// （不查 SRV）；否则 SRV 解析（源 `ResolveSrvInternalAsync(host, CancellationToken.None)`）：
/// 无记录 → 原值；目标为空/空白 → 原值；成功 → `(目标, SRV 端口, 原域名)`。
/// 任一失败 → 原值（源 catch{}；resolve_srv 对所有错误路径返回 None → `.ok().flatten()`
/// 等价兜底）。
///
/// ⚠️ UNMAPPED：SRV 端口——源返回 `(Target, Port)` 并以 SRV 端口作 ConnectPort；
/// lan_discovery.rs 的 `resolve_srv` 契约只返回目标（端口被丢弃）→ 本实现 ConnectPort
/// 取 25565（该分支仅在 port == 25565 时进入；SRV 记录指定其他端口的服务器连接端口错误，
/// 见日志）。
async fn resolve_status_endpoint(_server_manager: &ServerManager, address: &str) -> StatusEndpoint {
    let (host, port) = parse_server_address(address);
    // 源：if (port != 25565 || IPAddress.TryParse(host, out _)) → 原值（不查 SRV）
    // str::parse::<IpAddr> 覆盖 IPv4/IPv6，等价 IPAddress.TryParse
    if port != DEFAULT_PORT || host.parse::<IpAddr>().is_ok() {
        return StatusEndpoint {
            connect_host: host.clone(),
            connect_port: port,
            handshake_host: host,
        };
    }

    // 源：try { ResolveSrvInternalAsync(host, CancellationToken.None).GetAwaiter().GetResult() }
    // TD-3：直接走 resolve_srv_internal（保留 SRV 端口，源 (Target, Port) 语义）
    let result = crate::services::server::lan_discovery::resolve_srv_internal(
        &host,
        &CancellationToken::new(),
    )
    .await;
    match result {
        // 源：string.IsNullOrWhiteSpace(target) → 原值
        Some((target, srv_port)) if !target.trim().is_empty() => StatusEndpoint {
            connect_host: target,
            // TD-3：SRV 端口（源 result.Value.Port）替代默认 25565
            connect_port: srv_port,
            handshake_host: host,
        },
        // 源：result null / target 空白 / catch{} → 原值
        _ => StatusEndpoint {
            connect_host: host.clone(),
            connect_port: port,
            handshake_host: host,
        },
    }
}

// ===== Legacy 回退判定（源 ShouldFallbackToLegacy）=====

/// 是否回退 Legacy 协议（源：ShouldFallbackToLegacy = `ex is InvalidDataException or
/// JsonException or EndOfStreamException`）。
///
/// ⚠️ UNMAPPED：源按异常类型判定；mc_ping.rs 的 `query_modern_server_state` 入口不可失败
/// （错误已收敛为 ServerState，类型不可直接观察，P51a 定案）→ 本实现按错误消息文本分类
/// （mc_ping.rs 的错误文本全部确定，见各变体）：
/// - `ModernPingError::Protocol`（对应源 InvalidDataException / EndOfStreamException，
///   P51a 定案：EOF 等一律落 Protocol）→ 确定性文本匹配
/// - `ModernPingError::Json`（对应源 JsonException）→ serde_json Syntax 错误 Display 带
///   ` at line {n} column {m}` 位置后缀 → 后缀识别
/// - `ModernPingError::Io` / `Timeout` / `Canceled`（源不落回退）→ 不匹配 → false
///
/// 文本分类为近似：本域内错误文本全部确定，误判仅可能来自含上述特征的外部文本（实际
/// 不会出现）。另：源回退需 `tcpConnected && ...`，Protocol/Json 错误只在连接成功后出现
/// （读/解析阶段）→ tcpConnected 由错误种类蕴含，见 get_server_state_by_address。
fn should_fallback_to_legacy(error_message: &str) -> bool {
    is_protocol_error_message(error_message) || looks_like_serde_json_error(error_message)
}

/// 协议类错误文本（mc_ping.rs `ModernPingError::Protocol` 的全部确定性文本；
/// 对应源 InvalidDataException / EndOfStreamException）
fn is_protocol_error_message(message: &str) -> bool {
    // 源：ReadStatusResponse / MeasurePing 的 packet id 校验（InvalidDataException）
    message.starts_with("Unexpected status response packet id ")
        || message.starts_with("Unexpected pong response packet id ")
        // 源：ReadExactly EOF（EndOfStreamException 语义；前缀覆盖
        // "…while reading VarInt." / "…String." / "…Int64."）
        || message.starts_with("Unexpected end of stream while reading ")
        // 源：ReadVarInt 过长 / ReadString 负数长度（InvalidDataException）
        || message == "VarInt is too large."
        || message == "String length cannot be negative."
}

/// serde_json Syntax 错误文本识别（对应源 JsonException；serde_json::Error 的 Display
/// 格式为 `{消息} at line {行} column {列}`，逐字符常量）
fn looks_like_serde_json_error(message: &str) -> bool {
    let Some(index) = message.rfind(" at line ") else {
        return false;
    };
    let rest = &message[index + " at line ".len()..];
    let Some((line, column)) = rest.split_once(" column ") else {
        return false;
    };
    line.parse::<u64>().is_ok() && column.parse::<u64>().is_ok()
}

// ===== 名称反查（源 TryGetServerNameByAddress）=====

/// 按地址反查服务器名称（源：TryGetServerNameByAddress）：
/// 读服务器列表按地址（忽略大小写）匹配 → 名称；未命中 → 空串。
///
/// ⚠️ UNMAPPED：源 catch（InvalidDataException / IOException / UnauthorizedAccessException）
/// → 空串；servers_dat.rs 的 `load_server_list` 对这些错误 panic!（P50 定案，同步契约无
/// Result）→ 以 `catch_unwind` 还原吞错语义（差异：panic hook 会向 stderr 打印一次，
/// 源为静默；返回值行为一致）。
impl ServerManager {
    /// 按地址从服务器列表找名称（源：TryGetServerNameByAddress，固有方法供 trait 编排调用）
    pub(crate) fn try_get_server_name_by_address(&self, address: &str) -> String {
        let server = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.load_server_list()
                .into_iter()
                .find(|server| server.address.eq_ignore_ascii_case(address))
        }))
        .unwrap_or_default();
        // 源：?.Name ?? string.Empty
        server.map(|server| server.name).unwrap_or_default()
    }
}

// ===== 状态查询 async 编排核心（源 GetServerStateByAddress async 块体）=====

/// 按地址获取服务器状态的 async 编排核心（源：GetServerStateByAddress 的 async 块体，
/// 自同步 trait 方法提取；由同步方法 block_on 委托 / `get_server_state_by_address_async`
/// await 调用，两入口共享同一实现）。
///
/// 流程（逐字）：构造 state（Address + Name=TryGetServerNameByAddress）→
/// ResolveStatusEndpoint（**try 块外**：地址解析失败异常上抛 → 本实现 panic! 同位置上抛）
/// → modern 查询 → 失败且（tcpConnected && ShouldFallbackToLegacy）→ legacy 查询。
///
/// ⚠️ tcpConnected：源以 `out bool` 记录 TCP 是否已连接，仅 true 且错误类型命中才回退；
/// Protocol/Json 类错误只在连接成功后出现（读/解析阶段）→ tcpConnected 由错误种类蕴含，
/// 本实现直接按错误消息分类（should_fallback_to_legacy），等价。
///
/// modern 查询的 ct 用新建令牌（源同步路径自建 5s 连接 CTS，无外部 ct）。
impl ServerManager {
    /// 按地址获取服务器状态的 async 编排核心（源：GetServerStateByAddress async 块体；
    /// 供同步方法 block_on 委托与 `get_server_state_by_address_async` await 调用）
    pub(crate) async fn get_server_state_by_address_inner(&self, address: &str) -> ServerState {
        // 源：var state = new ServerState { Address = address, Name = TryGetServerNameByAddress(address) };
        let state = ServerState {
            address: address.to_string(),
            name: self.try_get_server_name_by_address(address),
            ..ServerState::default()
        };

        // 源：var endpoint = ResolveStatusEndpoint(address);（try 块外）
        let endpoint = resolve_status_endpoint(self, address).await;

        // 源：QueryModernServerState（同步路径）；错误在内部收敛为离线状态
        let ct = CancellationToken::new();
        let modern = query_modern_server_state(&endpoint, state, &ct).await;
        // 源成功路径：直接返回（IsOnline/ErrorMessage/Ping 已填）
        if modern.is_online {
            return modern;
        }

        // 源：catch → if (tcpConnected && ShouldFallbackToLegacy(ex))
        //      → QueryLegacyServerState(endpoint, state, ex.Message)
        if should_fallback_to_legacy(&modern.error_message) {
            // 传入 modern 查询后的状态（源 catch 后同一 state 对象续用，保留可能
            // 已解析的 icon 等字段）；priorError = ex.Message = ErrorMessage
            let prior_error = modern.error_message.clone();
            query_legacy_server_state(&endpoint, modern, &prior_error).await
        } else {
            // 源：state.IsOnline = false; state.ErrorMessage = ex.Message; return state;
            modern
        }
    }
}

// ===== ServerManager trait 聚合实现（源 ServerManager.cs 编排方法）=====

/// 服务器管理器（源：`ServerManager` 具体类）。聚合 trait 全部 15 方法：
/// 固有方法（servers_dat.rs / lan_discovery.rs）直接委托，状态查询/ Ping 在本文件编排。
#[async_trait]
impl ServerManagerApi for ServerManager {
    /// 加载服务器列表（源：LoadServerList）→ 委托固有实现（servers_dat.rs）
    fn load_server_list(&self) -> Vec<ServerEntry> {
        self.load_server_list()
    }

    /// 保存服务器列表（源：SaveServerList）→ 委托固有实现（servers_dat.rs）
    fn save_server_list(&self, servers: &[ServerEntry]) {
        self.save_server_list(servers);
    }

    /// 新增或更新服务器（源：AddOrUpdateServer）→ 委托固有实现（servers_dat.rs）
    fn add_or_update_server(&self, server: &ServerEntry) {
        self.add_or_update_server(server);
    }

    /// 按地址移除服务器（源：RemoveServer）→ 委托固有实现（servers_dat.rs）
    fn remove_server(&self, address: &str) -> bool {
        self.remove_server(address)
    }

    /// 按地址获取服务器（源：GetServer）→ 委托固有实现（servers_dat.rs）
    fn get_server(&self, address: &str) -> Option<ServerEntry> {
        self.get_server(address)
    }

    /// 服务器文件（servers.dat）是否存在（源：ServerFileExists）→ 委托固有实现
    fn server_file_exists(&self) -> bool {
        self.server_file_exists()
    }

    /// 清空全部服务器（源：ClearServers）→ 委托固有实现（servers_dat.rs）
    fn clear_servers(&self) {
        self.clear_servers();
    }

    /// 获取服务器文件路径（源：GetServerFilePath）→ 委托固有实现（servers_dat.rs）
    fn get_server_file_path(&self) -> String {
        self.get_server_file_path()
    }

    /// 按名称获取服务器状态（源：GetServerStateByName；`ServerState?` → `Option<ServerState>`）。
    ///
    /// 先按名称查服务器列表（源 `StringComparison.Ordinal` = 区分大小写精确匹配，
    /// Rust `==` 字节比较等价），未命中 → None；命中 → 按条目地址委托
    /// `get_server_state_by_address`（trait 方法；源同为同步方法，内部分块阻塞驱动）。
    fn get_server_state_by_name(&self, name: &str) -> Option<ServerState> {
        // 源：LoadServerList().FirstOrDefault(entry => string.Equals(entry.Name, name, Ordinal))
        let server = self
            .load_server_list()
            .into_iter()
            .find(|entry| entry.name == name)?;
        // 源：server is null ? null : GetServerStateByAddress(server.Address)
        Some(self.get_server_state_by_address(&server.address))
    }

    /// 按地址获取服务器状态（源：GetServerStateByAddress，同步方法；sync-over-async 委托）。
    ///
    /// 流程（逐字）：构造 state（Address + Name=TryGetServerNameByAddress）→
    /// ResolveStatusEndpoint（**try 块外**：地址解析失败异常上抛 → 本实现 panic! 同位置上抛）
    /// → modern 查询 → 失败且（tcpConnected && ShouldFallbackToLegacy）→ legacy 查询。
    /// 实际编排见固有方法 `get_server_state_by_address_inner`。
    ///
    /// ⚠️ 同步包装：以 `Handle::current().block_on` 阻塞驱动 async（对应源
    /// `GetAwaiter().GetResult()`）：调用方须处于 tokio runtime 上下文（无 runtime → panic），
    /// 且**不得在 runtime worker 线程内调用**（worker 内 block_on → panic
    /// "Cannot start a runtime from within a runtime"）→ tokio async 上下文
    /// （axum handler 等）请用 `get_server_state_by_address_async`。
    fn get_server_state_by_address(&self, address: &str) -> ServerState {
        // 源：GetAwaiter().GetResult()（同步驱动 async 编排核心）
        let handle = tokio::runtime::Handle::current();
        handle.block_on(self.get_server_state_by_address_inner(address))
    }

    /// 按地址获取服务器状态的 async 编排版本（源：GetServerStateByAddress 的 async 等价；
    /// 供 tokio async 上下文直接调用，避免同步变体在 runtime worker 线程内
    /// block_on 的 "Cannot start a runtime from within a runtime" panic）。
    async fn get_server_state_by_address_async(&self, address: &str) -> ServerState {
        self.get_server_state_by_address_inner(address).await
    }

    /// Ping 指定主机与端口（源：PingAsync(string host, int port, CancellationToken ct)；
    /// `Task<ServerState?>` → `Result<Option<ServerState>, Error>`）。
    ///
    /// 流程（逐字）：构造 state（Address = host **原值不解析**；Name =
    /// TryGetServerNameByAddress(host)）→ modern 查询——连接与握手均用原 host/port，
    /// **无 SRV 解析**（源 async 路径直连）。全部失败 → 离线 + ErrorMessage（源 catch
    /// 过滤器 7 类异常）→ 恒 `Ok(Some(state))`。
    /// ct 取消检查点在 query_modern_server_state 内部（P51a：连接 select! 竞速 +
    /// 各阶段 is_cancelled 检查点），本层不重复。
    async fn ping(&self, host: &str, port: i32, ct: &CancellationToken) -> Result<Option<ServerState>, Error> {
        // 源：new ServerState { Address = host, Name = TryGetServerNameByAddress(host) }
        let state = ServerState {
            address: host.to_string(),
            name: self.try_get_server_name_by_address(host),
            ..ServerState::default()
        };
        // 源：QueryModernServerStateAsync(host, port, state, ct)
        // 连接目标 = host:port、握手主机 = host、握手端口 = (ushort)port（C# 显式截断）
        let endpoint = StatusEndpoint {
            connect_host: host.to_string(),
            connect_port: port as u16,
            handshake_host: host.to_string(),
        };
        let state = query_modern_server_state(&endpoint, state, ct).await;
        Ok(Some(state))
    }

    /// Ping 指定服务器条目（源：PingAsync(ServerEntry entry, CancellationToken ct) 重载，
    /// 重命名 `ping_entry`）：恒以固定端口 25565 委托 ping（源逐字
    /// `return await PingAsync(entry.Address, 25565, ct);`）。
    async fn ping_entry(&self, entry: &ServerEntry, ct: &CancellationToken) -> Result<Option<ServerState>, Error> {
        self.ping(&entry.address, PING_ENTRY_PORT, ct).await
    }

    /// 在局域网内发现服务器（源：DiscoverLanServers(TimeSpan timeout)）→ 委托固有实现
    /// （lan_discovery.rs）
    fn discover_lan_servers(&self, timeout: Duration) -> Vec<LanServerEntry> {
        self.discover_lan_servers(timeout)
    }

    /// 异步发现局域网服务器（源：DiscoverLanAsync）→ 委托固有实现（lan_discovery.rs；
    /// IAsyncEnumerable → mpsc 通道）
    async fn discover_lan(&self, ct: &CancellationToken) -> Result<mpsc::Receiver<LanServerEntry>, Error> {
        self.discover_lan(ct).await
    }

    /// 解析 SRV 记录获取服务器地址（源：ResolveSrvAsync）→ 委托固有实现（lan_discovery.rs）
    async fn resolve_srv(&self, host: &str, ct: &CancellationToken) -> Result<Option<String>, Error> {
        self.resolve_srv(host, ct).await
    }
}


