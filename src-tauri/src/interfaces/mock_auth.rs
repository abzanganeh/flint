//! In-memory `AuthInterface` for unit tests (Task 1.10).

use std::sync::Mutex;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use super::auth::{AuthInterface, AuthToken, Plan, User};

const FIXTURE_USER_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
const FIXTURE_PASSWORD: &str = "correct-password";
const FIXTURE_REFRESH: &str = "mock-refresh-token";

struct MockAuthState {
    user: User,
    logged_in: bool,
    refresh_issued: usize,
}

/// Deterministic auth provider for tests. Not for production use.
#[allow(clippy::new_without_default)]
pub struct MockAuth {
    inner: Mutex<MockAuthState>,
}

#[allow(clippy::new_without_default)]
impl MockAuth {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MockAuthState {
                user: User {
                    id: Uuid::parse_str(FIXTURE_USER_ID).expect("valid fixture uuid"),
                    email: "fixture@flint.dev".to_string(),
                    plan: Plan::Free,
                },
                logged_in: false,
                refresh_issued: 0,
            }),
        }
    }

    fn fixture_token() -> AuthToken {
        AuthToken {
            access_token: SecretString::new("mock-access-token".into()),
            refresh_token: SecretString::new(FIXTURE_REFRESH.into()),
            expires_at: Utc::now() + Duration::hours(1),
        }
    }

    fn refreshed_token() -> AuthToken {
        AuthToken {
            access_token: SecretString::new("mock-access-token-refreshed".into()),
            refresh_token: SecretString::new(FIXTURE_REFRESH.into()),
            expires_at: Utc::now() + Duration::hours(2),
        }
    }
}

#[async_trait]
impl AuthInterface for MockAuth {
    async fn signup(&self, email: &str, password: &str) -> Result<User> {
        if password.is_empty() {
            return Err(anyhow!("Invalid credentials"));
        }
        let mut state = self.inner.lock().map_err(|_| anyhow!("lock poisoned"))?;
        state.user.email = email.to_string();
        Ok(User {
            id: state.user.id,
            email: state.user.email.clone(),
            plan: state.user.plan,
        })
    }

    async fn login(&self, email: &str, password: &str) -> Result<AuthToken> {
        if password != FIXTURE_PASSWORD {
            return Err(anyhow!("Invalid credentials"));
        }
        let mut state = self.inner.lock().map_err(|_| anyhow!("lock poisoned"))?;
        state.user.email = email.to_string();
        state.logged_in = true;
        Ok(Self::fixture_token())
    }

    async fn logout(&self, token: &AuthToken) -> Result<()> {
        if token.access_token.expose_secret() != "mock-access-token"
            && token.access_token.expose_secret() != "mock-access-token-refreshed"
        {
            return Err(anyhow!("Invalid credentials"));
        }
        let mut state = self.inner.lock().map_err(|_| anyhow!("lock poisoned"))?;
        if !state.logged_in {
            return Err(anyhow!("Not logged in"));
        }
        state.logged_in = false;
        Ok(())
    }

    async fn refresh(&self, refresh_token: &str) -> Result<AuthToken> {
        if refresh_token != FIXTURE_REFRESH {
            return Err(anyhow!("Invalid credentials"));
        }
        let mut state = self.inner.lock().map_err(|_| anyhow!("lock poisoned"))?;
        state.refresh_issued += 1;
        state.logged_in = true;
        Ok(Self::refreshed_token())
    }

    async fn get_current_user(&self, token: &AuthToken) -> Result<User> {
        let state = self.inner.lock().map_err(|_| anyhow!("lock poisoned"))?;
        if !state.logged_in {
            return Err(anyhow!("Not logged in"));
        }
        let _ = token;
        Ok(User {
            id: state.user.id,
            email: state.user.email.clone(),
            plan: state.user.plan,
        })
    }

    async fn delete_account(&self, _token: &AuthToken) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signup_returns_user_with_email() {
        let auth = MockAuth::new();
        let user = auth
            .signup("new@flint.dev", FIXTURE_PASSWORD)
            .await
            .expect("signup succeeds");
        assert_eq!(user.email, "new@flint.dev");
        assert_eq!(user.id, Uuid::parse_str(FIXTURE_USER_ID).unwrap());
        assert_eq!(user.plan, Plan::Free);
    }

    #[tokio::test]
    async fn login_returns_auth_token() {
        let auth = MockAuth::new();
        let token = auth
            .login("user@flint.dev", FIXTURE_PASSWORD)
            .await
            .expect("login succeeds");
        assert_eq!(token.access_token.expose_secret(), "mock-access-token");
        assert_eq!(token.refresh_token.expose_secret(), FIXTURE_REFRESH);
        assert!(token.expires_at > Utc::now());
    }

    #[tokio::test]
    async fn logout_ends_session() {
        let auth = MockAuth::new();
        let token = auth
            .login("user@flint.dev", FIXTURE_PASSWORD)
            .await
            .expect("login");
        auth.logout(&token).await.expect("logout succeeds");
        assert!(auth.get_current_user(&token).await.is_err());
    }

    #[tokio::test]
    async fn refresh_returns_new_access_token() {
        let auth = MockAuth::new();
        let _ = auth
            .login("user@flint.dev", FIXTURE_PASSWORD)
            .await
            .expect("login");
        let refreshed = auth
            .refresh(FIXTURE_REFRESH)
            .await
            .expect("refresh succeeds");
        assert_eq!(
            refreshed.access_token.expose_secret(),
            "mock-access-token-refreshed"
        );
        assert_eq!(refreshed.refresh_token.expose_secret(), FIXTURE_REFRESH);
    }
}
