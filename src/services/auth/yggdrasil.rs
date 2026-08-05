//! Yggdrasil 外置登录（B5，对应源文件 Services/YggdrasilAuthProvider.cs）
//!
//! 端点（base = server_url 去尾部 `/` 后补一个 `/`，即源 `_baseUrl`）：
//! - POST {base}authserver/authenticate  → YggdrasilAuthenticateRequest / Response
//! - POST {base}authserver/validate      → YggdrasilValidateRequest（仅 204 NoContent 视为有效）
//! - POST {base}authserver/invalidate    → YggdrasilInvalidateRequest（不检查响应状态）
//!
//! 认证失败（HTTP 非 2xx）按源返回失败 AuthResult（而非 Err）；
//! 错误消息优先取 YggdrasilError.errorMessage，缺省回退 "认证失败: {StatusCode}"。
//! 传输层异常（reqwest::Error）无法直接映射到 Error 枚举（B1 无 HTTP 变体），
//! 暂用 Error::DownloadFailed 承载（见 transport_error，⚠️ UNMAPPED，B6 补充）。

use async_trait::async_trait;
use uuid::Uuid;

use crate::api::auth::AuthProvider;
use crate::error::Error;
use crate::models::auth::{
    AuthRequest, AuthResult, YggdrasilAgent, YggdrasilAuthenticateRequest,
    YggdrasilAuthenticateResponse, YggdrasilError, YggdrasilInvalidateRequest,
    YggdrasilValidateRequest,
};

/// Yggdrasil 外置登录认证提供方（源：YggdrasilAuthProvider）。
/// 通过 Yggdrasil 协议（authlib-injector 兼容）向 `server_url` 发起认证、
/// 令牌校验与令牌作废请求。
pub(crate) struct YggdrasilAuthProvider {
    /// 复用的 HTTP 客户端（源：HttpClient）
    http: reqwest::Client,
    /// 请求基址：`serverUrl.TrimEnd('/') + "/"`（源：_baseUrl）
    server_url: String,
}

impl YggdrasilAuthProvider {
    /// 创建 Yggdrasil 提供方（源：构造函数）。
    /// `server_url` 尾部的 `/` 会被剥除后统一补一个 `/` 作为请求基址。
    pub(crate) fn new(http: reqwest::Client, server_url: String) -> Self {
        Self {
            http,
            server_url: server_url.trim_end_matches('/').to_string() + "/",
        }
    }
}

#[async_trait]
impl AuthProvider for YggdrasilAuthProvider {
    /// 使用用户名密码认证（源：AuthenticateAsync）。
    async fn authenticate(&self, request: AuthRequest) -> Result<AuthResult, Error> {
        // C#: new YggdrasilAuthenticateRequest(
        //      Agent: new("Minecraft", 1), Username: request.Username ?? "",
        //      Password: request.Password ?? "",
        //      ClientToken: Guid.NewGuid().ToString("N"), RequestUser: true)
        // C#: Guid.NewGuid().ToString("N") —— 32 位小写十六进制、无连字符
        let req = YggdrasilAuthenticateRequest {
            agent: YggdrasilAgent {
                name: "Minecraft".to_string(),
                version: 1,
            },
            username: request.username.unwrap_or_default(),
            password: request.password.unwrap_or_default(),
            client_token: Uuid::new_v4().simple().to_string(),
            request_user: true,
        };

        let response = self
            .http
            .post(format!("{}authserver/authenticate", self.server_url))
            .json(&req)
            .send()
            .await
            .map_err(transport_error)?;

        if !response.status().is_success() {
            // C#: err?.ErrorMessage ?? $"认证失败: {response.StatusCode}"
            let status = response.status().as_u16();
            let body = response.text().await.map_err(transport_error)?;
            let err: Option<YggdrasilError> = serde_json::from_str(&body).ok();
            return Ok(AuthResult {
                success: false,
                username: None,
                access_token: None,
                client_token: None,
                refresh_token: None,
                uuid: None,
                user_type: None,
                expires_at: None,
                error_message: Some(
                    err.and_then(|e| e.error_message)
                        .unwrap_or_else(|| format!("认证失败: {status}")),
                ),
            });
        }

        let body = response.text().await.map_err(transport_error)?;
        let auth_resp: YggdrasilAuthenticateResponse =
            serde_json::from_str(&body).map_err(parse_error)?;

        // C#: authResp?.AccessToken == null
        if auth_resp.access_token.is_none() {
            return Ok(AuthResult {
                success: false,
                username: None,
                access_token: None,
                client_token: None,
                refresh_token: None,
                uuid: None,
                user_type: None,
                expires_at: None,
                error_message: Some("无法解析认证响应".to_string()),
            });
        }

        // C#: authResp.SelectedProfile ?? authResp.AvailableProfiles?.FirstOrDefault()
        let profile = auth_resp
            .selected_profile
            .clone()
            .or_else(|| auth_resp.available_profiles.as_ref()?.first().cloned());
        // C#: authResp.User?.Properties?.FirstOrDefault(p => p.Name == "userType")?.Value
        let user_type = auth_resp
            .user
            .as_ref()
            .and_then(|u| u.properties.as_ref())
            .and_then(|props| props.iter().find(|p| p.name.as_deref() == Some("userType")))
            .and_then(|p| p.value.clone());

        Ok(AuthResult {
            success: true,
            username: profile.as_ref().and_then(|p| p.name.clone()),
            access_token: auth_resp.access_token,
            client_token: auth_resp.client_token,
            refresh_token: None,
            uuid: profile.as_ref().and_then(|p| p.id.clone()),
            user_type,
            // C#: ExpiresAt = DateTimeOffset.UtcNow.AddHours(6)
            expires_at: Some(utc_now_plus_hours(6)),
            error_message: None,
        })
    }

    /// 校验访问令牌是否有效（源：ValidateAsync）：仅 204 NoContent 视为有效。
    async fn validate(&self, access_token: &str) -> Result<bool, Error> {
        let req = YggdrasilValidateRequest {
            access_token: access_token.to_string(),
            client_token: String::new(),
        };

        let response = self
            .http
            .post(format!("{}authserver/validate", self.server_url))
            .json(&req)
            .send()
            .await
            .map_err(transport_error)?;

        Ok(response.status() == reqwest::StatusCode::NO_CONTENT)
    }

    /// 作废访问令牌（源：InvalidateAsync）：不检查响应状态。
    async fn invalidate(&self, access_token: &str) -> Result<(), Error> {
        let req = YggdrasilInvalidateRequest {
            access_token: access_token.to_string(),
            client_token: String::new(),
        };

        self.http
            .post(format!("{}authserver/invalidate", self.server_url))
            .json(&req)
            .send()
            .await
            .map_err(transport_error)?;

        Ok(())
    }
}

/// ⚠️ UNMAPPED：将 reqwest 传输错误映射为 Error。
/// 源在传输异常时抛出 HttpRequestException；
/// 目标 Error 枚举（B1，对应 Exceptions/ 下 5 个异常）暂无 HTTP 变体，
/// 暂以 Error::DownloadFailed 承载（建议 B6 补充 Http 变体后替换）。
fn transport_error(source: reqwest::Error) -> Error {
    Error::DownloadFailed {
        message: format!("yggdrasil request failed: {source}"),
        source: Some(Box::new(source)),
    }
}

/// ⚠️ UNMAPPED：将响应体 JSON 解析错误映射为 Error。
/// 源在解析异常时抛出 JsonException；暂以 Error::DownloadFailed 承载（同 transport_error）。
fn parse_error(source: serde_json::Error) -> Error {
    Error::DownloadFailed {
        message: format!("yggdrasil response parse failed: {source}"),
        source: Some(Box::new(source)),
    }
}

/// 计算 UTC 当前时间 + `hours` 小时，输出 "YYYY-MM-DDTHH:MM:SS+00:00"。
/// 对应源 `DateTimeOffset.UtcNow.AddHours(6)`。B1 定案 DateTimeOffset -> String
/// 原始文本保真（chrono 决策推迟到 B6）；此处无 chrono，用 std::time::SystemTime +
/// civil_from_days（Hinnant 算法）手工换算，格式对齐 System.Text.Json 的
/// DateTimeOffset 默认序列化（yyyy-MM-ddTHH:mm:ss+00:00）。
fn utc_now_plus_hours(hours: u64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + hours * 3600;
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!(
        "{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}+00:00"
    )
}

/// 天数（1970-01-01 起）→ (年, 月, 日)。Howard Hinnant civil_from_days 算法。
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    ((y + if m <= 2 { 1 } else { 0 }) as i32, m, d)
}
