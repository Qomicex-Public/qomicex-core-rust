//! LaunchExecutor trait：JVM 参数组装 + 进程启动（B3）
//!
//! 对应源文件：Public/ILaunchExecutor.cs（Qomicex.Core.AOT.Interfaces）
//!
//! 方法映射表：
//! - `LaunchAsync(LaunchOptions) -> Task<LaunchResult>` → `launch(&self, options) -> Result<LaunchResult, Error>`
//! - `KillAsync(int processId) -> Task<bool>` → `kill(&self, process_id) -> Result<bool, Error>`
//!
//! 备注：MAPPING_TABLE.yaml 记录为 `ExecuteAsync -> execute`，与源文件实际
//! 签名（LaunchAsync）不符，以源文件为准采用 `launch`（详见 B3 翻译日志）。

use crate::error::Error;
use crate::models::launch::{LaunchOptions, LaunchResult};
use async_trait::async_trait;

/// 启动执行器（源：ILaunchExecutor 接口）。
///
/// 负责以指定选项组装 JVM 启动参数并拉起游戏进程，以及按进程 ID 结束进程。
#[async_trait]
pub trait LaunchExecutor: Send + Sync {
    /// 以给定选项启动游戏进程，返回启动结果（源：`LaunchAsync`）。
    async fn launch(&self, options: LaunchOptions) -> Result<LaunchResult, Error>;

    /// 按进程 ID 结束进程，返回是否成功结束（源：`KillAsync`）。
    async fn kill(&self, process_id: i32) -> Result<bool, Error>;
}
