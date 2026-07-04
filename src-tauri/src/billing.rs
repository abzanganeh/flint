//! v1 billing model — BYOK plus a flat Pro subscription tier.
//!
//! Strategy B Phase 3 metered credit ledgers, holds, and Smart Resume
//! entitlement API lookups are explicitly out of scope. Users bring their own
//! LLM keys; Pro (`Plan::Premium`) unlocks higher product limits via plan
//! gating elsewhere (open-session caps, feature flags), not per-token billing.

use crate::interfaces::auth::Plan;
use crate::keychain;

/// Primary LLM providers that satisfy BYOK for live inference (excludes Tavily).
const LLM_BYOK_PROVIDERS: &[&str] = &["groq", "deepseek", "openrouter", "openai", "anthropic"];

/// User-visible billing tier for v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingTier {
    /// Free plan — BYOK required for cloud LLM; basic session limits.
    FreeByok,
    /// Flat Pro subscription — still BYOK in v1; higher limits via plan gates.
    Pro,
}

impl BillingTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreeByok => "free",
            Self::Pro => "pro",
        }
    }
}

/// Snapshot returned to the Settings UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingStatus {
    pub tier: BillingTier,
    pub has_byok_llm_key: bool,
    /// Always `false` in v1 — metered billing is deferred.
    pub metered_billing_enabled: bool,
}

pub fn billing_tier_for_plan(plan: Plan) -> BillingTier {
    match plan {
        Plan::Premium => BillingTier::Pro,
        Plan::Free => BillingTier::FreeByok,
    }
}

/// True when at least one cloud LLM provider key is stored in the OS keychain.
pub fn has_byok_llm_key() -> bool {
    LLM_BYOK_PROVIDERS
        .iter()
        .any(|provider| keychain::get_api_key(provider).is_ok())
}

/// Gate live-session start on v1 billing rules (BYOK required; no credit ledger).
pub fn validate_live_session_billing(plan: Plan) -> Result<(), String> {
    let _tier = billing_tier_for_plan(plan);
    if has_byok_llm_key() {
        return Ok(());
    }
    Err(billing_access_error(plan))
}

pub fn billing_status(plan: Plan) -> BillingStatus {
    BillingStatus {
        tier: billing_tier_for_plan(plan),
        has_byok_llm_key: has_byok_llm_key(),
        metered_billing_enabled: false,
    }
}

fn billing_access_error(plan: Plan) -> String {
    match billing_tier_for_plan(plan) {
        BillingTier::Pro => "Add a cloud LLM API key in Settings → API Keys to start a live session. \
                            Pro includes higher session limits; inference still uses your keys in v1."
            .to_string(),
        BillingTier::FreeByok => "Add a cloud LLM API key in Settings → API Keys to start a live session. \
                                  Flint v1 uses bring-your-own-key billing — no metered credits."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premium_maps_to_pro_tier() {
        assert_eq!(billing_tier_for_plan(Plan::Premium), BillingTier::Pro);
    }

    #[test]
    fn free_maps_to_byok_tier() {
        assert_eq!(billing_tier_for_plan(Plan::Free), BillingTier::FreeByok);
    }

    #[test]
    fn billing_status_never_enables_metered_billing() {
        let status = billing_status(Plan::Free);
        assert!(!status.metered_billing_enabled);
    }

    #[test]
    fn validate_requires_byok_when_no_keys_present() {
        // CI/dev machines may have keys in the OS keychain; only assert error shape
        // when validation fails so the test stays deterministic off-keychain.
        if has_byok_llm_key() {
            assert!(validate_live_session_billing(Plan::Free).is_ok());
        } else {
            let err = validate_live_session_billing(Plan::Free).expect_err("missing BYOK");
            assert!(err.contains("bring-your-own-key"));
        }
    }
}
