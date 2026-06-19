//! Wiremock integration tests for Phase 12 LLM providers.

use flint_lib::llm::anthropic::AnthropicProvider;
use flint_lib::llm::deepseek::new_deepseek;
use flint_lib::llm::openai::new_openai;
use flint_lib::llm::provider::{CompletionConfig, LLMProvider};
use futures::StreamExt;
use secrecy::SecretString;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn collect_stream(
    provider: &dyn LLMProvider,
    prompt: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let config = CompletionConfig {
        max_tokens: Some(32),
        temperature: 0.0,
        stream: true,
    };
    let mut stream = provider.complete_stream(prompt.to_string(), config).await?;
    let mut out = String::new();
    while let Some(token) = stream.next().await {
        out.push_str(&token?);
    }
    Ok(out)
}

fn openai_sse_body() -> String {
    [
        r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n")
}

#[tokio::test]
async fn deepseek_streams_tokens_via_wiremock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(openai_sse_body()))
        .mount(&server)
        .await;

    let provider = new_deepseek(SecretString::new("test-key".into()))
        .expect("deepseek provider")
        .with_base_url(format!("{}/v1/chat/completions", server.uri()));

    let text = collect_stream(&provider, "ping").await.expect("stream");
    assert_eq!(text, "Hello");
}

#[tokio::test]
async fn openai_streams_tokens_via_wiremock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(openai_sse_body()))
        .mount(&server)
        .await;

    let provider = new_openai(SecretString::new("test-key".into()))
        .expect("openai provider")
        .with_base_url(format!("{}/v1/chat/completions", server.uri()));

    let text = collect_stream(&provider, "ping").await.expect("stream");
    assert_eq!(text, "Hello");
}

#[tokio::test]
async fn anthropic_streams_tokens_via_wiremock() {
    let server = MockServer::start().await;
    let body = [
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        "",
    ]
    .join("\n");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(SecretString::new("test-key".into()))
        .expect("anthropic provider")
        .with_base_url(format!("{}/v1/messages", server.uri()));

    let text = collect_stream(&provider, "ping").await.expect("stream");
    assert_eq!(text, "Hi");
}

#[tokio::test]
async fn failover_primary_to_deepseek_to_ollama() {
    use flint_lib::llm::failover::FailoverManager;
    use flint_lib::llm::provider::{FailingMockLLMProvider, MockLLMProvider};
    use flint_lib::llm::rate_limiter::RateLimiter;
    use std::sync::Arc;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "groq".to_string(),
        error_message: "connection refused".to_string(),
    });

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let failing_deepseek = new_deepseek(SecretString::new("key".into()))
        .expect("deepseek")
        .with_base_url(format!("{}/v1/chat/completions", server.uri()));
    let cloud: Arc<dyn LLMProvider> = Arc::new(failing_deepseek);

    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "local ok".to_string(),
        provider_name: "ollama".to_string(),
    });

    let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
    let manager = FailoverManager::new(primary, vec![cloud], Arc::clone(&local), rl);

    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("mock app")
        .handle()
        .clone();

    let mut stream = manager
        .complete_stream(
            "test".to_string(),
            CompletionConfig {
                max_tokens: Some(16),
                temperature: 0.0,
                stream: true,
            },
            &app,
            50,
        )
        .await
        .expect("cascade to ollama");

    assert!(manager.is_using_local());
    let token = stream.next().await.unwrap().unwrap();
    assert_eq!(token, "local ok");
}

#[tokio::test]
async fn failover_primary_to_deepseek_on_hard_failure() {
    use flint_lib::llm::failover::FailoverManager;
    use flint_lib::llm::provider::{FailingMockLLMProvider, MockLLMProvider};
    use flint_lib::llm::rate_limiter::RateLimiter;
    use std::sync::Arc;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "groq".to_string(),
        error_message: "connection refused".to_string(),
    });

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(openai_sse_body()))
        .mount(&server)
        .await;

    let deepseek = new_deepseek(SecretString::new("key".into()))
        .expect("deepseek")
        .with_base_url(format!("{}/v1/chat/completions", server.uri()));
    let cloud: Arc<dyn LLMProvider> = Arc::new(deepseek);

    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "should not run".to_string(),
        provider_name: "ollama".to_string(),
    });

    let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
    let manager = FailoverManager::new(primary, vec![cloud], local, rl);

    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("mock app")
        .handle()
        .clone();

    let mut stream = manager
        .complete_stream(
            "test".to_string(),
            CompletionConfig {
                max_tokens: Some(16),
                temperature: 0.0,
                stream: true,
            },
            &app,
            50,
        )
        .await
        .expect("deepseek cloud tier should serve");

    assert!(!manager.is_using_local());
    assert_eq!(manager.active_provider_name(), "deepseek");
    let token = stream.next().await.unwrap().unwrap();
    assert_eq!(token, "Hello");
}
