//! Resolve LLM providers and build the failover stack (Phase 12.4 / 12.7).

use std::sync::Arc;

use tracing::info;

use super::anthropic::resolve_anthropic;
use super::deepseek::resolve_deepseek;
use super::failover::FailoverManager;
use super::groq::GroqProvider;
use super::openai::resolve_openai;
use super::openrouter;
use super::provider::LLMProvider;
use super::rate_limiter::RateLimiter;
use crate::keychain;
use crate::session::persistence::SessionPersistence;
use secrecy::SecretString;

/// Cloud LLM providers the user may select as primary.
pub const PRIMARY_PROVIDERS: &[&str] = &["groq", "openai", "anthropic", "deepseek"];

pub fn resolve_primary_by_name(name: &str) -> Option<Arc<dyn LLMProvider>> {
    match name {
        "groq" => keychain::get_api_key("groq")
            .ok()
            .and_then(|k| GroqProvider::new(k).ok())
            .map(|p| Arc::new(p) as Arc<dyn LLMProvider>),
        "openai" => resolve_openai(),
        "anthropic" => resolve_anthropic(),
        "deepseek" => resolve_deepseek(),
        _ => None,
    }
}

/// First configured primary in preference order: user setting → groq → openai → anthropic → deepseek.
pub fn resolve_primary(persistence: &SessionPersistence) -> Option<Arc<dyn LLMProvider>> {
    if let Ok(Some(pref)) = persistence.get_preferred_primary_provider() {
        if let Some(provider) = resolve_primary_by_name(&pref) {
            info!(provider = %pref, "primary LLM resolved from user preference");
            return Some(provider);
        }
    }

    for name in PRIMARY_PROVIDERS {
        if let Some(provider) = resolve_primary_by_name(name) {
            info!(provider = %name, "primary LLM resolved from first available key");
            return Some(provider);
        }
    }
    None
}

/// Ordered cloud fallback tiers after primary: DeepSeek → OpenRouter.
/// Skips providers that duplicate the active primary or lack a key.
pub fn resolve_cloud_tiers(primary_name: &str) -> Vec<Arc<dyn LLMProvider>> {
    let mut tiers = Vec::new();

    if primary_name != "deepseek" {
        if let Some(ds) = resolve_deepseek() {
            tiers.push(ds);
        }
    }

    if primary_name != "openrouter" {
        if let Some(or) = openrouter::resolve_openrouter() {
            tiers.push(or);
        }
    }

    tiers
}

pub fn build_failover_manager(
    primary: Arc<dyn LLMProvider>,
    cloud_tiers: Vec<Arc<dyn LLMProvider>>,
    local: Arc<dyn LLMProvider>,
) -> FailoverManager {
    let rate_limiter = Arc::new(RateLimiter::new(
        primary.name(),
        primary.rate_limit().requests_per_minute,
        primary.rate_limit().tokens_per_minute,
    ));
    FailoverManager::new(primary, cloud_tiers, local, rate_limiter)
}

/// Dev-only: seed keychain from `.env` when no key is stored yet.
#[cfg(debug_assertions)]
pub fn bootstrap_dev_keys_from_env() {
    const ENV_MAP: &[(&str, &str)] = &[
        ("groq", "GROQ_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
    ];

    for (provider, env_var) in ENV_MAP {
        if keychain::get_api_key(provider).is_ok() {
            continue;
        }
        if let Ok(value) = std::env::var(env_var) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                let _ = keychain::store_api_key(provider, SecretString::new(trimmed.to_string()));
            }
        }
    }
}

#[cfg(not(debug_assertions))]
pub fn bootstrap_dev_keys_from_env() {}
