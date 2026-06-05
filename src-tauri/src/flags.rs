//! Phase 7.6 — feature flag evaluation with remote fetch and local kill-switch.
//!
//! ## Evaluation rule (from `.cursor/rules/flint-rust.mdc`)
//!
//! ```text
//! fn is_enabled(flag, user) -> bool {
//!     if !flag.enabled { return false; }
//!     if !flag.allowed_plans.contains(&user.plan) { return false; }
//!     stable_hash(user.id) % 100 < flag.rollout_percentage
//! }
//! ```
//!
//! ## Kill-switch behavior
//!
//! * Remote fetch (Supabase Edge Function `/functions/v1/flags`) succeeds:
//!   results replace the in-memory map and are persisted to disk with the
//!   current timestamp.
//! * Remote fetch fails AND a cached file exists AND it is < 24h old: use
//!   the cache.
//! * Remote fetch fails AND there is no usable cache: fall back to the
//!   compiled-in default — every flag marked `ga = true` is enabled, every
//!   `ga = false` flag is disabled. This guarantees the app boots even with
//!   no network and no prior cache.
//!
//! ## Stable hashing
//!
//! The rollout percentage check uses a deterministic FNV-1a hash so that a
//! given user_id always maps to the same bucket. We deliberately avoid the
//! `std::collections::hash_map::DefaultHasher` because it is randomised per
//! process. A user that lands in bucket 73 at launch must still be in
//! bucket 73 a week later.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::interfaces::auth::Plan;

const FLAGS_FETCH_TIMEOUT_SECS: u64 = 5;
const CACHE_TTL_SECS: i64 = 24 * 60 * 60;
const CACHE_FILE_NAME: &str = "feature_flags.json";

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// Server-side representation of a flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlag {
    pub name: String,
    pub enabled: bool,
    pub allowed_plans: Vec<Plan>,
    /// 0..=100 — fraction of users (by stable hash bucket) that receive the
    /// flag. `100` means "everyone in `allowed_plans` who has `enabled =
    /// true`". `0` means "nobody". Values above 100 are clamped on read.
    pub rollout_percentage: u8,
    /// Marks a flag as Generally Available. Used as the offline-and-no-cache
    /// default — GA flags are enabled, non-GA flags are disabled.
    #[serde(default)]
    pub ga: bool,
}

/// Compiled-in baseline. Keep this short — every entry has to be reasoned
/// about on every launch. Real flag values come from Supabase at runtime.
fn default_flags() -> Vec<FeatureFlag> {
    vec![FeatureFlag {
        name: "post_session_summary".to_string(),
        enabled: true,
        allowed_plans: vec![Plan::Free, Plan::Premium],
        rollout_percentage: 100,
        ga: true,
    }]
}

/// Wire format for the cached file on disk + the Supabase response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagsBundle {
    pub fetched_at: DateTime<Utc>,
    pub flags: Vec<FeatureFlag>,
}

impl FlagsBundle {
    pub fn from_defaults() -> Self {
        Self {
            fetched_at: Utc::now(),
            flags: default_flags(),
        }
    }

    /// True when `fetched_at` is within the 24h freshness window.
    pub fn is_fresh(&self) -> bool {
        let age = Utc::now() - self.fetched_at;
        age.num_seconds() < CACHE_TTL_SECS
    }
}

/// Provenance for the currently active flag set — used by the debug
/// command and exposed to logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagsOrigin {
    Remote,
    Cache,
    Defaults,
}

/// User context the evaluator needs. Decoupled from the full `User` struct
/// so callers don't have to load Supabase profile data to evaluate a flag.
#[derive(Debug, Clone)]
pub struct EvaluationContext {
    pub user_id: Uuid,
    pub plan: Plan,
}

// ────────────────────────────────────────────────────────────────────────────
// Remote source abstraction (so tests can swap it out)
// ────────────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait FlagsSource: Send + Sync {
    /// Fetch the current flag set. Implementors should fail fast — the
    /// caller falls back to the cache or defaults on any error.
    async fn fetch(&self) -> Result<Vec<FeatureFlag>>;
}

/// Default production source: Supabase Edge Function `/functions/v1/flags`.
///
/// The Edge Function is expected to return:
///
/// ```json
/// {
///   "flags": [
///     {
///       "name": "post_session_summary",
///       "enabled": true,
///       "allowed_plans": ["free", "premium"],
///       "rollout_percentage": 100,
///       "ga": true
///     }
///   ]
/// }
/// ```
pub struct SupabaseFlagsSource {
    client: Client,
    endpoint: String,
    anon_key: SecretString,
}

#[derive(Deserialize)]
struct SupabaseFlagsResponse {
    flags: Vec<FeatureFlag>,
}

impl SupabaseFlagsSource {
    pub fn new(base_url: String, anon_key: String) -> Result<Self> {
        let base = base_url.trim_end_matches('/').to_string();
        anyhow::ensure!(!base.is_empty(), "Supabase URL is not configured");
        anyhow::ensure!(!anon_key.is_empty(), "Supabase anon key is not configured");

        let client = Client::builder()
            .timeout(Duration::from_secs(FLAGS_FETCH_TIMEOUT_SECS))
            .build()
            .context("build HTTP client for feature flags")?;
        Ok(Self {
            client,
            endpoint: format!("{base}/functions/v1/flags"),
            anon_key: SecretString::new(anon_key),
        })
    }
}

#[async_trait::async_trait]
impl FlagsSource for SupabaseFlagsSource {
    async fn fetch(&self) -> Result<Vec<FeatureFlag>> {
        let response = self
            .client
            .get(&self.endpoint)
            .header("apikey", self.anon_key.expose_secret())
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.anon_key.expose_secret()),
            )
            .send()
            .await
            .context("Supabase /flags request failed")?;

        let status = response.status();
        if status != StatusCode::OK {
            anyhow::bail!("Supabase /flags returned HTTP {status}");
        }

        let body: SupabaseFlagsResponse = response
            .json()
            .await
            .context("Supabase /flags returned unparseable JSON")?;
        Ok(body.flags)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Client
// ────────────────────────────────────────────────────────────────────────────

/// In-memory feature-flag evaluator with disk-backed kill switch.
///
/// Construction never blocks on the network: [`load`] reads the disk cache
/// (or falls back to defaults) so the very first `is_enabled` call returns
/// immediately. A separate [`refresh_from`] call pulls fresh values from
/// the supplied [`FlagsSource`] — schedule it at startup and any time the
/// auth token rotates.
pub struct FeatureFlagClient {
    /// `(bundle, origin)` — protected by a single RwLock so reads stay cheap.
    inner: RwLock<(FlagsBundle, FlagsOrigin)>,
    cache_path: PathBuf,
}

impl FeatureFlagClient {
    /// Build a client whose cache file lives at `cache_path`. On first run
    /// the file does not exist yet — the constructor seeds the in-memory
    /// state with the compiled defaults so evaluation works from t=0.
    pub fn load(cache_path: PathBuf) -> Self {
        let (bundle, origin) = match read_cache(&cache_path) {
            Ok(Some(bundle)) if bundle.is_fresh() => {
                info!(
                    age_secs = (Utc::now() - bundle.fetched_at).num_seconds(),
                    "feature flags loaded from cache"
                );
                (bundle, FlagsOrigin::Cache)
            }
            Ok(Some(_stale)) => {
                warn!("feature flag cache is stale; using compiled defaults until refresh");
                (FlagsBundle::from_defaults(), FlagsOrigin::Defaults)
            }
            Ok(None) => {
                info!("no feature flag cache; seeding compiled defaults");
                (FlagsBundle::from_defaults(), FlagsOrigin::Defaults)
            }
            Err(e) => {
                warn!(error = %e, "feature flag cache unreadable; using compiled defaults");
                (FlagsBundle::from_defaults(), FlagsOrigin::Defaults)
            }
        };

        Self {
            inner: RwLock::new((bundle, origin)),
            cache_path,
        }
    }

    /// Refresh the flag set from `source`. On success the new bundle is
    /// written through to disk. On failure the in-memory state is left
    /// untouched — typically that means the previous remote response, the
    /// cache, or the defaults remain authoritative.
    pub async fn refresh_from(&self, source: &dyn FlagsSource) -> Result<()> {
        let flags = source.fetch().await?;
        let bundle = FlagsBundle {
            fetched_at: Utc::now(),
            flags,
        };

        if let Err(e) = write_cache(&self.cache_path, &bundle) {
            warn!(error = %e, "feature flag cache write failed");
        }

        {
            let mut guard = self.inner.write().expect("flags lock poisoned");
            *guard = (bundle, FlagsOrigin::Remote);
        }
        info!("feature flags refreshed from remote");
        Ok(())
    }

    /// Evaluate `flag` for `ctx`. Unknown flags fall back to the compiled
    /// default; if the flag isn't compiled either, the result is `false`.
    pub fn is_enabled(&self, flag: &str, ctx: &EvaluationContext) -> bool {
        let guard = self.inner.read().expect("flags lock poisoned");
        let bundle = &guard.0;
        let lookup = bundle
            .flags
            .iter()
            .find(|f| f.name == flag)
            .cloned()
            .or_else(|| default_flags().into_iter().find(|f| f.name == flag));
        match lookup {
            None => false,
            Some(flag_cfg) => evaluate(&flag_cfg, ctx),
        }
    }

    /// Snapshot of the current state for diagnostics + the debug command.
    pub fn snapshot(&self) -> ClientSnapshot {
        let guard = self.inner.read().expect("flags lock poisoned");
        let (bundle, origin) = &*guard;
        ClientSnapshot {
            origin: *origin,
            fetched_at: bundle.fetched_at,
            flag_count: bundle.flags.len(),
            flags: bundle.flags.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientSnapshot {
    pub origin: FlagsOrigin,
    pub fetched_at: DateTime<Utc>,
    pub flag_count: usize,
    pub flags: Vec<FeatureFlag>,
}

// ────────────────────────────────────────────────────────────────────────────
// Evaluation core (pure functions — easy to unit test)
// ────────────────────────────────────────────────────────────────────────────

/// Pure evaluator. Exposed for testing — production code uses
/// [`FeatureFlagClient::is_enabled`] which adds the cache + default lookup.
pub fn evaluate(flag: &FeatureFlag, ctx: &EvaluationContext) -> bool {
    if !flag.enabled {
        return false;
    }
    if !flag.allowed_plans.contains(&ctx.plan) {
        return false;
    }
    let bucket = stable_hash(ctx.user_id) % 100;
    let rollout = flag.rollout_percentage.min(100) as u64;
    bucket < rollout
}

/// Deterministic 64-bit FNV-1a hash of a UUID. Identical across processes
/// and Flint releases — the rollout bucket assignment must not drift when
/// the user reopens the app.
pub fn stable_hash(id: Uuid) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ────────────────────────────────────────────────────────────────────────────
// On-disk cache helpers (testable)
// ────────────────────────────────────────────────────────────────────────────

pub fn cache_path_in(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join(CACHE_FILE_NAME)
}

fn read_cache(path: &std::path::Path) -> Result<Option<FlagsBundle>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).context("read feature flag cache")?;
    let bundle: FlagsBundle =
        serde_json::from_str(&raw).context("parse feature flag cache JSON")?;
    Ok(Some(bundle))
}

fn write_cache(path: &std::path::Path, bundle: &FlagsBundle) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create cache dir")?;
    }
    // Atomic write: stage to a sibling tmp file, then rename. Guards against
    // a partial flush leaving an unparseable cache after a crash mid-write.
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(bundle).context("serialise flags bundle")?;
    std::fs::write(&tmp, json).context("write feature flag cache (tmp)")?;
    std::fs::rename(&tmp, path).context("rename feature flag cache into place")?;
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Convenience helpers used by Tauri commands + app startup
// ────────────────────────────────────────────────────────────────────────────

/// Build the production Supabase source from `plugins.supabase` in
/// `tauri.conf.json`. Returns `None` if the plugin block is missing or
/// malformed — the client then keeps running on cache + defaults.
pub fn supabase_source_from_plugins(
    plugins: &HashMap<String, serde_json::Value>,
) -> Option<SupabaseFlagsSource> {
    let raw = plugins.get("supabase")?;
    let url = raw.get("url")?.as_str()?.to_string();
    let key = raw.get("anonKey")?.as_str()?.to_string();
    SupabaseFlagsSource::new(url, key).ok()
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn user(plan: Plan) -> EvaluationContext {
        EvaluationContext {
            user_id: Uuid::nil(),
            plan,
        }
    }

    fn flag(rollout: u8, plans: &[Plan]) -> FeatureFlag {
        FeatureFlag {
            name: "test_flag".to_string(),
            enabled: true,
            allowed_plans: plans.to_vec(),
            rollout_percentage: rollout,
            ga: false,
        }
    }

    // ── Pure evaluator ────────────────────────────────────────────────────

    #[test]
    fn disabled_flag_is_never_enabled() {
        let mut f = flag(100, &[Plan::Free, Plan::Premium]);
        f.enabled = false;
        assert!(!evaluate(&f, &user(Plan::Free)));
    }

    #[test]
    fn plan_not_in_allowed_list_is_disabled() {
        let f = flag(100, &[Plan::Premium]);
        assert!(!evaluate(&f, &user(Plan::Free)));
        assert!(evaluate(&f, &user(Plan::Premium)));
    }

    #[test]
    fn rollout_zero_disables_for_everyone() {
        let f = flag(0, &[Plan::Free, Plan::Premium]);
        assert!(!evaluate(&f, &user(Plan::Free)));
    }

    #[test]
    fn rollout_one_hundred_enables_for_everyone() {
        let f = flag(100, &[Plan::Free, Plan::Premium]);
        for _ in 0..50 {
            let ctx = EvaluationContext {
                user_id: Uuid::new_v4(),
                plan: Plan::Free,
            };
            assert!(evaluate(&f, &ctx));
        }
    }

    #[test]
    fn rollout_overflow_is_clamped() {
        let mut f = flag(200, &[Plan::Free]);
        f.rollout_percentage = 200;
        let ctx = EvaluationContext {
            user_id: Uuid::new_v4(),
            plan: Plan::Free,
        };
        // 200 clamps to 100, so the flag must fire.
        assert!(evaluate(&f, &ctx));
    }

    #[test]
    fn rollout_50_splits_population_roughly_evenly() {
        let f = flag(50, &[Plan::Free]);
        let mut hits = 0;
        let n = 10_000;
        for _ in 0..n {
            let ctx = EvaluationContext {
                user_id: Uuid::new_v4(),
                plan: Plan::Free,
            };
            if evaluate(&f, &ctx) {
                hits += 1;
            }
        }
        // FNV-1a over random UUIDs should land around 50% +/- 3%.
        assert!(
            (hits as f64 / n as f64 - 0.5).abs() < 0.03,
            "skew too large: {hits}/{n}"
        );
    }

    // ── Stable hash ───────────────────────────────────────────────────────

    #[test]
    fn stable_hash_is_deterministic() {
        let id = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap();
        let h1 = stable_hash(id);
        let h2 = stable_hash(id);
        assert_eq!(h1, h2);
    }

    #[test]
    fn stable_hash_differs_for_different_ids() {
        let h1 = stable_hash(Uuid::nil());
        let h2 = stable_hash(Uuid::from_u128(1));
        assert_ne!(h1, h2);
    }

    // ── Bundle freshness ──────────────────────────────────────────────────

    #[test]
    fn bundle_is_fresh_within_24h() {
        let bundle = FlagsBundle {
            fetched_at: Utc::now() - chrono::Duration::hours(23),
            flags: Vec::new(),
        };
        assert!(bundle.is_fresh());
    }

    #[test]
    fn bundle_is_stale_after_24h() {
        let bundle = FlagsBundle {
            fetched_at: Utc::now() - chrono::Duration::hours(25),
            flags: Vec::new(),
        };
        assert!(!bundle.is_fresh());
    }

    // ── Disk cache round-trip ─────────────────────────────────────────────

    #[test]
    fn cache_round_trips_through_disk() {
        let tmp = TempDir::new().unwrap();
        let path = cache_path_in(tmp.path());
        let bundle = FlagsBundle {
            fetched_at: Utc::now(),
            flags: vec![flag(75, &[Plan::Premium])],
        };
        write_cache(&path, &bundle).unwrap();
        let read = read_cache(&path).unwrap().unwrap();
        assert_eq!(read.flags.len(), 1);
        assert_eq!(read.flags[0].name, "test_flag");
        assert_eq!(read.flags[0].rollout_percentage, 75);
    }

    #[test]
    fn read_cache_returns_none_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = cache_path_in(tmp.path());
        assert!(read_cache(&path).unwrap().is_none());
    }

    // ── Client (kill-switch behavior) ─────────────────────────────────────

    struct StubSource {
        flags: Vec<FeatureFlag>,
        fail: bool,
        call_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl FlagsSource for StubSource {
        async fn fetch(&self) -> Result<Vec<FeatureFlag>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                anyhow::bail!("simulated network outage");
            }
            Ok(self.flags.clone())
        }
    }

    fn premium_user() -> EvaluationContext {
        EvaluationContext {
            user_id: Uuid::nil(),
            plan: Plan::Premium,
        }
    }

    #[tokio::test]
    async fn fresh_install_uses_compiled_defaults_until_refresh() {
        let tmp = TempDir::new().unwrap();
        let client = FeatureFlagClient::load(cache_path_in(tmp.path()));

        // post_session_summary is a GA default — should already be true.
        assert!(client.is_enabled("post_session_summary", &premium_user()));
        // Unknown flag → false.
        assert!(!client.is_enabled("nonexistent", &premium_user()));
        assert_eq!(client.snapshot().origin, FlagsOrigin::Defaults);
    }

    #[tokio::test]
    async fn refresh_persists_to_disk_and_flips_source_to_remote() {
        let tmp = TempDir::new().unwrap();
        let client = FeatureFlagClient::load(cache_path_in(tmp.path()));
        let source = StubSource {
            flags: vec![FeatureFlag {
                name: "experimental_panel".to_string(),
                enabled: true,
                allowed_plans: vec![Plan::Premium],
                rollout_percentage: 100,
                ga: false,
            }],
            fail: false,
            call_count: AtomicUsize::new(0),
        };

        client.refresh_from(&source).await.expect("fetch ok");

        assert_eq!(client.snapshot().origin, FlagsOrigin::Remote);
        assert!(client.is_enabled("experimental_panel", &premium_user()));
        // Disk cache exists and parses.
        let cached = read_cache(&cache_path_in(tmp.path())).unwrap().unwrap();
        assert_eq!(cached.flags.len(), 1);
    }

    #[tokio::test]
    async fn supabase_unreachable_falls_back_to_cache() {
        let tmp = TempDir::new().unwrap();
        let cache_path = cache_path_in(tmp.path());

        // Seed the cache from a successful run.
        write_cache(
            &cache_path,
            &FlagsBundle {
                fetched_at: Utc::now() - chrono::Duration::hours(2),
                flags: vec![FeatureFlag {
                    name: "experimental_panel".to_string(),
                    enabled: true,
                    allowed_plans: vec![Plan::Free, Plan::Premium],
                    rollout_percentage: 100,
                    ga: false,
                }],
            },
        )
        .unwrap();

        let client = FeatureFlagClient::load(cache_path);
        assert_eq!(client.snapshot().origin, FlagsOrigin::Cache);

        // Now try a refresh that fails — cache must remain authoritative.
        let bad_source = StubSource {
            flags: Vec::new(),
            fail: true,
            call_count: AtomicUsize::new(0),
        };
        let refresh = client.refresh_from(&bad_source).await;
        assert!(refresh.is_err(), "refresh must surface the network error");
        assert_eq!(client.snapshot().origin, FlagsOrigin::Cache);
        assert!(client.is_enabled("experimental_panel", &premium_user()));
    }

    #[tokio::test]
    async fn stale_cache_is_ignored_in_favor_of_defaults() {
        let tmp = TempDir::new().unwrap();
        let cache_path = cache_path_in(tmp.path());
        write_cache(
            &cache_path,
            &FlagsBundle {
                fetched_at: Utc::now() - chrono::Duration::hours(25),
                flags: vec![FeatureFlag {
                    name: "experimental_panel".to_string(),
                    enabled: true,
                    allowed_plans: vec![Plan::Premium],
                    rollout_percentage: 100,
                    ga: false,
                }],
            },
        )
        .unwrap();

        let client = FeatureFlagClient::load(cache_path);
        assert_eq!(client.snapshot().origin, FlagsOrigin::Defaults);
        // The stale cache's flag must NOT carry into the new bundle.
        assert!(!client.is_enabled("experimental_panel", &premium_user()));
        // The compiled-in GA default still works.
        assert!(client.is_enabled("post_session_summary", &premium_user()));
    }

    #[tokio::test]
    async fn corrupt_cache_is_ignored_in_favor_of_defaults() {
        let tmp = TempDir::new().unwrap();
        let cache_path = cache_path_in(tmp.path());
        std::fs::write(&cache_path, "{ this is not valid json }").unwrap();

        let client = FeatureFlagClient::load(cache_path);
        assert_eq!(client.snapshot().origin, FlagsOrigin::Defaults);
        assert!(client.is_enabled("post_session_summary", &premium_user()));
    }

    // ── Concurrent reads ──────────────────────────────────────────────────

    #[tokio::test]
    async fn concurrent_is_enabled_calls_dont_deadlock() {
        let tmp = TempDir::new().unwrap();
        let client = Arc::new(FeatureFlagClient::load(cache_path_in(tmp.path())));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let c = Arc::clone(&client);
            handles.push(tokio::spawn(async move {
                for _ in 0..1_000 {
                    let _ = c.is_enabled("post_session_summary", &premium_user());
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }
}
