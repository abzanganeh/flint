//! OpenAI streaming provider (Phase 12.2).

use std::sync::Arc;
use std::time::Duration;

use secrecy::SecretString;

use super::openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
use super::provider::LLMProvider;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "gpt-4o-mini";
const CONTEXT_WINDOW: usize = 128_000;
const RATE_LIMIT_RPM: u32 = 48;
const RATE_LIMIT_TPM: u32 = 48_000;

pub fn openai_config() -> OpenAiCompatConfig {
    OpenAiCompatConfig {
        provider_name: "openai",
        base_url: OPENAI_BASE_URL.to_string(),
        default_model: DEFAULT_MODEL,
        context_window: CONTEXT_WINDOW,
        requests_per_minute: RATE_LIMIT_RPM,
        tokens_per_minute: RATE_LIMIT_TPM,
        request_timeout: Duration::from_secs(60),
        user_agent: None,
    }
}

pub fn new_openai(api_key: SecretString) -> Result<OpenAiCompatProvider, anyhow::Error> {
    OpenAiCompatProvider::new(api_key, openai_config())
}

pub fn resolve_openai() -> Option<Arc<dyn LLMProvider>> {
    let api_key = crate::keychain::get_api_key("openai").ok()?;
    new_openai(api_key)
        .map(|p| Arc::new(p) as Arc<dyn LLMProvider>)
        .ok()
}
