//! DeepSeek streaming provider — OpenAI-compatible API (Phase 12.1).

use std::sync::Arc;
use std::time::Duration;

use secrecy::SecretString;

use super::openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
use super::provider::LLMProvider;

const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "deepseek-chat";
const CONTEXT_WINDOW: usize = 64_000;
const RATE_LIMIT_RPM: u32 = 48;
const RATE_LIMIT_TPM: u32 = 48_000;
const USER_AGENT: &str = "Flint/1.0 (https://github.com/abzanganeh/flint)";

pub fn deepseek_config() -> OpenAiCompatConfig {
    OpenAiCompatConfig {
        provider_name: "deepseek",
        base_url: DEEPSEEK_BASE_URL.to_string(),
        default_model: DEFAULT_MODEL,
        context_window: CONTEXT_WINDOW,
        requests_per_minute: RATE_LIMIT_RPM,
        tokens_per_minute: RATE_LIMIT_TPM,
        request_timeout: Duration::from_secs(60),
        user_agent: Some(USER_AGENT),
    }
}

pub fn new_deepseek(api_key: SecretString) -> Result<OpenAiCompatProvider, anyhow::Error> {
    OpenAiCompatProvider::new(api_key, deepseek_config())
}

pub fn resolve_deepseek() -> Option<Arc<dyn LLMProvider>> {
    let api_key = crate::keychain::get_api_key("deepseek").ok()?;
    new_deepseek(api_key)
        .map(|p| Arc::new(p) as Arc<dyn LLMProvider>)
        .ok()
}
