//! AuthProvider trait（B3）：对应源文件 Public/IAuthProvider.cs
//!
//! 方法映射表：
//! - AuthenticateAsync(AuthRequest request) -> authenticate(&self, request: AuthRequest) -> Result<AuthResult, Error>
//! - ValidateAsync(string accessToken)       -> validate(&self, access_token: &str) -> Result<bool, Error>
//! - InvalidateAsync(string accessToken)     -> invalidate(&self, access_token: &str) -> Result<(), Error>
//! - StartDeviceCodeAsync()（默认实现，返回 null）        -> start_device_code(&self) -> Result<Option<DeviceCodeResult>, Error>
//! - PollForTokenAsync(string deviceCode)（默认实现，返回 null） -> poll_for_token(&self, device_code: &str) -> Result<Option<PollTokenResult>, Error>
//! - CompleteLoginAsync(...)（默认实现，固定错误消息）    -> complete_login(&self, ...) -> Result<AuthResult, Error>
//! - RefreshLoginAsync(string refreshToken)（默认实现，固定错误消息） -> refresh_login(&self, ...) -> Result<AuthResult, Error>
//!
//! 模型 AuthRequest / AuthResult / DeviceCodeResult / PollTokenResult 已由 B1 落位
//! src/models/auth.rs，本文件只移植接口本身。

use crate::error::Error;
use crate::models::auth::{AuthRequest, AuthResult, DeviceCodeResult, PollTokenResult};

/// 认证提供方（源：IAuthProvider）。
/// 实现账号密码/令牌认证、校验与作废；设备码登录流（微软）为可选能力，默认不支持。
pub trait AuthProvider: Send + Sync {
    /// 使用用户名密码或令牌进行认证（源：AuthenticateAsync）
    async fn authenticate(&self, request: AuthRequest) -> Result<AuthResult, Error>;

    /// 校验访问令牌是否有效（源：ValidateAsync）
    async fn validate(&self, access_token: &str) -> Result<bool, Error>;

    /// 作废访问令牌（源：InvalidateAsync）
    async fn invalidate(&self, access_token: &str) -> Result<(), Error>;

    /// 启动设备码登录流程，返回设备码/用户码/验证地址（源：StartDeviceCodeAsync）。
    /// 默认实现：不支持设备码登录，返回 None。
    async fn start_device_code(&self) -> Result<Option<DeviceCodeResult>, Error> {
        Ok(None)
    }

    /// 轮询设备码登录状态，取回令牌（源：PollForTokenAsync）。
    /// 默认实现：不支持设备码登录，返回 None。
    async fn poll_for_token(&self, _device_code: &str) -> Result<Option<PollTokenResult>, Error> {
        Ok(None)
    }

    /// 设备码登录完成：用 accessToken + refreshToken 组装完整认证结果（源：CompleteLoginAsync）。
    /// 默认实现：此认证方式不支持设备码登录。
    async fn complete_login(
        &self,
        _access_token: &str,
        _refresh_token: &str,
    ) -> Result<AuthResult, Error> {
        Ok(AuthResult {
            success: false,
            username: None,
            access_token: None,
            client_token: None,
            refresh_token: None,
            uuid: None,
            user_type: None,
            expires_at: None,
            error_message: Some("此认证方式不支持设备码登录".to_string()),
        })
    }

    /// 用刷新令牌续期访问令牌（源：RefreshLoginAsync）。
    /// 默认实现：此认证方式不支持令牌刷新。
    async fn refresh_login(&self, _refresh_token: &str) -> Result<AuthResult, Error> {
        Ok(AuthResult {
            success: false,
            username: None,
            access_token: None,
            client_token: None,
            refresh_token: None,
            uuid: None,
            user_type: None,
            expires_at: None,
            error_message: Some("此认证方式不支持令牌刷新".to_string()),
        })
    }
}
