//! Subscription entitlement checks for v1.
//!
//! v1 scope is intentionally minimal: callers only need to know whether the
//! signed-in user has an active paid subscription. Credit holds, metered
//! ledgers, and Smart Resume API lookups are deferred to Strategy B Phase 3.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::interfaces::auth::{AuthInterface, AuthToken, Plan};

/// Result of an entitlement lookup. v1 exposes only subscription status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntitlementStatus {
    pub has_active_subscription: bool,
}

/// Contract for subscription entitlement resolution.
#[async_trait]
pub trait EntitlementChecker: Send + Sync {
    async fn check(&self, token: &AuthToken) -> Result<EntitlementStatus>;
}

/// Resolves entitlement from the authenticated user's Supabase plan field.
pub struct PlanEntitlementChecker {
    auth: Arc<dyn AuthInterface>,
}

impl PlanEntitlementChecker {
    pub fn new(auth: Arc<dyn AuthInterface>) -> Self {
        Self { auth }
    }
}

#[async_trait]
impl EntitlementChecker for PlanEntitlementChecker {
    async fn check(&self, token: &AuthToken) -> Result<EntitlementStatus> {
        let user = self.auth.get_current_user(token).await?;
        Ok(EntitlementStatus {
            has_active_subscription: user.plan == Plan::Premium,
        })
    }
}

/// Fixed entitlement for unit/integration tests.
#[cfg(test)]
pub struct StubEntitlementChecker {
    status: EntitlementStatus,
}

#[cfg(test)]
impl StubEntitlementChecker {
    pub fn new(has_active_subscription: bool) -> Self {
        Self {
            status: EntitlementStatus {
                has_active_subscription,
            },
        }
    }
}

#[cfg(test)]
#[async_trait]
impl EntitlementChecker for StubEntitlementChecker {
    async fn check(&self, _token: &AuthToken) -> Result<EntitlementStatus> {
        Ok(self.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::mock_auth::MockAuth;
    use secrecy::SecretString;

    fn token() -> AuthToken {
        AuthToken {
            access_token: SecretString::new("access".to_string()),
            refresh_token: SecretString::new("refresh".to_string()),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        }
    }

    #[tokio::test]
    async fn premium_user_has_active_subscription() {
        let auth = Arc::new(MockAuth::with_logged_in_plan(Plan::Premium));
        let checker = PlanEntitlementChecker::new(auth);
        let status = checker.check(&token()).await.expect("check succeeds");
        assert!(status.has_active_subscription);
    }

    #[tokio::test]
    async fn free_user_has_no_active_subscription() {
        let auth = Arc::new(MockAuth::with_logged_in_plan(Plan::Free));
        let checker = PlanEntitlementChecker::new(auth);
        let status = checker.check(&token()).await.expect("check succeeds");
        assert!(!status.has_active_subscription);
    }

    #[tokio::test]
    async fn stub_checker_returns_configured_status() {
        let checker = StubEntitlementChecker::new(true);
        let status = checker.check(&token()).await.expect("check succeeds");
        assert!(status.has_active_subscription);
    }
}
