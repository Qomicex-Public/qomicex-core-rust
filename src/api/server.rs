//! ServerManager trait：servers.dat CRUD + Ping + LAN（B3）
//!
//! 对应源文件：Public/Services/IServerManager.cs（namespace Qomicex.Core.AOT.Public.Services）
//!
//! 方法映射表：
//! - `List<ServerEntry> LoadServerList()` → `load_server_list(&self) -> Vec<ServerEntry>`
//! - `void SaveServerList(IReadOnlyList<ServerEntry> servers)` → `save_server_list(&self, servers: &[ServerEntry])`
//! - `void AddOrUpdateServer(ServerEntry server)` → `add_or_update_server(&self, server: &ServerEntry)`
//! - `bool RemoveServer(string address)` → `remove_server(&self, address: &str) -> bool`
//! - `ServerEntry? GetServer(string address)` → `get_server(&self, address: &str) -> Option<ServerEntry>`
//! - `bool ServerFileExists()` → `server_file_exists(&self) -> bool`
//! - `void ClearServers()` → `clear_servers(&self)`
//! - `string GetServerFilePath()` → `get_server_file_path(&self) -> String`
//! - `ServerState? GetServerStateByName(string name)` → `get_server_state_by_name(&self, name: &str) -> Option<ServerState>`
//! - `ServerState GetServerStateByAddress(string address)` → `get_server_state_by_address(&self, address: &str) -> ServerState`
//! - `Task<ServerState?> PingAsync(string host, int port, CancellationToken ct)`
//!   → `async fn ping(&self, host: &str, port: i32, ct: &tokio_util::sync::CancellationToken) -> Result<Option<ServerState>, Error>`
//! - `Task<ServerState?> PingAsync(ServerEntry entry, CancellationToken ct)`（重载 → 改名
//!   `ping_entry`，见日志重命名决策）
//!   → `async fn ping_entry(&self, entry: &ServerEntry, ct: &tokio_util::sync::CancellationToken) -> Result<Option<ServerState>, Error>`
//! - `IReadOnlyList<LanServerEntry> DiscoverLanServers(TimeSpan timeout)`
//!   → `discover_lan_servers(&self, timeout: Duration) -> Vec<LanServerEntry>`（TimeSpan → std::time::Duration）
//! - `IAsyncEnumerable<LanServerEntry> DiscoverLanAsync(CancellationToken ct = default)`
//!   → `async fn discover_lan(&self, ct: &tokio_util::sync::CancellationToken) -> Result<mpsc::Receiver<LanServerEntry>, Error>`
//!   （流式 → mpsc 通道，基于 ADR-001 D3 先例，见日志）
//! - `Task<string?> ResolveSrvAsync(string host, CancellationToken ct)`
//!   → `async fn resolve_srv(&self, host: &str, ct: &tokio_util::sync::CancellationToken) -> Result<Option<String>, Error>`
//!
//! 签名规则（B3 既有约定）：`Task<T>` → `Result<T, Error>`；`Task<T?>` → `Result<Option<T>, Error>`；
//! 同步方法直接映射（C# 错误为异常语义，Rust 侧暂不包装 Result，待实现批次定案）。
//!
//! ⚠️ 缺失标注：ServerEntry / ServerState / LanServerEntry 定义于源
//! Services/Options/ServiceTypes.cs（"#region 服务器类型"）。
//! MAPPING_TABLE.yaml models 段仅登记 ServerEntry/ServerState → models::local；
//! LanServerEntry 未登记（UNMAPPED，见下）。Rust 侧 models/local.rs 尚无这些模型
//! （其文件头注明"Local/ServerEntry 等另在后续批"）→ 本文件按映射表目标路径引用
//! `crate::models::local`，模型批次补齐前不编译，属预期。
//!
//! ⚠️ UNMAPPED：
//! - `CancellationToken`：Cargo.toml 无 tokio-util（Cargo.toml 修改被禁止），项目无既有映射，
//!   占位参数 `ct: &tokio_util::sync::CancellationToken`（候选：tokio_util::sync::CancellationToken，待依赖批次定案）。
//! - `LanServerEntry`：模型映射表未登记，按命名约定建议 `crate::models::local::LanServerEntry`
//!   （建议路径，未定案，模型批次登记时确认）。

use async_trait::async_trait;
use crate::error::Error;
use crate::models::local::{LanServerEntry, ServerEntry, ServerState};
use std::time::Duration;
use tokio::sync::mpsc;
/// 服务器管理器（源：IServerManager）。
/// 负责 servers.dat 的增删改查/读写、服务器状态查询（Ping）、
/// LAN 局域网服务器发现与 SRV 记录解析。
#[async_trait]
pub trait ServerManager: Send + Sync {
    /// 加载服务器列表（源：LoadServerList，同步方法）
    fn load_server_list(&self) -> Vec<ServerEntry>;

    /// 保存服务器列表（源：SaveServerList，同步方法；
    /// `IReadOnlyList<ServerEntry>` 只读集合 → `&[ServerEntry]` 借用）
    fn save_server_list(&self, servers: &[ServerEntry]);

    /// 新增或更新服务器（源：AddOrUpdateServer，同步方法）
    fn add_or_update_server(&self, server: &ServerEntry);

    /// 按地址移除服务器，返回是否移除成功（源：RemoveServer，同步方法）
    fn remove_server(&self, address: &str) -> bool;

    /// 按地址获取服务器（源：GetServer，返回 `ServerEntry?` → `Option<ServerEntry>`，同步方法）
    fn get_server(&self, address: &str) -> Option<ServerEntry>;

    /// 服务器文件（servers.dat）是否存在（源：ServerFileExists，同步方法）
    fn server_file_exists(&self) -> bool;

    /// 清空全部服务器（源：ClearServers，同步方法）
    fn clear_servers(&self);

    /// 获取服务器文件路径（源：GetServerFilePath，同步方法）
    fn get_server_file_path(&self) -> String;

    /// 按名称获取服务器状态（源：GetServerStateByName，返回 `ServerState?` → `Option<ServerState>`，同步方法）
    fn get_server_state_by_name(&self, name: &str) -> Option<ServerState>;

    /// 按地址获取服务器状态（源：GetServerStateByAddress，同步方法）
    fn get_server_state_by_address(&self, address: &str) -> ServerState;

    /// Ping 指定主机与端口，返回服务器状态（源：PingAsync(string host, int port, CancellationToken ct)；
    /// `Task<ServerState?>` → `Result<Option<ServerState>, Error>`）
    async fn ping(&self, host: &str, port: i32, ct: &tokio_util::sync::CancellationToken) -> Result<Option<ServerState>, Error>;

    /// Ping 指定服务器条目，返回服务器状态（源：PingAsync(ServerEntry entry, CancellationToken ct) 重载，
    /// 重命名 `ping_entry` 区分）
    async fn ping_entry(&self, entry: &ServerEntry, ct: &tokio_util::sync::CancellationToken) -> Result<Option<ServerState>, Error>;

    /// 在局域网内发现服务器（源：DiscoverLanServers(TimeSpan timeout)，同步方法；
    /// `TimeSpan` → `std::time::Duration`）
    fn discover_lan_servers(&self, timeout: Duration) -> Vec<LanServerEntry>;

    /// 异步发现局域网服务器（源：DiscoverLanAsync，`IAsyncEnumerable<LanServerEntry>` 流式 →
    /// `tokio::sync::mpsc::Receiver<LanServerEntry>` 通道，基于 ADR-001 D3 mpsc 先例；
    /// 注意：流中途错误细节在通道映射下丢失，仅首错以 Result 返回，语义差异见翻译日志）
    async fn discover_lan(&self, ct: &tokio_util::sync::CancellationToken) -> Result<mpsc::Receiver<LanServerEntry>, Error>;

    /// 解析 SRV 记录获取服务器地址（源：ResolveSrvAsync(string host, CancellationToken ct)；
    /// `Task<string?>` → `Result<Option<String>, Error>`）
    async fn resolve_srv(&self, host: &str, ct: &tokio_util::sync::CancellationToken) -> Result<Option<String>, Error>;
}




