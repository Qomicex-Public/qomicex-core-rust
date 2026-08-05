//! 认证模型（B1）：对应源文件
//! - Models/Auth/YggdrasilModels.cs（Yggdrasil 认证 DTO，AuthJsonContext 注册：CamelCase + WhenWritingNull）
//! - Models/UserAuth.cs（UserAuth 记录）
//! - Public/IAuthProvider.cs 内的 AuthRequest / AuthResult / DeviceCodeResult / PollTokenResult
//! - Builder/CoreOptions.cs 内的 AuthMode 枚举、AuthOptions 配置类

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

/// Yggdrasil 客户端代理（源：YggdrasilAgent）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilAgent {
    pub name: String,
    pub version: i32,
}

/// Yggdrasil 认证请求（源：YggdrasilAuthenticateRequest）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilAuthenticateRequest {
    pub agent: YggdrasilAgent,
    pub username: String,
    pub password: String,
    pub client_token: String,
    pub request_user: bool,
}

/// Yggdrasil 认证响应（源：YggdrasilAuthenticateResponse）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilAuthenticateResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_profiles: Option<Vec<YggdrasilProfile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_profile: Option<YggdrasilProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<YggdrasilUser>,
}

/// Yggdrasil 令牌刷新请求（源：YggdrasilRefreshRequest）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilRefreshRequest {
    pub access_token: String,
    pub client_token: String,
    pub request_user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_profile: Option<YggdrasilProfile>,
}

/// Yggdrasil 令牌刷新响应（源：YggdrasilRefreshResponse）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilRefreshResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_profile: Option<YggdrasilProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<YggdrasilUser>,
}

/// Yggdrasil 令牌校验请求（源：YggdrasilValidateRequest）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilValidateRequest {
    pub access_token: String,
    pub client_token: String,
}

/// Yggdrasil 令牌作废请求（源：YggdrasilInvalidateRequest）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilInvalidateRequest {
    pub access_token: String,
    pub client_token: String,
}

/// Yggdrasil 角色档案（源：YggdrasilProfile）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<YggdrasilProperty>>,
}

/// Yggdrasil 用户（源：YggdrasilUser）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilUser {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<YggdrasilProperty>>,
}

/// Yggdrasil 属性键值对（源：YggdrasilProperty）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilProperty {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Yggdrasil 错误信息（源：YggdrasilError）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YggdrasilError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

/// 表示用户认证信息。（源：UserAuth 记录；未注册任何 JsonContext，仅内存传递）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserAuth {
    pub name: String,
    pub uuid: String,
    pub token: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// 认证请求（源：IAuthProvider.cs 内 AuthRequest 类）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub access_token: Option<String>,
    pub server_url: Option<String>,
    pub is_offline: bool,
}

/// 认证结果（源：IAuthProvider.cs 内 AuthResult 类）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthResult {
    pub success: bool,
    pub username: Option<String>,
    pub access_token: Option<String>,
    pub client_token: Option<String>,
    pub refresh_token: Option<String>,
    pub uuid: Option<String>,
    pub user_type: Option<String>,
    /// 源类型为 `DateTimeOffset?`，按 B1 规则以 String 保留原始文本（未引入 chrono）
    pub expires_at: Option<String>,
    pub error_message: Option<String>,
}

/// 设备码登录结果（源：IAuthProvider.cs 内 DeviceCodeResult 记录）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeResult {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: i32,
    pub expires_in: i32,
}

/// 设备码轮询令牌结果（源：IAuthProvider.cs 内 PollTokenResult 记录）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PollTokenResult {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub error: Option<String>,
    pub is_completed: bool,
    pub is_pending: bool,
}

/// 认证模式（源：CoreOptions.cs 内 AuthMode 枚举；纯配置用、未注册 JsonContext，仍用 serde_repr 数字序列化保持一致）
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(i32)]
pub enum AuthMode {
    Offline = 0,
    Microsoft = 1,
    Yggdrasil = 2,
}

/// 认证配置项（源：CoreOptions.cs 内 AuthOptions 类，含显式默认值）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthOptions {
    pub mode: AuthMode,
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub server_url: Option<String>,
    pub authlib_injector_param: Option<String>,
}

impl Default for AuthOptions {
    fn default() -> Self {
        Self {
            mode: AuthMode::Offline,
            uuid: None,
            name: Some("Player".to_string()),
            token: None,
            access_token: Some("0".to_string()),
            refresh_token: None,
            server_url: None,
            authlib_injector_param: None,
        }
    }
}
