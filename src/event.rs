//! CoreEvent 事件枚举 + 日志级别（B3）
//!
//! 对应源项目 Public/Events/：IDownloadProgressEvent、IInstallProgressEvent
//! （事件 EventHandler<DownloadProgress>）以及 README 描述的 IProgress<T> 进度报告。
//!
//! 设计决策（ADR-001 D3）：进度/事件统一走 `mpsc::channel<CoreEvent>`，
//! 避免回调闭包生命周期问题（跨线程安全）。

use crate::models::download::DownloadProgress;

/// 日志级别（对应启动核心内部日志输出）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// 调试
    Debug,
    /// 信息
    Info,
    /// 警告
    Warn,
    /// 错误
    Error,
}

/// 核心事件（源：Event<DownloadProgress> + IProgress<T> 语义合并）
#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    /// 下载进度（对应 IDownloadProgressEvent.DownloadProgressChanged）
    DownloadProgress(DownloadProgress),
    /// 安装进度（对应 IInstallProgressEvent.InstallProgressChanged）
    InstallProgress(DownloadProgress),
    /// 日志输出
    Log {
        /// 级别
        level: LogLevel,
        /// 消息
        message: String,
    },
    /// 流程阶段切换（如版本安装的 stage 变化）
    State {
        /// 阶段名
        phase: String,
    },
}

/// 进度上报 trait：服务实现向事件通道发送进度（对应 C# IProgress<T>）
pub trait ProgressReporter: Send + Sync {
    /// 发送下载进度事件
    fn report_download(&self, progress: DownloadProgress);
    /// 发送安装进度事件
    fn report_install(&self, progress: DownloadProgress);
    /// 发送阶段切换事件
    fn report_state(&self, phase: &str);
}

/// 基于 `tokio::sync::mpsc::Sender` 的进度上报实现
pub struct ChannelProgressReporter {
    sender: tokio::sync::mpsc::Sender<CoreEvent>,
}

impl ChannelProgressReporter {
    /// 创建上报器
    pub fn new(sender: tokio::sync::mpsc::Sender<CoreEvent>) -> Self {
        Self { sender }
    }
}

impl ProgressReporter for ChannelProgressReporter {
    fn report_download(&self, progress: DownloadProgress) {
        let _ = self.sender.try_send(CoreEvent::DownloadProgress(progress));
    }

    fn report_install(&self, progress: DownloadProgress) {
        let _ = self.sender.try_send(CoreEvent::InstallProgress(progress));
    }

    fn report_state(&self, phase: &str) {
        let _ = self.sender.try_send(CoreEvent::State {
            phase: phase.to_string(),
        });
    }
}
