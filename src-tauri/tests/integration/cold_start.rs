//! Cold-start deep-link integration tests.
//!
//! Verifies `capture_cold_start_token_from_env` — the path exercised by the
//! Linux dev handler (`scripts/flint-deeplink-handler.sh`) which sets
//! `FLINT_IMPORT_URL` before launching the binary, and by the dev workflow
//! documented in `STRATEGY_B_INTEGRATION_PLAN.md §1.4`.

use std::sync::OnceLock;

use flint_lib::deep_link;
use tokio::sync::Mutex;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn cold_start_env_var_produces_token() {
    let _guard = env_lock().lock().await;
    std::env::set_var(
        "FLINT_IMPORT_URL",
        "flint://import?token=550e8400-e29b-41d4-a716-446655440000",
    );

    let token = deep_link::capture_cold_start_token_from_env();

    std::env::remove_var("FLINT_IMPORT_URL");
    assert_eq!(
        token.as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
}

#[tokio::test]
async fn cold_start_env_var_unset_returns_none() {
    let _guard = env_lock().lock().await;
    std::env::remove_var("FLINT_IMPORT_URL");
    assert!(deep_link::capture_cold_start_token_from_env().is_none());
}

#[tokio::test]
async fn cold_start_env_var_invalid_scheme_returns_none() {
    let _guard = env_lock().lock().await;
    std::env::set_var(
        "FLINT_IMPORT_URL",
        "https://not-flint.example.com/?token=abc",
    );
    let token = deep_link::capture_cold_start_token_from_env();
    std::env::remove_var("FLINT_IMPORT_URL");
    assert!(token.is_none());
}

#[tokio::test]
async fn cold_start_env_var_whitespace_only_returns_none() {
    let _guard = env_lock().lock().await;
    std::env::set_var("FLINT_IMPORT_URL", "   ");
    let token = deep_link::capture_cold_start_token_from_env();
    std::env::remove_var("FLINT_IMPORT_URL");
    assert!(token.is_none());
}

#[tokio::test]
async fn cold_start_env_var_expired_token_over_64_chars_rejected() {
    let _guard = env_lock().lock().await;
    let long = "x".repeat(65);
    std::env::set_var("FLINT_IMPORT_URL", format!("flint://import?token={long}"));
    let token = deep_link::capture_cold_start_token_from_env();
    std::env::remove_var("FLINT_IMPORT_URL");
    assert!(token.is_none());
}
