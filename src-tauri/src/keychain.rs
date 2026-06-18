//! OS keychain access for API keys and auth tokens.
//!
//! Service name: `flint`. User/entry names: `api_key_{provider}`, `auth_token_access`,
//! `auth_token_refresh`, and `auth_token_expires_at` (RFC3339, required for session restore).

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use keyring::Entry;
use secrecy::{ExposeSecret, SecretString};

use crate::interfaces::auth::AuthToken;

const SERVICE: &str = "flint";

const AUTH_ACCESS_ENTRY: &str = "auth_token_access";
const AUTH_REFRESH_ENTRY: &str = "auth_token_refresh";
const AUTH_EXPIRES_ENTRY: &str = "auth_token_expires_at";
const LEGAL_CONSENT_ENTRY: &str = "legal_consent_accepted";
const REHEARSAL_COMPLETED_ENTRY: &str = "rehearsal_completed";
const OAUTH_PKCE_VERIFIER_ENTRY: &str = "oauth_pkce_verifier";

/// Every LLM provider that may have an API key stored under
/// `api_key_{provider}`. Kept in sync with the providers Flint can connect
/// to so [`clear_all_user_secrets`] never leaves orphan entries behind.
pub const KNOWN_API_PROVIDERS: &[&str] = &["groq", "openrouter", "openai", "anthropic", "tavily"];

const READ_CREDENTIALS_MSG: &str = "Could not read credentials. Please log in again.";
const SAVE_CREDENTIALS_MSG: &str = "Could not save credentials. Please try again.";

fn api_key_entry_name(provider: &str) -> String {
    format!("api_key_{provider}")
}

fn open_entry(user: &str) -> Result<Entry> {
    Entry::new(SERVICE, user).map_err(|_| anyhow!(SAVE_CREDENTIALS_MSG))
}

fn store_password(user: &str, secret: &SecretString) -> Result<()> {
    let entry = open_entry(user)?;
    entry
        .set_password(secret.expose_secret())
        .map_err(|_| anyhow!(SAVE_CREDENTIALS_MSG))
}

fn get_password(user: &str) -> Result<SecretString> {
    let entry = Entry::new(SERVICE, user).map_err(|_| anyhow!(READ_CREDENTIALS_MSG))?;
    let value = entry
        .get_password()
        .map_err(|_| anyhow!(READ_CREDENTIALS_MSG))?;
    Ok(SecretString::new(value))
}

fn delete_password(user: &str) -> Result<()> {
    let entry = open_entry(user)?;
    match entry.delete_password() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(anyhow!(SAVE_CREDENTIALS_MSG)),
    }
}

/// Store an LLM provider API key in the OS keychain.
pub fn store_api_key(provider: &str, key: SecretString) -> Result<()> {
    store_password(&api_key_entry_name(provider), &key)
}

/// Load an LLM provider API key from the OS keychain.
pub fn get_api_key(provider: &str) -> Result<SecretString> {
    get_password(&api_key_entry_name(provider))
}

/// Remove an LLM provider API key from the OS keychain.
pub fn delete_api_key(provider: &str) -> Result<()> {
    delete_password(&api_key_entry_name(provider))
}

/// Persist access and refresh tokens (and expiry) in the OS keychain.
pub fn store_auth_token(token: &AuthToken) -> Result<()> {
    store_password(AUTH_ACCESS_ENTRY, &token.access_token)?;
    store_password(AUTH_REFRESH_ENTRY, &token.refresh_token)?;
    let entry = open_entry(AUTH_EXPIRES_ENTRY)?;
    entry
        .set_password(&token.expires_at.to_rfc3339())
        .map_err(|_| anyhow!(SAVE_CREDENTIALS_MSG))?;
    Ok(())
}

/// Load auth tokens from the OS keychain.
pub fn get_auth_token() -> Result<AuthToken> {
    let access_token = get_password(AUTH_ACCESS_ENTRY)?;
    let refresh_token = get_password(AUTH_REFRESH_ENTRY)?;
    let expires_raw = get_password(AUTH_EXPIRES_ENTRY)?;
    let expires_at = DateTime::parse_from_rfc3339(expires_raw.expose_secret())
        .map_err(|_| anyhow!(READ_CREDENTIALS_MSG))?
        .with_timezone(&Utc);
    Ok(AuthToken {
        access_token,
        refresh_token,
        expires_at,
    })
}

/// Record that the user accepted the first-launch legal disclaimer (§18).
pub fn set_legal_consent_accepted() -> Result<()> {
    store_password(LEGAL_CONSENT_ENTRY, &SecretString::new("1".into()))
}

/// Whether the legal disclaimer was accepted on this device.
pub fn is_legal_consent_accepted() -> bool {
    get_password(LEGAL_CONSENT_ENTRY)
        .map(|v| v.expose_secret() == "1")
        .unwrap_or(false)
}

/// Record that the user completed the mandatory rehearsal before going live.
pub fn set_rehearsal_completed() -> Result<()> {
    store_password(REHEARSAL_COMPLETED_ENTRY, &SecretString::new("1".into()))
}

/// Whether the user has completed rehearsal at least once on this device.
pub fn is_rehearsal_completed() -> bool {
    get_password(REHEARSAL_COMPLETED_ENTRY)
        .map(|v| v.expose_secret() == "1")
        .unwrap_or(false)
}

/// Remove all auth token entries from the OS keychain.
pub fn clear_auth_token() -> Result<()> {
    delete_password(AUTH_ACCESS_ENTRY)?;
    delete_password(AUTH_REFRESH_ENTRY)?;
    delete_password(AUTH_EXPIRES_ENTRY)?;
    Ok(())
}

/// Store the PKCE code verifier between browser open and deep-link callback.
pub fn store_oauth_code_verifier(verifier: &str) -> Result<()> {
    store_password(
        OAUTH_PKCE_VERIFIER_ENTRY,
        &SecretString::new(verifier.to_string()),
    )
}

/// Take the stored PKCE verifier (one-time use).
pub fn take_oauth_code_verifier() -> Result<Option<String>> {
    match get_password(OAUTH_PKCE_VERIFIER_ENTRY) {
        Ok(secret) => {
            let value = secret.expose_secret().clone();
            let _ = delete_password(OAUTH_PKCE_VERIFIER_ENTRY);
            Ok(Some(value))
        }
        Err(_) => Ok(None),
    }
}

/// Drop a pending OAuth PKCE verifier without completing the flow.
pub fn clear_oauth_code_verifier() {
    let _ = delete_password(OAUTH_PKCE_VERIFIER_ENTRY);
}

/// Phase 7.5 — purge account-bound keychain entries for GDPR delete.
///
/// BYOK API keys are **not** cleared — they are device-local credentials the
/// user manages independently of their cloud account.
pub fn clear_account_secrets() -> Result<()> {
    purge_keychain_entries(&[
        AUTH_ACCESS_ENTRY,
        AUTH_REFRESH_ENTRY,
        AUTH_EXPIRES_ENTRY,
        OAUTH_PKCE_VERIFIER_ENTRY,
        LEGAL_CONSENT_ENTRY,
        REHEARSAL_COMPLETED_ENTRY,
    ])
}

/// Purge every keychain entry Flint controls, including BYOK API keys.
///
/// Used by explicit "remove all secrets" flows — not by default account deletion.
#[allow(dead_code)]
pub fn clear_all_user_secrets() -> Result<()> {
    let mut entries: Vec<&str> = vec![
        AUTH_ACCESS_ENTRY,
        AUTH_REFRESH_ENTRY,
        AUTH_EXPIRES_ENTRY,
        LEGAL_CONSENT_ENTRY,
        REHEARSAL_COMPLETED_ENTRY,
    ];
    let api_entries: Vec<String> = KNOWN_API_PROVIDERS
        .iter()
        .map(|p| api_key_entry_name(p))
        .collect();
    let api_refs: Vec<&str> = api_entries.iter().map(String::as_str).collect();
    entries.extend(api_refs);
    purge_keychain_entries(&entries)
}

fn purge_keychain_entries(entries: &[&str]) -> Result<()> {
    let mut first_error: Option<anyhow::Error> = None;

    for entry in entries {
        if let Err(e) = delete_password(entry) {
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    /// Linux CI runners often lack dbus/secret-service; skip rather than fail.
    fn keychain_available() -> bool {
        let probe = SecretString::new("probe".into());
        let provider = "__flint_keychain_probe__";
        if store_api_key(provider, probe).is_err() {
            return false;
        }
        let _ = delete_api_key(provider);
        true
    }

    #[test]
    fn test_api_key_round_trip() {
        if !keychain_available() {
            // Silent skip — OS keychain is not available in many CI/headless
            // environments. Run with `RUST_LOG=warn cargo test -- --nocapture`
            // to surface skips via the tracing subscriber.
            tracing::warn!("SKIP test_api_key_round_trip: OS keychain unavailable");
            return;
        }

        let key = SecretString::new("sk-test-key-12345".into());
        store_api_key("groq", key.clone()).unwrap();
        let retrieved = get_api_key("groq").unwrap();
        assert_eq!(retrieved.expose_secret(), "sk-test-key-12345");
        delete_api_key("groq").unwrap();
        assert!(get_api_key("groq").is_err());
    }
}
