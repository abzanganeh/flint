//! Keychain-backed session restore and refresh (Task 1.9).

use chrono::Utc;
use tracing::info;

use crate::interfaces::auth::{AuthInterface, AuthToken};
use crate::keychain;

/// True when the access token is past its expiry time.
pub fn is_token_expired(token: &AuthToken) -> bool {
    token.expires_at <= Utc::now()
}

/// Load a valid session from the OS keychain, refreshing if the access token expired.
pub async fn restore_auth_from_keychain(auth: &dyn AuthInterface) -> Option<AuthToken> {
    let token = keychain::get_auth_token().ok()?;

    if !is_token_expired(&token) {
        info!(event = "auth_session_restored");
        return Some(token);
    }

    info!(event = "auth_token_expired_refreshing");
    match auth.refresh(&token.refresh_token).await {
        Ok(new_token) => {
            if keychain::store_auth_token(&new_token).is_ok() {
                info!(event = "auth_session_refreshed");
                Some(new_token)
            } else {
                let _ = keychain::clear_auth_token();
                info!(event = "auth_refresh_keychain_store_failed");
                None
            }
        }
        Err(_) => {
            let _ = keychain::clear_auth_token();
            info!(event = "auth_refresh_failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use secrecy::SecretString;

    fn sample_token(expires_at: chrono::DateTime<Utc>) -> AuthToken {
        AuthToken {
            access_token: SecretString::new("access".into()),
            refresh_token: SecretString::new("refresh".into()),
            expires_at,
        }
    }

    #[test]
    fn detects_expired_token() {
        let token = sample_token(Utc::now() - Duration::hours(1));
        assert!(is_token_expired(&token));
    }

    #[test]
    fn detects_valid_token() {
        let token = sample_token(Utc::now() + Duration::hours(1));
        assert!(!is_token_expired(&token));
    }
}
