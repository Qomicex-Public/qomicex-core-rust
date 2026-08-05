//! Microsoft OAuth 设备码流认证（B5，对应源：Services/MicrosoftAuthProvider.cs）
//!
//! 协议端点清单（URL 逐字保留自源）：
//! - POST https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode（获取设备码/用户码/验证地址/间隔）
//! - POST https://login.microsoftonline.com/consumers/oauth2/v2.0/token（轮询令牌 + 刷新令牌，form）
//! - POST https://user.auth.xboxlive.com/user/authenticate（Xbox Live 认证，JSON）
//! - POST https://xsts.auth.xboxlive.com/xsts/authorize（XSTS 授权，JSON）
//! - POST https://api.minecraftservices.com/authentication/login_with_xbox（Minecraft 登录，JSON）
//! - GET  https://api.minecraftservices.com/entitlements/mcstore（令牌校验，Bearer）
//! - GET  https://api.minecraftservices.com/minecraft/profile（角色档案，Bearer）
//!
//! 流程：获取设备码 → 用户在浏览器完成授权 → 轮询令牌（调用方按 interval 轮询）→
//! 链式认证（Xbox Live → XSTS → Minecraft）→ 取角色档案 → 组装 AuthResult。

use async_trait::async_trait;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::auth::AuthProvider;
use crate::error::Error;
use crate::models::auth::{AuthRequest, AuthResult, DeviceCodeResult, PollTokenResult};

/// 微软 OAuth 设备码流认证提供方（源：`internal sealed class MicrosoftAuthProvider : IAuthProvider`）。
/// 完整设备码流程：获取设备码 → 用户浏览器授权 → 轮询令牌 → Xbox/XSTS/Minecraft 链式认证 → 角色档案。
pub(crate) struct MicrosoftAuthProvider {
    /// 微软应用客户端 ID（源：`_clientId`）
    client_id: String,
    /// 共享 HTTP 客户端（源：`_http` HttpClient）
    http: reqwest::Client,
}

impl MicrosoftAuthProvider {
    /// 创建 Microsoft 认证提供方（源：构造函数注入 HttpClient + clientId）
    pub(crate) fn new(http: reqwest::Client, client_id: impl Into<String>) -> Self {
        Self {
            http,
            client_id: client_id.into(),
        }
    }

    /// 完整登录内部实现（源：CompleteLoginAsync 的 try 块）。
    /// 源以异常消息作为 ErrorMessage，Rust 侧以 `Err(String)` 承载，由 complete_login 包装为失败结果。
    async fn complete_login_inner(
        &self,
        access_token: &str,
        refresh_token: &str,
    ) -> Result<AuthResult, String> {
        let (xbox_token, uhs) = self.authenticate_xbox_live(access_token).await?;
        let xsts_token = self.authenticate_xsts(&xbox_token).await?;
        let mc_token = self.authenticate_minecraft(&xsts_token, &uhs).await?;
        let profile = self.get_minecraft_profile(&mc_token).await?;

        Ok(AuthResult {
            success: true,
            username: profile.as_ref().map(|(_, name)| name.clone()),
            uuid: profile.as_ref().map(|(id, _)| id.clone()),
            access_token: Some(mc_token),
            client_token: None,
            refresh_token: Some(refresh_token.to_string()),
            user_type: Some("msa".to_string()),
            expires_at: Some(expires_at_utc_plus_24h()),
            error_message: None,
        })
    }

    /// Xbox Live 认证（源：AuthenticateXboxLiveAsync），返回 (Token, uhs)。
    /// 模拟源 `EnsureSuccessStatusCode` 异常与 `Exception` 中文消息。
    async fn authenticate_xbox_live(&self, access_token: &str) -> Result<(String, String), String> {
        let payload = serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={access_token}")
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        });

        let resp = self
            .http
            .post("https://user.auth.xboxlive.com/user/authenticate")
            .header("Accept", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        ensure_success(resp.status())?;
        let body = resp.text().await.map_err(|e| e.to_string())?;

        let data: Value = parse_object(&body).ok_or_else(|| "无法解析 Xbox Live 响应".to_string())?;
        let token = data
            .get("Token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Xbox Live 响应缺少 Token".to_string())?
            .to_string();
        let uhs = data
            .get("DisplayClaims")
            .and_then(|v| v.get("xui"))
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("uhs"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Xbox Live 响应缺少 uhs".to_string())?
            .to_string();

        Ok((token, uhs))
    }

    /// XSTS 授权（源：AuthenticateXstsAsync），返回 XSTS Token。
    async fn authenticate_xsts(&self, xbox_token: &str) -> Result<String, String> {
        let payload = serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbox_token]
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        });

        let resp = self
            .http
            .post("https://xsts.auth.xboxlive.com/xsts/authorize")
            .json(&payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        ensure_success(resp.status())?;
        let body = resp.text().await.map_err(|e| e.to_string())?;

        let data: Value = parse_object(&body).ok_or_else(|| "无法解析 XSTS 响应".to_string())?;
        data.get("Token")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "XSTS 响应缺少 Token".to_string())
    }

    /// Minecraft 登录（源：AuthenticateMinecraftAsync），返回 Minecraft access_token。
    async fn authenticate_minecraft(
        &self,
        xsts_token: &str,
        uhs: &str,
    ) -> Result<String, String> {
        let payload = serde_json::json!({
            "identityToken": format!("XBL3.0 x={uhs};{xsts_token}")
        });

        let resp = self
            .http
            .post("https://api.minecraftservices.com/authentication/login_with_xbox")
            .json(&payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        ensure_success(resp.status())?;
        let body = resp.text().await.map_err(|e| e.to_string())?;

        let data: Value =
            parse_object(&body).ok_or_else(|| "无法解析 Minecraft 认证响应".to_string())?;
        data.get("access_token")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "Minecraft 认证响应缺少 access_token".to_string())
    }

    /// 获取 Minecraft 角色档案（源：GetMinecraftProfileAsync）。
    /// 非 2xx / 解析失败 / id 或 name 为空 → 返回 None；网络异常按源语义向上传播。
    async fn get_minecraft_profile(
        &self,
        mc_token: &str,
    ) -> Result<Option<(String, String)>, String> {
        let resp = self
            .http
            .get("https://api.minecraftservices.com/minecraft/profile")
            .bearer_auth(mc_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Ok(None);
        }
        let body = resp.text().await.map_err(|e| e.to_string())?;

        let data: Value = match parse_object(&body) {
            Some(v) => v,
            None => return Ok(None),
        };
        let id = data.get("id").and_then(|v| v.as_str());
        let name = data.get("name").and_then(|v| v.as_str());

        match (id, name) {
            (Some(id), Some(name)) if !id.is_empty() && !name.is_empty() => {
                Ok(Some((id.to_string(), name.to_string())))
            }
            _ => Ok(None),
        }
    }
}

#[async_trait]
impl AuthProvider for MicrosoftAuthProvider {
    /// 使用令牌认证（源：AuthenticateAsync）。
    /// 源要求 access_token 非空（来自设备码流程），否则返回失败结果；否则复用令牌执行完整登录。
    async fn authenticate(&self, request: AuthRequest) -> Result<AuthResult, Error> {
        match request.access_token {
            Some(token) if !token.is_empty() => self.complete_login(&token, &token).await,
            _ => Ok(failed_auth_result("需要 access_token（来自设备码流程）")),
        }
    }

    /// 校验访问令牌（源：ValidateAsync）：请求 Xbox 商城权益接口，2xx 视为有效。
    /// 源 catch 全部异常并返回 false，Rust 侧等价：请求失败返回 Ok(false)。
    async fn validate(&self, access_token: &str) -> Result<bool, Error> {
        match self
            .http
            .get("https://api.minecraftservices.com/entitlements/mcstore")
            .bearer_auth(access_token)
            .send()
            .await
        {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    /// 作废访问令牌（源：InvalidateAsync，空实现）
    async fn invalidate(&self, _access_token: &str) -> Result<(), Error> {
        Ok(())
    }

    /// 启动设备码登录流程（源：StartDeviceCodeAsync）。
    /// 非 2xx / 响应不可解析 / 缺少 device_code 或 user_code → Ok(None)（源返回 null）。
    async fn start_device_code(&self) -> Result<Option<DeviceCodeResult>, Error> {
        let resp = self
            .http
            .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("scope", "offline_access XboxLive.signin XboxLive.offline_access"),
            ])
            .send()
            .await
            .map_err(http_err)?;
        // 源先读 body 再判状态码，顺序保持一致
        let status = resp.status();
        let body = resp.text().await.map_err(http_err)?;

        if !status.is_success() {
            return Ok(None);
        }

        let data: Value = match parse_object(&body) {
            Some(v) => v,
            None => return Ok(None),
        };
        let device_code = data.get("device_code").and_then(|v| v.as_str());
        let user_code = data.get("user_code").and_then(|v| v.as_str());
        let verification_uri = data
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // 源默认值：interval ?? 5，expires_in ?? 900
        let interval = data.get("interval").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
        let expires_in = data.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(900) as i32;

        let (Some(device_code), Some(user_code)) = (device_code, user_code) else {
            return Ok(None);
        };

        Ok(Some(DeviceCodeResult {
            device_code: device_code.to_string(),
            user_code: user_code.to_string(),
            verification_uri: verification_uri.to_string(),
            interval,
            expires_in,
        }))
    }

    /// 轮询设备码登录状态（源：PollForTokenAsync）。
    /// 源为单次 POST（轮询循环在调用方按 interval 驱动），本方法不做循环。
    /// 错误码分支：authorization_declined/expired_token → 终止（is_pending=false）；
    /// slow_down 及其余（含 authorization_pending）→ 继续轮询（is_pending=true）。
    async fn poll_for_token(&self, device_code: &str) -> Result<Option<PollTokenResult>, Error> {
        let resp = self
            .http
            .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .await
            .map_err(http_err)?;
        let body = resp.text().await.map_err(http_err)?;

        let data: Value = match parse_object(&body) {
            Some(v) => v,
            None => {
                // 源：解析失败 → (null, null, "解析失败", false, true)
                return Ok(Some(PollTokenResult {
                    access_token: None,
                    refresh_token: None,
                    error: Some("解析失败".to_string()),
                    is_completed: false,
                    is_pending: true,
                }));
            }
        };

        let err = data.get("error").and_then(|v| v.as_str());

        match err {
            None => {
                // 源：无 error → 取 access_token/refresh_token，完成
                Ok(Some(PollTokenResult {
                    access_token: data
                        .get("access_token")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    refresh_token: data
                        .get("refresh_token")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    error: None,
                    is_completed: true,
                    is_pending: false,
                }))
            }
            Some("authorization_declined") | Some("expired_token") => {
                // 源：用户拒绝或设备码过期 → 终止轮询
                Ok(Some(PollTokenResult {
                    access_token: None,
                    refresh_token: None,
                    error: err.map(|s| s.to_string()),
                    is_completed: false,
                    is_pending: false,
                }))
            }
            Some("slow_down") => {
                // 源：需放慢轮询间隔 → 继续（调用方按错误调整间隔）
                Ok(Some(PollTokenResult {
                    access_token: None,
                    refresh_token: None,
                    error: err.map(|s| s.to_string()),
                    is_completed: false,
                    is_pending: true,
                }))
            }
            Some(_) => {
                // 源：其余错误码（含 authorization_pending）→ 继续轮询
                Ok(Some(PollTokenResult {
                    access_token: None,
                    refresh_token: None,
                    error: err.map(|s| s.to_string()),
                    is_completed: false,
                    is_pending: true,
                }))
            }
        }
    }

    /// 设备码登录完成（源：CompleteLoginAsync）：链式认证 + 取档案 → 成功结果。
    /// 源 catch 全部异常为失败结果（ErrorMessage = ex.Message），Rust 侧同：内部错误包装为失败 AuthResult。
    async fn complete_login(
        &self,
        access_token: &str,
        refresh_token: &str,
    ) -> Result<AuthResult, Error> {
        Ok(match self.complete_login_inner(access_token, refresh_token).await {
            Ok(result) => result,
            Err(message) => failed_auth_result(&message),
        })
    }

    /// 用刷新令牌续期访问令牌（源：RefreshLoginAsync）。
    /// 刷新失败返回固定错误消息；成功后用新令牌走完整登录流程（源：CompleteLoginAsync(newToken, newRefresh ?? old)）。
    async fn refresh_login(&self, refresh_token: &str) -> Result<AuthResult, Error> {
        let resp = self
            .http
            .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("scope", "XboxLive.signin offline_access"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(http_err)?;
        let status = resp.status();
        let body = resp.text().await.map_err(http_err)?;

        if !status.is_success() {
            return Ok(failed_auth_result("令牌刷新失败"));
        }

        let data: Value = match parse_object(&body) {
            Some(v) => v,
            None => return Ok(failed_auth_result("无法解析刷新响应")),
        };

        let new_access_token = match data.get("access_token").and_then(|v| v.as_str()) {
            Some(token) => token.to_string(),
            None => return Ok(failed_auth_result("刷新响应缺少 access_token")),
        };
        let new_refresh_token = data
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| refresh_token.to_string());

        self.complete_login(&new_access_token, &new_refresh_token).await
    }
}

/// 失败认证结果（源：`new AuthResult { Success = false, ErrorMessage = ... }`，其余字段 null）
fn failed_auth_result(error_message: &str) -> AuthResult {
    AuthResult {
        success: false,
        username: None,
        access_token: None,
        client_token: None,
        refresh_token: None,
        uuid: None,
        user_type: None,
        expires_at: None,
        error_message: Some(error_message.to_string()),
    }
}

/// 解析 JSON 对象（等价源 `JsonNode.Parse(body)?.AsObject()`：仅对象视为有效）
fn parse_object(body: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(body) {
        Ok(v) if v.is_object() => Some(v),
        _ => None,
    }
}

/// 模拟 .NET `EnsureSuccessStatusCode()` 的 HttpRequestException 消息
fn ensure_success(status: reqwest::StatusCode) -> Result<(), String> {
    if status.is_success() {
        Ok(())
    } else {
        let reason = status.canonical_reason().unwrap_or("");
        Err(format!(
            "Response status code does not indicate success: {} ({}).",
            status.as_u16(),
            reason
        ))
    }
}

/// 网络错误映射：源 HttpRequestException 无对应 Error 变体（B1 仅 5 个异常），
/// 借用 Error::DownloadFailed 承载（message + source），日志 p23 已记录该映射决策。
fn http_err(e: reqwest::Error) -> Error {
    Error::DownloadFailed {
        message: format!("HTTP 请求失败: {e}"),
        source: Some(Box::new(e)),
    }
}

/// 源语义 `DateTimeOffset.UtcNow.AddHours(24)`（B1 定案：DateTimeOffset → String，未引入 chrono）。
/// 输出 ISO 8601 UTC（整秒，无小数位；源 STJ 序列化带最多 7 位小数秒，偏差已在日志 p23 记录）。
fn expires_at_utc_plus_24h() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + 24 * 60 * 60;
    unix_secs_to_iso8601_utc(secs)
}

/// Unix 秒 → ISO 8601 UTC 字符串（Howard Hinnant civil_from_days 算法，无 chrono 依赖）
fn unix_secs_to_iso8601_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        y,
        m,
        d,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// 天数 → 公历日期（Howard Hinnant civil_from_days 算法）
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

