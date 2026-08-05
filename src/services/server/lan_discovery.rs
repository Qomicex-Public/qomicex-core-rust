//! 局域网游戏发现（UDP 多播）+ SRV 解析 + 自定义 DNS 客户端（B11, P52）
//!
//! 对应源文件：Services/Options/ServerManager.cs（Qomicex.Core.AOT，`#region 局域网发现`、
//! `#region 地址解析与 SRV`、`#region 局域网发现辅助` 三节）
//!
//! 本文件覆盖的方法（源全部为 static，不触碰实例字段）：
//! - `DiscoverLanServers(TimeSpan timeout)` → `discover_lan_servers(&self, Duration)`
//! - `DiscoverLanAsync(CancellationToken)` → `discover_lan(&self, ct) -> mpsc::Receiver`
//! - `ResolveSrvAsync(string host, CancellationToken)` → `resolve_srv(&self, host, ct)`
//! - 私有辅助：`ResolveSrvInternalAsync` / `GetDnsServers` / `BuildDnsQuery` /
//!   `EncodeDnsName` / `ParseDnsSrvResponse` / `DecodeDnsName` /
//!   `CreateLanDiscoveryClient` / `ParseLanBroadcast` / `ExtractLanTag`
//!
//! 契约：src/api/server.rs（ServerManager trait，`ct: &tokio_util::sync::CancellationToken`）
//! 结构体：`pub(crate) struct ServerManager` 定义于 src/services/server/servers_dat.rs
//! （P50 并行批次）。本文件三方法为固有方法（pub(crate)，P53 起）供 server.rs
//! （P53 trait 聚合 impl）委托调用；trait 的完整 15 方法实现集中于 server.rs
//! （Rust 同一 trait 只能一个 impl 块，P50/P51/P52 的固有方法统一接线）。
//!
//! 语义差异与 ⚠️ UNMAPPED（详见翻译日志 p52-lan-srv-dns.md）：
//! - DNS 查询 ID 初值：源 `Random().Next(1, 65535)` → `AtomicU16::new(0)`（无 rand 依赖）⚠️
//! - Windows 系统 DNS 枚举：源 .NET 托管 API（NetworkInterface.DnsAddresses）无 Rust 等价
//!   → 解析 `ipconfig /all` 输出近似；Linux/Android → /etc/resolv.conf ⚠️
//! - `ReuseAddress`：源在 bind **前**设置，Rust 无 bind 前配置（需 socket2）→ bind 后设置，
//!   Windows 多实例共享 4445 端口可能失败（Android/Linux 无影响）⚠️
//! - `SendTimeout = 3000`：tokio `send_to` 无超时（UDP 小报文实际不阻塞）⚠️
//! - `discover_lan_servers`：源以 CTS(timeout) 阻塞消费 IAsyncEnumerable → 本实现用
//!   std::net::UdpSocket 同步镜像（read_timeout = 剩余时间），不依赖 tokio runtime；
//!   超时触发粒度有差异（见日志）
//! - DNS/多播二进制布局逐字节忠实移植（无 RD 位、SRV QTYPE=33、0xC0 压缩指针等，见日志）
//!
//! Android 兼容性：纯 Rust + tokio::net::UdpSocket + `join_multicast_v4`（Android 支持）；
//! 无 FFI、无平台专用库；系统 DNS 读取走 /etc/resolv.conf。

use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::models::local::LanServerEntry;
use crate::services::server::servers_dat::ServerManager;

/// LAN 发现多播组地址（源：`IPAddress.Parse("224.0.2.60")`，逐字）
const LAN_MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 2, 60);

/// LAN 发现监听端口（源：`new IPEndPoint(IPAddress.Any, 4445)`，逐字）
const LAN_DISCOVERY_PORT: u16 = 4445;

/// DNS 服务端口（源：`new IPEndPoint(addr, 53)`）
const DNS_PORT: u16 = 53;

/// DNS 兜底服务器（源：`IPAddress.Parse("8.8.8.8")`）
const DNS_FALLBACK_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));

/// DNS 接收超时（源：`udp.Client.ReceiveTimeout = 3000`，毫秒）
const DNS_RECEIVE_TIMEOUT: Duration = Duration::from_millis(3000);

/// DNS SRV 记录类型值（源：BuildDnsQuery 尾部 `query[^3] = 33` / ParseDnsSrvResponse `ansType == 33`）
const DNS_TYPE_SRV: u16 = 33;

/// LAN 广播报文接收缓冲大小（源 UdpClient 无上限，整个 UDP 数据报返回；
/// MC LAN 广播报文远小于此，固定缓冲为等价近似）
const LAN_RECV_BUFFER: usize = 4096;

/// DNS 响应接收缓冲大小（同上；无 EDNS 查询时 RFC 1035 上限 512 字节）
const DNS_RECV_BUFFER: usize = 4096;

/// mpsc 通道容量（源 IAsyncEnumerable 为无界拉式流；本实现有界背压，见日志语义差异）
const LAN_CHANNEL_CAPACITY: usize = 64;

/// DNS 查询 ID 计数器（源：`static int _dnsQueryId` + `Interlocked.Increment`）。
/// ⚠️ UNMAPPED：源初值为 `new Random().Next(1, 65535)`，本实现初值 0（无 rand 依赖）；
/// 环绕 fetch_add 保证进程内 ID 唯一性，与源等价。
static DNS_QUERY_ID: AtomicU16 = AtomicU16::new(0);

/// LAN 发现 / SRV 解析 / DNS（源 ServerManager.cs 静态方法，无实例状态）
impl ServerManager {
    /// 在局域网内发现服务器（源：DiscoverLanServers(TimeSpan timeout)，同步方法）。
    ///
    /// 源以 `CancellationTokenSource(timeout)` 阻塞消费 IAsyncEnumerable 直到超时；
    /// 本实现用 std::net::UdpSocket 同步镜像（不依赖 tokio runtime）：每次 recv 前
    /// 设置 `read_timeout = 剩余时间`，超时后退出（源为取消令牌到点即停，粒度差异见日志）。
    pub(crate) fn discover_lan_servers(&self, timeout: Duration) -> Vec<LanServerEntry> {
        let mut entries: Vec<LanServerEntry> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // 源：CreateLanDiscoveryClient 抛 SocketException/IOException → yield break（空列表）
        let socket = match create_lan_discovery_client_std() {
            Ok(s) => s,
            Err(_) => return entries,
        };

        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; LAN_RECV_BUFFER];

        loop {
            // 源：while (!cancellationToken.IsCancellationRequested)
            let now = Instant::now();
            if now >= deadline {
                break;
            }

            // 等价于源 ReceiveAsync(ct) 到点抛 OperationCanceledException → yield break
            let remaining = deadline - now;
            let _ = socket.set_read_timeout(Some(remaining));

            let (len, src) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                // 源未设置 ReceiveTimeout，这里 WouldBlock/TimedOut 仅来自自设 read_timeout
                // （timeout 到期信号）→ 回到循环头由 deadline 判断退出
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    continue;
                }
                // 源：catch (SocketException) → yield break
                Err(_) => break,
            };

            // 源：Encoding.UTF8.GetString(result.Buffer)（非法 UTF-8 → 替换符，同 from_utf8_lossy）
            let payload = String::from_utf8_lossy(&buf[..len]);
            if let Some(entry) = parse_lan_broadcast(&payload, &src.ip().to_string()) {
                // 源：$"{entry.Address}|{entry.Port}|{entry.Motd}" 去重后才 yield
                let key = format!("{}|{}|{}", entry.address, entry.port, entry.motd);
                if seen.insert(key) {
                    entries.push(entry);
                }
            }
        }

        entries
    }

    /// 异步发现局域网服务器（源：DiscoverLanAsync，IAsyncEnumerable 流式 →
    /// `tokio::sync::mpsc::Receiver` 通道，契约 ADR-001 D3 mpsc 先例）。
    ///
    /// 源所有错误路径均为 `yield break`（空流，不抛异常）→ 本实现恒返回 `Ok(rx)`：
    /// 创建客户端失败/接收失败/取消 → 发送端 drop → rx 立即关流（空流等价）；
    /// 契约 `Result` 中的 `Err` 在源语义下不会出现。
    pub(crate) async fn discover_lan(&self, ct: &CancellationToken) -> Result<mpsc::Receiver<LanServerEntry>, Error> {
        let (tx, rx) = mpsc::channel::<LanServerEntry>(LAN_CHANNEL_CAPACITY);

        // 克隆令牌供任务持有（tokio::spawn 需 'static；克隆共享同一取消状态，同源实例语义）
        let ct = ct.clone();

        tokio::spawn(async move {
            // 源：CreateLanDiscoveryClient 失败 → yield break（空流）
            let socket = match create_lan_discovery_client().await {
                Ok(s) => s,
                Err(_) => return,
            };

            let mut seen: HashSet<String> = HashSet::new();
            let mut buf = [0u8; LAN_RECV_BUFFER];

            // 源：while (!cancellationToken.IsCancellationRequested)
            while !ct.is_cancelled() {
                let (len, src) = tokio::select! {
                    // 源：await client.ReceiveAsync(cancellationToken)
                    result = socket.recv_from(&mut buf) => match result {
                        // 源：catch (SocketException) → yield break
                        Ok(v) => v,
                        Err(_) => return,
                    },
                    // 源：catch (OperationCanceledException) → yield break
                    _ = ct.cancelled() => return,
                };

                // 源：Encoding.UTF8.GetString(result.Buffer) + RemoteEndPoint.Address.ToString()
                let payload = String::from_utf8_lossy(&buf[..len]);
                if let Some(entry) = parse_lan_broadcast(&payload, &src.ip().to_string()) {
                    let key = format!("{}|{}|{}", entry.address, entry.port, entry.motd);
                    if seen.insert(key) {
                        // 源为拉式 yield（无背压）；本实现 send().await 背压暂停，
                        // 保证条目不丢失；接收方 drop → send 失败 → 停止监听
                        if tx.send(entry).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// 解析 SRV 记录获取服务器地址（源：ResolveSrvAsync(string host, CancellationToken ct)；
    /// `Task<string?>` → `Result<Option<String>, Error>`）
    pub(crate) async fn resolve_srv(&self, host: &str, ct: &CancellationToken) -> Result<Option<String>, Error> {
        let result = resolve_srv_internal(host, ct).await;
        // 源：return result?.Target
        Ok(result.map(|(target, _port)| target))
    }
}

/// 创建 LAN 发现 UDP 客户端（源：CreateLanDiscoveryClient）。
///
/// 源顺序：`new UdpClient(AddressFamily.InterNetwork)` →
/// `SetSocketOption(ReuseAddress, true)` + `ExclusiveAddressUse = false` →
/// `Bind(IPAddress.Any, 4445)` → `JoinMulticastGroup("224.0.2.60")`。
/// ⚠️ UNMAPPED：源在 bind **之前**设置 ReuseAddress（Windows 多实例共享 4445 依赖此顺序），
/// Rust std/tokio 无 bind 前配置 API（需 socket2 依赖）→ 本实现 bind 后设置，
/// Windows 多实例场景可能失败（Android/Linux 不受影响）。
async fn create_lan_discovery_client() -> std::io::Result<tokio::net::UdpSocket> {
    let socket = create_lan_discovery_client_std()?;
    tokio::net::UdpSocket::from_std(socket)
}

/// LAN 发现客户端（std 同步镜像，供 discover_lan_servers 使用；与 tokio 版本同配置）
/// 用 socket2 设置 SO_REUSEADDR（std::net::UdpSocket 无此 API；UDP 多播共享端口必需）
fn create_lan_discovery_client_std() -> std::io::Result<std::net::UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    socket.bind(&socket2::SockAddr::from(std::net::SocketAddr::new(
        Ipv4Addr::UNSPECIFIED.into(),
        LAN_DISCOVERY_PORT,
    )))?;
    socket.join_multicast_v4(&LAN_MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    let socket: std::net::UdpSocket = socket.into();
    Ok(socket)
}

/// 解析 LAN 广播报文（源：ParseLanBroadcast）。
///
/// 报文格式：`[MOTD]...[/MOTD][AD]<端口>[/AD]`（逐字）。
/// - MOTD 标签缺失 → 源 `?? "missing no"` 默认值
/// - AD 标签缺失或非数字 → null（源 `int.TryParse` 失败返回 null）
/// - `Address` = 数据报源地址；`DisplayAddress` = `"{address}:{port}"`
fn parse_lan_broadcast(payload: &str, source_address: &str) -> Option<LanServerEntry> {
    let motd = extract_lan_tag(payload, "MOTD").unwrap_or_else(|| "missing no".to_string());
    let port_text = extract_lan_tag(payload, "AD")?;
    // 源 int.TryParse（整数，允许首尾空白）→ Rust 侧 trim 后 parse::<i32> 等价
    let port: i32 = port_text.trim().parse().ok()?;

    Some(LanServerEntry {
        motd,
        address: source_address.to_string(),
        port,
        display_address: format!("{source_address}:{port}"),
    })
}

/// 提取广播报文中的标签内容（源：ExtractLanTag）。
///
/// 源语义：找 `[{tag}]` 与 `[/{tag}]`（Ordinal），返回中间文本；任一缺失 → null。
fn extract_lan_tag(payload: &str, tag_name: &str) -> Option<String> {
    let start_tag = format!("[{tag_name}]");
    let end_tag = format!("[/{tag_name}]");

    let start_index = payload.find(&start_tag)? + start_tag.len();
    // 源：payload.IndexOf(endTag, startIndex)（从 startIndex 起搜）→ 等价于子串内查找后偏移
    let end_index = payload[start_index..].find(&end_tag)? + start_index;

    Some(payload[start_index..end_index].to_string())
}

/// SRV 内部解析（源：ResolveSrvInternalAsync）。
///
/// 流程（逐字）：`_minecraft._tcp.{host}` 编码 → BuildDnsQuery → 绑定临时 UDP 端口 →
/// 向**全部**系统 DNS 服务器顺序发送（源 foreach）→ 3 秒内接收首个响应（源 ReceiveTimeout
/// 3000ms + ReceiveAsync(ct)）→ ParseDnsSrvResponse。任一环节失败 → None（源全部错误
/// 路径 null）。
async fn resolve_srv_internal(host: &str, ct: &CancellationToken) -> Option<(String, u16)> {
    let query_name = encode_dns_name(&format!("_minecraft._tcp.{host}"));
    let query = build_dns_query(&query_name);

    // 源：new UdpClient()（发送时隐式绑定）→ 等价显式绑定临时端口；失败 → 外层 catch → null
    let socket = match tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(s) => s,
        Err(_) => return None,
    };

    let dns_servers = get_dns_servers();
    for dns_server in &dns_servers {
        // 源：await udp.SendAsync(...)（SocketException/IOException → 外层 catch → null）
        if socket.send_to(&query, dns_server).await.is_err() {
            return None;
        }
    }

    let mut buf = [0u8; DNS_RECV_BUFFER];
    let received = tokio::time::timeout(DNS_RECEIVE_TIMEOUT, async {
        tokio::select! {
            // 源：ReceiveAsync(ct) → OperationCanceledException → 外层 catch → null
            _ = ct.cancelled() => None,
            result = socket.recv_from(&mut buf) => match result {
                // 源：catch (SocketException)/(IOException) → return null
                Ok((len, _)) => Some(len),
                Err(_) => None,
            },
        }
    })
    .await;

    // 源：ReceiveTimeout 3000ms 到点 → SocketException(TimedOut) → null
    let len = match received {
        Ok(Some(len)) => len,
        _ => return None,
    };

    // 源：return ParseDnsSrvResponse(result.Buffer)
    parse_dns_srv_response(&buf[..len])
}

/// 读取系统 DNS 服务器列表（源：GetDnsServers）。
///
/// 源用 .NET 托管 API 枚举所有已启用网卡的 DnsAddresses（去重，端口 53），
/// 空列表兜底 `8.8.8.8:53`。Rust std 无等价枚举 API：
/// - Linux/Android：解析 `/etc/resolv.conf` 的 `nameserver` 行（glibc 同样读取该文件，语义最近）
/// - Windows：解析 `ipconfig /all` 输出 ⚠️ UNMAPPED（源为托管 API；ipconfig 解析为近似，
///   输出格式随系统/区域设置变化，见日志）
fn get_dns_servers() -> Vec<SocketAddr> {
    let mut servers: Vec<SocketAddr> = Vec::new();

    for addr in system_dns_addresses() {
        let endpoint = SocketAddr::new(addr, DNS_PORT);
        if !servers.contains(&endpoint) {
            servers.push(endpoint);
        }
    }

    // 源：servers.Count == 0 → new IPEndPoint(IPAddress.Parse("8.8.8.8"), 53)
    if servers.is_empty() {
        servers.push(SocketAddr::new(DNS_FALLBACK_ADDR, DNS_PORT));
    }

    servers
}

/// 系统 DNS 地址枚举（平台分支）
#[cfg(target_os = "windows")]
fn system_dns_addresses() -> Vec<IpAddr> {
    // ⚠️ UNMAPPED：源 NetworkInterface.GetIPProperties().DnsAddresses（GetAdaptersAddresses
    // P/Invoke）；Rust std 无等价 → ipconfig /all 输出近似。
    // 解析规则（对英文/中文等区域设置均适用，见日志）：
    // 1) 含 "DNS" 的标签行（如 "DNS Servers . . : 192.168.1.1" / "DNS 服务器 . . : 8.8.8.8"）
    //    → 取标签分隔符 ": "（冒号后跟空格）之后的部分解析 IP；
    //    （IPv6 地址内的 ':' 后跟十六进制字符而非空格 → 不会误切）
    // 2) 无标签的裸 IP 行（DNS 列表续行）→ 直接解析；
    // 其余含标签的 IP 行（IPv4 Address/Default Gateway 等）不含 "DNS" 且带标签 → 天然排除。
    let output = match std::process::Command::new("ipconfig").arg("/all").output() {
        Ok(o) => o.stdout,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output);

    let mut addresses: Vec<IpAddr> = Vec::new();
    for line in text.lines() {
        let mut candidates: Vec<String> = Vec::new();
        if line.contains("DNS") {
            // 标签行：取最后一个 ": "（冒号后跟空格）之后的 IP 列表（逗号分隔容错）
            if let Some(sep) = line.rfind(": ") {
                let rest = line[sep + 2..].replace(',', " ");
                candidates.extend(rest.split_whitespace().map(|s| s.to_string()));
            }
        } else {
            // 续行：整行去掉逗号后可能直接是 IP（无标签）
            let rest = line.replace(',', " ");
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            if tokens.len() == 1 {
                candidates.push(tokens[0].to_string());
            }
        }

        for token in &candidates {
            if let Ok(ip) = token.parse::<IpAddr>() {
                if !addresses.contains(&ip) {
                    addresses.push(ip);
                }
            }
        }
    }

    addresses
}

/// 系统 DNS 地址枚举（Linux/Android/macOS：/etc/resolv.conf）
#[cfg(not(target_os = "windows"))]
fn system_dns_addresses() -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = Vec::new();

    if let Ok(content) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("nameserver") {
                if let Some(token) = rest.split_whitespace().next() {
                    if let Ok(ip) = token.parse::<IpAddr>() {
                        if !addresses.contains(&ip) {
                            addresses.push(ip);
                        }
                    }
                }
            }
        }
    }

    addresses
}

/// 构建 DNS 查询报文（源：BuildDnsQuery，二进制布局逐字）。
///
/// 12 字节头 + QNAME + 4 字节（QTYPE/QCLASS）：
/// - [0..2]：查询 ID（大端，原子计数递增；源 `Interlocked.Increment & 0xFFFF`）
/// - [2..6]：FLAGS 全零 + QDCOUNT=1（**无 RD 递归位**，逐字保留；byte[5] = 1）
/// - [6..8] ANCOUNT=0、[8..10] NSCOUNT=0、[10..12] ARCOUNT=0（全零）
/// - QTYPE = 33（SRV，大端）、QCLASS = 1（IN）
fn build_dns_query(query_name: &[u8]) -> Vec<u8> {
    let id = DNS_QUERY_ID.fetch_add(1, Ordering::Relaxed);

    let mut query = vec![0u8; 12 + query_name.len() + 4];
    query[0] = (id >> 8) as u8;
    query[1] = id as u8;
    query[5] = 1;
    let qname_end = 12 + query_name.len();
    query[12..qname_end].copy_from_slice(query_name);
    query[qname_end] = 0;
    query[qname_end + 1] = 33;
    query[qname_end + 2] = 0;
    query[qname_end + 3] = 1;
    query
}

/// 编码 DNS 名称（源：EncodeDnsName，逐字）。
///
/// 以 '.' 分割标签；每个标签：1 字节长度（`(byte)bytes.Length` 截断语义同 `as u8`）
/// + ASCII 字节（源 Encoding.ASCII：非 ASCII 字符替换为 '?' 0x3F，同本实现映射）；
/// 末尾 0 终结。
fn encode_dns_name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.') {
        let bytes: Vec<u8> = label
            .chars()
            .map(|c| if (c as u32) <= 0x7F { c as u8 } else { b'?' })
            .collect();
        out.push(bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
    out.push(0);
    out
}

/// 解析 DNS SRV 响应（源：ParseDnsSrvResponse，二进制布局逐字）。
///
/// - 报文 < 12 字节 → null
/// - ANCOUNT = header[6..7]（大端，不校验 QR 标志，逐字）
/// - 跳过 Question 段：QNAME（标签序列或 0xC0 压缩指针 → +2）后 +4（QTYPE/QCLASS）
/// - 遍历 ANCOUNT 个 Answer：跳过 NAME（0xC0 指针 → +2；否则标签序列 + 0 终结 → +1），
///   固定 10 字节：TYPE/CLASS/TTL/RDLENGTH；`TYPE == 33 (SRV)` 且 RDLENGTH >= 6 →
///   Port = rdata[+4..+6]（大端）、Target = DecodeDnsName(rdata + 6) 并返回；
///   否则 offset += RDLENGTH 继续
/// - 越界路径源以异常 → 外层 catch → null 收束；本实现等价返回 None（见各边界注释）
fn parse_dns_srv_response(response: &[u8]) -> Option<(String, u16)> {
    if response.len() < 12 {
        return None;
    }

    // 源：anCount = (header[6] << 8) | header[7]
    let an_count = ((response[6] as u16) << 8) | response[7] as u16;

    let mut offset = 12usize;

    // 跳过 Question 的 QNAME（源：压缩指针 → +2 并 break；标签 → +len+1）
    while offset < response.len() && response[offset] != 0 {
        if (response[offset] & 0xC0) == 0xC0 {
            offset += 2;
            break;
        }
        offset += response[offset] as usize + 1;
    }

    // 源：response[offset] == 0 → offset++（源越界会抛异常 → null；
    // 本实现边界保护后落入后续越界退出 → None，结果等价）
    if offset < response.len() && response[offset] == 0 {
        offset += 1;
    }

    // 跳过 QTYPE/QCLASS（源：offset += 4）
    offset += 4;

    for _ in 0..an_count {
        if offset >= response.len() {
            break;
        }

        // 跳过 Answer 的 NAME
        if (response[offset] & 0xC0) == 0xC0 {
            offset += 2;
        } else {
            while offset < response.len() && response[offset] != 0 {
                offset += response[offset] as usize + 1;
            }
            offset += 1;
        }

        // 源：if (offset + 10 > response.Length) break
        if offset + 10 > response.len() {
            break;
        }

        let ans_type = ((response[offset] as u16) << 8) | response[offset + 1] as u16;
        let rd_length = ((response[offset + 8] as u16) << 8) | response[offset + 9] as u16;
        offset += 10;

        // 源：ansType == 33 && rdLength >= 6 → 立即返回
        if ans_type == DNS_TYPE_SRV && rd_length >= 6 {
            let port = ((response[offset + 4] as u16) << 8) | response[offset + 5] as u16;
            let target = decode_dns_name(response, offset + 6);
            return Some((target, port));
        }

        // 源：offset += rdLength（无边界检查，越界由下一轮 offset >= len 收束，同源）
        offset += rd_length as usize;
    }

    None
}

/// 解码 DNS 名称（源：DecodeDnsName，压缩指针逐字）。
///
/// - 0xC0 前缀 → 压缩指针：`((b & 0x3F) << 8) | 下一字节` 作为新 offset 继续解析
///   （仅第一次跳转记录 originalOffset；循环跳转环会死循环——同源行为，不加固）；
/// - 普通标签：1 字节长度 + 字节（源 Encoding.ASCII.GetString：>0x7F → '?'，同本实现）；
/// - 越界 → 停止收集（同源 break）；
/// - 返回 '.' 连接（源 string.Join(".", labels)）
fn decode_dns_name(message: &[u8], offset: usize) -> String {
    let mut labels: Vec<String> = Vec::new();
    let mut jumped = false;
    let mut original_offset = offset;
    let mut offset = offset;

    while offset < message.len() && message[offset] != 0 {
        if (message[offset] & 0xC0) == 0xC0 {
            if !jumped {
                original_offset = offset + 2;
                jumped = true;
            }

            let pointer = (((message[offset] & 0x3F) as u16) << 8) | message[offset + 1] as u16;
            offset = pointer as usize;
            continue;
        }

        let length = message[offset] as usize;
        offset += 1;
        if offset + length > message.len() {
            break;
        }

        // 源：Encoding.ASCII.GetString（>0x7F → '?' 0x3F）
        let label: String = message[offset..offset + length]
            .iter()
            .map(|&b| if b <= 0x7F { b as char } else { '?' })
            .collect();
        labels.push(label);
        offset += length;
    }

    if jumped {
        let _ = original_offset;
    }

    labels.join(".")
}




