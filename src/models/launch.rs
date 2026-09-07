//! 启动模型（B1）：对应源文件
//! - Public/Models/LaunchResult.cs：LaunchResult 类
//! - Builder/CoreOptions.cs 内 JavaOptions / LaunchOptions 记录类

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::models::auth::AuthOptions;

/// 启动阶段回调：core 在解压 natives / 组装启动参数前以阶段名调用
/// （"natives" / "params"），供宿主（启动器后端）上报细分启动进度。
/// 不参与序列化，也不参与 `PartialEq` 比较（回调不影响语义）。
pub type LaunchStageCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// 启动结果（源：Public/Models/LaunchResult.cs 的 LaunchResult 类）。
///
/// 源类中的 `Exception?` 与 `Action<string>?` / `Action<int>?` 回调字段为 .NET 专属
/// 类型（异常对象 / 委托），无法映射为 serde 数据类型，已省略（⚠️ UNMAPPED）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub success: bool,
    pub process_id: i32,
    pub message: Option<String>,
}

/// Java 启动选项（源：Builder/CoreOptions.cs 内 JavaOptions 记录类）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JavaOptions {
    pub java_path: String,
    pub max_memory_mb: i32,
    pub extra_jvm_args: Option<Vec<String>>,
}

impl Default for JavaOptions {
    fn default() -> Self {
        Self {
            java_path: "java".to_string(),
            max_memory_mb: 512,
            extra_jvm_args: None,
        }
    }
}

/// 启动选项（源：Builder/CoreOptions.cs 内 LaunchOptions 记录类）。
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOptions {
    pub version: String,
    pub version_isolation: bool,
    pub join_server: Option<String>,
    pub join_world: Option<String>,
    pub java_options: Option<JavaOptions>,
    pub auth_options: Option<AuthOptions>,
    pub game_root: Option<String>,
    /// 启动细分阶段回调（非源字段，不参与序列化）。
    #[serde(skip)]
    pub on_stage: Option<LaunchStageCallback>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            version: String::new(),
            version_isolation: false,
            join_server: None,
            join_world: None,
            java_options: None,
            auth_options: None,
            game_root: None,
            on_stage: None,
        }
    }
}

impl std::fmt::Debug for LaunchOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchOptions")
            .field("version", &self.version)
            .field("version_isolation", &self.version_isolation)
            .field("join_server", &self.join_server)
            .field("join_world", &self.join_world)
            .field("java_options", &self.java_options)
            .field("auth_options", &self.auth_options)
            .field("game_root", &self.game_root)
            .finish_non_exhaustive()
    }
}

impl PartialEq for LaunchOptions {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.version_isolation == other.version_isolation
            && self.join_server == other.join_server
            && self.join_world == other.join_world
            && self.java_options == other.java_options
            && self.auth_options == other.auth_options
            && self.game_root == other.game_root
    }
}
