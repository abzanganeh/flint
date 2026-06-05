//! Supabase URL + anon-key resolution.
//!
//! `tauri.conf.json` ships with **empty** values for `plugins.supabase.url`
//! and `plugins.supabase.anonKey` to comply with the security rule that
//! forbids API keys on disk in committed source (`flint-security.mdc`).
//!
//! At runtime callers must instead supply both values via environment
//! variables — these override anything that may have been left in the
//! config file by accident:
//!
//! | Variable | Purpose |
//! |---|---|
//! | `FLINT_SUPABASE_URL` | Base URL of the Supabase project. |
//! | `FLINT_SUPABASE_ANON_KEY` | Public anon key. RLS still gates every table. |
//!
//! Local dev: export both in your shell before `cargo tauri dev`.
//! CI / packaged builds: inject via the build environment.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

const URL_ENV_VAR: &str = "FLINT_SUPABASE_URL";
const ANON_KEY_ENV_VAR: &str = "FLINT_SUPABASE_ANON_KEY";

#[derive(Debug, Clone)]
pub struct SupabaseConfig {
    pub url: String,
    pub anon_key: String,
}

#[derive(Debug, Deserialize, Default)]
struct PluginShape {
    #[serde(default)]
    url: String,
    #[serde(default, rename = "anonKey")]
    anon_key: String,
}

/// Resolve the active Supabase config: environment variables win over the
/// `plugins.supabase` block in `tauri.conf.json`.
///
/// Returns `Ok(None)` when neither source supplies a usable pair — callers
/// that can keep working without Supabase (e.g. feature-flag client) should
/// treat that as "no remote". Callers that require Supabase (e.g. auth)
/// should call [`resolve_supabase_config_required`] instead.
pub fn resolve_supabase_config(
    plugins: &HashMap<String, serde_json::Value>,
) -> Option<SupabaseConfig> {
    let shape: PluginShape = plugins
        .get("supabase")
        .cloned()
        .map(|raw| serde_json::from_value(raw).unwrap_or_default())
        .unwrap_or_default();

    let url = std::env::var(URL_ENV_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or(shape.url);
    let anon_key = std::env::var(ANON_KEY_ENV_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or(shape.anon_key);

    if url.is_empty() || anon_key.is_empty() {
        return None;
    }
    Some(SupabaseConfig { url, anon_key })
}

/// Variant for code paths that cannot function without Supabase.
pub fn resolve_supabase_config_required(
    plugins: &HashMap<String, serde_json::Value>,
) -> Result<SupabaseConfig> {
    resolve_supabase_config(plugins).with_context(|| {
        format!(
            "Supabase config missing: set {URL_ENV_VAR} and {ANON_KEY_ENV_VAR} or populate plugins.supabase in tauri.conf.json"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    // Process-global env vars — tests must not race over them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn scrub_env() {
        std::env::remove_var(URL_ENV_VAR);
        std::env::remove_var(ANON_KEY_ENV_VAR);
    }

    #[test]
    fn env_overrides_plugin_values() {
        let _g = ENV_LOCK.lock().unwrap();
        scrub_env();
        std::env::set_var(URL_ENV_VAR, "https://env-url");
        std::env::set_var(ANON_KEY_ENV_VAR, "env-key");
        let mut plugins = HashMap::new();
        plugins.insert(
            "supabase".to_string(),
            json!({ "url": "https://plugin-url", "anonKey": "plugin-key" }),
        );
        let cfg = resolve_supabase_config(&plugins).expect("env wins");
        assert_eq!(cfg.url, "https://env-url");
        assert_eq!(cfg.anon_key, "env-key");
        scrub_env();
    }

    #[test]
    fn falls_back_to_plugin_values_when_env_missing() {
        let _g = ENV_LOCK.lock().unwrap();
        scrub_env();
        let mut plugins = HashMap::new();
        plugins.insert(
            "supabase".to_string(),
            json!({ "url": "https://plugin-url", "anonKey": "plugin-key" }),
        );
        let cfg = resolve_supabase_config(&plugins).expect("plugin fallback");
        assert_eq!(cfg.url, "https://plugin-url");
        assert_eq!(cfg.anon_key, "plugin-key");
    }

    #[test]
    fn returns_none_when_both_sources_empty() {
        let _g = ENV_LOCK.lock().unwrap();
        scrub_env();
        let mut plugins = HashMap::new();
        plugins.insert("supabase".to_string(), json!({ "url": "", "anonKey": "" }));
        assert!(resolve_supabase_config(&plugins).is_none());
    }

    #[test]
    fn required_helper_errors_when_missing() {
        let _g = ENV_LOCK.lock().unwrap();
        scrub_env();
        let plugins = HashMap::new();
        assert!(resolve_supabase_config_required(&plugins).is_err());
    }
}
