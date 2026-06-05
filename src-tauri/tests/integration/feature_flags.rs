//! Phase 7.6 — feature flag client end-to-end.
//!
//! These tests treat the whole `FeatureFlagClient` as a black box:
//!
//! * Cold-start with no cache → compiled defaults (so the GA-marked
//!   `post_session_summary` flag is reachable from t=0).
//! * Successful refresh → on-disk cache appears, subsequent cold start
//!   loads from cache instead of defaults.
//! * Supabase unreachable AFTER a successful refresh → cache stays
//!   authoritative; the prior flag values are still served.
//! * Cache expires (24h TTL) → next cold start falls back to defaults
//!   even though the file is on disk.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use tempfile::TempDir;
use uuid::Uuid;

use flint_lib::flags::{
    cache_path_in, EvaluationContext, FeatureFlag, FeatureFlagClient, FlagsBundle, FlagsOrigin,
    FlagsSource,
};
use flint_lib::interfaces::auth::Plan;

// ── Test fixtures ────────────────────────────────────────────────────────────

struct StubSource {
    flags: Vec<FeatureFlag>,
    fail: AtomicBool,
    calls: AtomicUsize,
}

impl StubSource {
    fn ok(flags: Vec<FeatureFlag>) -> Self {
        Self {
            flags,
            fail: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        }
    }

    fn always_fails() -> Self {
        Self {
            flags: Vec::new(),
            fail: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl FlagsSource for StubSource {
    async fn fetch(&self) -> Result<Vec<FeatureFlag>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("simulated outage");
        }
        Ok(self.flags.clone())
    }
}

fn user(plan: Plan) -> EvaluationContext {
    EvaluationContext {
        user_id: Uuid::nil(),
        plan,
    }
}

fn flag(name: &str, enabled: bool, plans: &[Plan], rollout: u8) -> FeatureFlag {
    FeatureFlag {
        name: name.to_string(),
        enabled,
        allowed_plans: plans.to_vec(),
        rollout_percentage: rollout,
        ga: false,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fresh_install_serves_compiled_defaults() {
    let tmp = TempDir::new().unwrap();
    let client = FeatureFlagClient::load(cache_path_in(tmp.path()));

    let snap = client.snapshot();
    assert_eq!(snap.origin, FlagsOrigin::Defaults);
    // The compiled GA flag is reachable even with no network and no cache.
    assert!(client.is_enabled("post_session_summary", &user(Plan::Free)));
}

#[tokio::test]
async fn successful_refresh_persists_and_replaces_defaults() {
    let tmp = TempDir::new().unwrap();
    let cache_path = cache_path_in(tmp.path());
    let client = FeatureFlagClient::load(cache_path.clone());

    let source = StubSource::ok(vec![flag("depth_panel_v2", true, &[Plan::Premium], 100)]);

    client.refresh_from(&source).await.expect("refresh ok");
    assert_eq!(source.call_count(), 1);

    let snap = client.snapshot();
    assert_eq!(snap.origin, FlagsOrigin::Remote);
    assert_eq!(snap.flag_count, 1);
    assert!(cache_path.exists(), "cache file must be written through");
    assert!(client.is_enabled("depth_panel_v2", &user(Plan::Premium)));
    assert!(!client.is_enabled("depth_panel_v2", &user(Plan::Free)));
}

#[tokio::test]
async fn cold_restart_after_refresh_reads_from_cache_not_defaults() {
    let tmp = TempDir::new().unwrap();
    let cache_path = cache_path_in(tmp.path());

    // First "run" — seed the cache via a successful refresh.
    {
        let client = FeatureFlagClient::load(cache_path.clone());
        let source = StubSource::ok(vec![flag(
            "rag_v2",
            true,
            &[Plan::Free, Plan::Premium],
            100,
        )]);
        client.refresh_from(&source).await.unwrap();
    }

    // Second "run" — restart the app, no refresh, kill-switch should
    // promote the cache to the authoritative source.
    let client = FeatureFlagClient::load(cache_path);
    let snap = client.snapshot();
    assert_eq!(snap.origin, FlagsOrigin::Cache);
    assert!(client.is_enabled("rag_v2", &user(Plan::Free)));
}

#[tokio::test]
async fn supabase_unreachable_keeps_cache_authoritative() {
    let tmp = TempDir::new().unwrap();
    let cache_path = cache_path_in(tmp.path());

    // Seed cache.
    {
        let client = FeatureFlagClient::load(cache_path.clone());
        let source = StubSource::ok(vec![flag("rag_v2", true, &[Plan::Premium], 100)]);
        client.refresh_from(&source).await.unwrap();
    }

    // New session, simulated network outage.
    let client = FeatureFlagClient::load(cache_path);
    assert_eq!(client.snapshot().origin, FlagsOrigin::Cache);

    let bad = StubSource::always_fails();
    let outcome = client.refresh_from(&bad).await;
    assert!(outcome.is_err(), "outage must surface to the caller");

    // Cache survives the failed refresh — `rag_v2` is still on.
    assert_eq!(client.snapshot().origin, FlagsOrigin::Cache);
    assert!(client.is_enabled("rag_v2", &user(Plan::Premium)));
}

#[tokio::test]
async fn expired_cache_falls_back_to_defaults_on_cold_start() {
    let tmp = TempDir::new().unwrap();
    let cache_path = cache_path_in(tmp.path());

    // Hand-craft a stale bundle directly on disk: 30h old, marks a
    // non-default flag as enabled. The kill switch must ignore it.
    let stale = FlagsBundle {
        fetched_at: chrono::Utc::now() - chrono::Duration::hours(30),
        flags: vec![flag("rag_v2", true, &[Plan::Premium], 100)],
    };
    let json = serde_json::to_string_pretty(&stale).unwrap();
    std::fs::write(&cache_path, json).unwrap();

    let client = FeatureFlagClient::load(cache_path);
    let snap = client.snapshot();
    assert_eq!(snap.origin, FlagsOrigin::Defaults);
    // Stale flag is gone, compiled GA default is back.
    assert!(!client.is_enabled("rag_v2", &user(Plan::Premium)));
    assert!(client.is_enabled("post_session_summary", &user(Plan::Premium)));
}

#[tokio::test]
async fn unknown_flag_falls_through_to_compiled_default_then_false() {
    let tmp = TempDir::new().unwrap();
    let client = FeatureFlagClient::load(cache_path_in(tmp.path()));

    // Compiled-in GA → true.
    assert!(client.is_enabled("post_session_summary", &user(Plan::Free)));
    // Never-heard-of-it → false (must NOT crash).
    assert!(!client.is_enabled("totally_made_up_flag", &user(Plan::Premium)));
}
