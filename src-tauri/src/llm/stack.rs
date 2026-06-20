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

/// Cloud LLM providers the user may select as primary (legacy radio picker).
pub const PRIMARY_PROVIDERS: &[&str] = &["groq", "openai", "anthropic", "deepseek"];

/// Full cloud provider list the user may reorder (Ollama is always last-resort, not reorderable).
pub const REORDERABLE_CLOUD_PROVIDERS: &[&str] =
    &["groq", "openai", "anthropic", "deepseek", "openrouter"];

/// Default priority when the user has not configured an order.
pub const DEFAULT_PROVIDER_PRIORITY: &[&str] =
    &["groq", "openai", "anthropic", "deepseek", "openrouter"];

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

/// Resolve a cloud provider by name, including OpenRouter fallback tier.
pub fn resolve_cloud_provider_by_name(name: &str) -> Option<Arc<dyn LLMProvider>> {
    match name {
        "openrouter" => openrouter::resolve_openrouter(),
        other => resolve_primary_by_name(other),
    }
}

/// Ordered cloud fallback names after `primary`, preserving user priority.
pub fn cloud_fallback_names(order: &[String], primary: &str) -> Vec<String> {
    order
        .iter()
        .filter(|name| name.as_str() != primary && name.as_str() != "ollama")
        .cloned()
        .collect()
}

/// First configured provider in the user's priority order.
pub fn resolve_primary(persistence: &SessionPersistence) -> Option<Arc<dyn LLMProvider>> {
    let order = persistence
        .get_provider_priority()
        .unwrap_or_else(|_| default_provider_priority());

    for name in &order {
        if let Some(provider) = resolve_cloud_provider_by_name(name) {
            info!(provider = %name, "primary LLM resolved from user priority order");
            return Some(provider);
        }
    }
    None
}

/// Ordered cloud fallback tiers after primary, following the user's priority list.
pub fn resolve_cloud_tiers(
    primary_name: &str,
    persistence: &SessionPersistence,
) -> Vec<Arc<dyn LLMProvider>> {
    let order = persistence
        .get_provider_priority()
        .unwrap_or_else(|_| default_provider_priority());

    cloud_fallback_names(&order, primary_name)
        .into_iter()
        .filter_map(|name| resolve_cloud_provider_by_name(&name))
        .collect()
}

pub fn default_provider_priority() -> Vec<String> {
    DEFAULT_PROVIDER_PRIORITY
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

pub fn normalize_provider_priority(order: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut normalized = Vec::new();

    for name in order {
        let lower = name.to_lowercase();
        if lower == "ollama" || !REORDERABLE_CLOUD_PROVIDERS.contains(&lower.as_str()) {
            continue;
        }
        if seen.insert(lower.clone()) {
            normalized.push(lower);
        }
    }

    for name in DEFAULT_PROVIDER_PRIORITY {
        if !seen.contains(*name) {
            normalized.push((*name).to_string());
        }
    }

    normalized
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::SessionPersistence;

    #[test]
    fn normalize_deduplicates_and_appends_missing_defaults() {
        let input = vec![
            "deepseek".to_string(),
            "groq".to_string(),
            "deepseek".to_string(),
            "ollama".to_string(),
        ];
        let out = normalize_provider_priority(&input);
        assert_eq!(out[0], "deepseek");
        assert_eq!(out[1], "groq");
        assert!(out.contains(&"openai".to_string()));
        assert!(!out.contains(&"ollama".to_string()));
    }

    #[test]
    fn cloud_fallback_names_skips_primary_and_ollama() {
        let order = vec![
            "groq".to_string(),
            "deepseek".to_string(),
            "openrouter".to_string(),
            "ollama".to_string(),
        ];
        let fallbacks = cloud_fallback_names(&order, "groq");
        assert_eq!(fallbacks, vec!["deepseek", "openrouter"]);
    }

    #[test]
    fn custom_priority_order_respected_by_resolve_primary() {
        let db = SessionPersistence::new(":memory:").expect("in-memory db");
        db.set_provider_priority(&[
            "deepseek".to_string(),
            "groq".to_string(),
            "openai".to_string(),
        ])
        .expect("set priority");

        let order = db.get_provider_priority().expect("read priority");
        assert_eq!(order[0], "deepseek");

        if let Some(provider) = resolve_primary(&db) {
            let first_available = order
                .iter()
                .find(|name| resolve_cloud_provider_by_name(name).is_some())
                .expect("at least one provider must be available for this assertion");
            assert_eq!(provider.name(), first_available.as_str());
        }
    }

    #[test]
    fn provider_without_key_is_skipped_in_fallback_chain() {
        let db = SessionPersistence::new(":memory:").expect("in-memory db");
        db.set_provider_priority(&[
            "groq".to_string(),
            "deepseek".to_string(),
            "openrouter".to_string(),
        ])
        .expect("set priority");

        let tiers = resolve_cloud_tiers("groq", &db);
        let names: Vec<_> = tiers.iter().map(|p| p.name().to_string()).collect();
        assert!(!names.contains(&"groq".to_string()));
    }
}
