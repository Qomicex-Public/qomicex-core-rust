//! 离线认证实现（B5）：对应源文件 Services/DefaultAuthProvider.cs
//!
//! DefaultAuthProvider 为默认/离线认证提供方：任何请求直接成功返回，
//! UUID 按 OfflineUuidHelper 规则生成（MD5 "OfflinePlayer:{name}" v3），
//! 令牌为随机 Guid（此处映射为 uuid v4）。

use async_trait::async_trait;
use uuid::Uuid;

use crate::api::auth::AuthProvider;
use crate::error::Error;
use crate::models::auth::{AuthRequest, AuthResult};
use crate::util::platform::generate_uuid;

/// 默认（离线）认证提供方（源：DefaultAuthProvider）。
/// 离线认证始终成功：用户名缺省为 "Player"，UUID 由名字离线推导，
/// AccessToken/ClientToken 为随机新值，UserType 固定 "legacy"。
pub(crate) struct OfflineAuthProvider;

#[async_trait]
impl AuthProvider for OfflineAuthProvider {
    /// 使用用户名密码或令牌进行认证（源：AuthenticateAsync）。
    /// 离线模式忽略凭据，直接返回成功结果。
    async fn authenticate(&self, request: AuthRequest) -> Result<AuthResult, Error> {
        // C#: OfflineUuidHelper.GenerateUuid(request.Username ?? "Player")
        let username = request.username.unwrap_or_else(|| "Player".to_string());
        let uuid = generate_uuid(&username);
        Ok(AuthResult {
            success: true,
            username: Some(username),
            // C#: Guid.NewGuid().ToString()
            access_token: Some(Uuid::new_v4().to_string()),
            client_token: Some(Uuid::new_v4().to_string()),
            refresh_token: None,
            uuid: Some(uuid),
            user_type: Some("legacy".to_string()),
            expires_at: None,
            error_message: None,
        })
    }

    /// 校验访问令牌是否有效（源：ValidateAsync）：离线模式恒有效。
    async fn validate(&self, _access_token: &str) -> Result<bool, Error> {
        Ok(true)
    }

    /// 作废访问令牌（源：InvalidateAsync）：离线模式无状态，空操作。
    async fn invalidate(&self, _access_token: &str) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::AuthProvider;
    use crate::models::auth::AuthRequest;

    #[tokio::test]
    async fn offline_auth_produces_legacy_result() {
        let provider = OfflineAuthProvider;
        let result = provider
            .authenticate(AuthRequest {
                username: Some("Steve".to_string()),
                password: None,
                access_token: None,
                server_url: None,
                is_offline: true,
            })
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.username.as_deref(), Some("Steve"));
        assert_eq!(result.user_type.as_deref(), Some("legacy"));
        let result2 = provider
            .authenticate(AuthRequest {
                username: Some("Steve".to_string()),
                password: None,
                access_token: None,
                server_url: None,
                is_offline: true,
            })
            .await
            .unwrap();
        assert_eq!(result.uuid, result2.uuid, "同用户名离线 UUID 必须稳定");
        let uuid = crate::util::platform::generate_uuid("Player");
        // 源 OfflineUuidHelper 输出 32 位无连字符 hex
        assert_eq!(uuid.len(), 32);
        assert!(!uuid.contains('-'));
        // v3 版本位（源 bit6 = hex & 0x0F | 0x30 → nibble 3）
        assert_eq!(&uuid[12..13], "3");
    }

    #[tokio::test]
    async fn offline_auth_defaults_username() {
        let provider = OfflineAuthProvider;
        let result = provider
            .authenticate(AuthRequest {
                username: None,
                password: None,
                access_token: None,
                server_url: None,
                is_offline: true,
            })
            .await
            .unwrap();
        assert_eq!(result.username.as_deref(), Some("Player"));
    }

    #[tokio::test]
    async fn offline_validate_invalidate() {
        let provider = OfflineAuthProvider;
        assert!(provider.validate("any-token").await.unwrap());
        assert!(provider.invalidate("any-token").await.is_ok());
    }
}
