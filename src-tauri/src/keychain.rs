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

/// Every LLM provider that may have an API key stored under
/// `api_key_{provider}`. Kept in sync with the providers Flint can connect
/// to so [`clear_all_user_secrets`] never leaves orphan entries behind.
pub const KNOWN_API_PROVIDERS: &[&str] = &["groq", "openai", "anthropic"];

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

/// Phase 7.5 — purge every keychain entry Flint controls for the current user.
///
/// Best-effort: each `delete_password` call already tolerates `NoEntry`. We
/// continue past individual failures so a corrupt single entry cannot block
/// the GDPR delete flow, and report the first error (if any) back to the
/// caller so the UI can warn the user that manual cleanup may be required.
///
/// Cleared entries:
/// * Auth tokens (access, refresh, expires_at)
/// * Legal-consent flag
/// * Rehearsal-completed flag
/// * Every API key registered under [`KNOWN_API_PROVIDERS`]
pub fn clear_all_user_secrets() -> Result<()> {
    let mut first_error: Option<anyhow::Error> = None;

    let entries: Vec<String> = std::iter::once(AUTH_ACCESS_ENTRY.to_string())
        .chain(std::iter::once(AUTH_REFRESH_ENTRY.to_string()))
        .chain(std::iter::once(AUTH_EXPIRES_ENTRY.to_string()))
        .chain(std::iter::once(LEGAL_CONSENT_ENTRY.to_string()))
        .chain(std::iter::once(REHEARSAL_COMPLETED_ENTRY.to_string()))
        .chain(KNOWN_API_PROVIDERS.iter().map(|p| api_key_entry_name(p)))
        .collect();

    for entry in entries {
        if let Err(e) = delete_password(&entry) {
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
