//! Phase 7.4 — cost cap enforcement.
//!
//! Tracks cumulative token usage and cost across the lifetime of an
//! authenticated app session. The orchestrator consults this tracker
//! **before** dispatching a new turn, so a runaway session cannot accrue
//! charges past a user-configured ceiling.
//!
//! ## Cap modes
//!
//! Either or both of the following may be configured. The first cap to be
//! breached suspends inference:
//!
//! * `max_total_tokens` — hard ceiling on `input + output` tokens combined
//! * `max_cost_estimate_usd` — hard ceiling on the rolling cost estimate
//!
//! When both are `None` the tracker behaves as an unbounded counter — no
//! warnings, no suspension. This matches v1 behavior for users who have
//! not opted into a cap yet.
//!
//! ## State transitions
//!
//! ```text
//!   Below threshold ─ record_turn ─→ Below   (Status::Ok)
//!   Below           ─ record_turn ─→ Warn80  (Status::Warning80)
//!   Warn80          ─ record_turn ─→ Reached (Status::Reached, suspended=true)
//!   Reached         ─ record_turn ─→ Reached (idempotent)
//!   Suspended       ─ lift()      ─→ Below   (counters preserved)
//!   *               ─ reset()     ─→ Below   (counters zeroed)
//! ```
//!
//! ## Safety
//!
//! `record_turn` is the only mutator on `cumulative`. It runs under a
//! single mutex so concurrent orchestrator turns serialise their bookkeeping
//! and never race past the cap.

#![allow(dead_code)]

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Threshold at which the soft "approaching cap" warning fires (80%).
const WARNING_FRACTION: f64 = 0.80;

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// Running per-session totals. Always non-negative; monotonic until `reset`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CumulativeUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_estimate_usd: f64,
}

/// Optional caps applied to [`CumulativeUsage`]. `None` means "no ceiling".
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostCap {
    pub max_total_tokens: Option<u64>,
    pub max_cost_estimate_usd: Option<f64>,
}

impl CostCap {
    /// True when at least one ceiling is configured.
    pub fn is_active(&self) -> bool {
        self.max_total_tokens.is_some() || self.max_cost_estimate_usd.is_some()
    }
}

/// Result of recording a single turn's usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostCapStatus {
    /// Within budget (below 80% of every configured ceiling).
    Ok,
    /// At or above 80% of any configured ceiling.
    Warning80,
    /// At or above 100% of a configured ceiling — inference is now suspended.
    Reached,
}

/// Fraction of the strictest active cap that has been spent so far. Returns
/// `None` when no cap is configured.
fn fraction_against_cap(usage: &CumulativeUsage, cap: &CostCap) -> Option<f64> {
    let token_fraction = cap
        .max_total_tokens
        .filter(|c| *c > 0)
        .map(|c| usage.total_tokens as f64 / c as f64);
    let cost_fraction = cap
        .max_cost_estimate_usd
        .filter(|c| *c > 0.0)
        .map(|c| usage.cost_estimate_usd / c);
    match (token_fraction, cost_fraction) {
        (Some(t), Some(c)) => Some(t.max(c)),
        (Some(t), None) => Some(t),
        (None, Some(c)) => Some(c),
        (None, None) => None,
    }
}

/// Snapshot of the tracker's observable state — used by Tauri commands and
/// the test suite.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CostStatus {
    pub usage: CumulativeUsage,
    pub cap: CostCap,
    pub suspended: bool,
    pub status: CostCapStatus,
    /// Fraction of the strictest cap consumed (`0.0..=1.0+`), or `None` if
    /// no cap is configured.
    pub fraction_used: Option<f64>,
}

// ────────────────────────────────────────────────────────────────────────────
// Tracker
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Inner {
    usage: CumulativeUsage,
    cap: CostCap,
    suspended: bool,
    /// Tracks the highest warning emitted since the last reset / lift so we
    /// don't spam `Warning80` events every turn after the threshold is hit.
    last_emitted_status: Option<CostCapStatus>,
}

/// Process-wide cost tracker. One instance lives in `AppState`; cloned via
/// `Arc<CostTracker>` for the orchestrator and command handlers.
#[derive(Debug, Default)]
pub struct CostTracker {
    inner: Mutex<Inner>,
}

impl CostTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one turn's usage and return the resulting cap status.
    ///
    /// `Status::Reached` automatically flips the suspended flag. Subsequent
    /// turns must call [`Self::lift_suspension`] before another inference is
    /// permitted. The returned status reflects the **new** state, *not* a
    /// delta — callers should treat repeated `Reached` results as idempotent.
    pub fn record_turn(&self, input_tokens: u64, output_tokens: u64, cost_usd: f64) -> CostStatus {
        let mut inner = self.inner.lock().expect("cost tracker mutex poisoned");
        inner.usage.input_tokens = inner.usage.input_tokens.saturating_add(input_tokens);
        inner.usage.output_tokens = inner.usage.output_tokens.saturating_add(output_tokens);
        inner.usage.total_tokens = inner
            .usage
            .total_tokens
            .saturating_add(input_tokens + output_tokens);
        inner.usage.cost_estimate_usd += cost_usd;
        Self::resolve_status(&mut inner)
    }

    /// Snapshot the current state without mutating it.
    pub fn snapshot(&self) -> CostStatus {
        let inner = self.inner.lock().expect("cost tracker mutex poisoned");
        let fraction = fraction_against_cap(&inner.usage, &inner.cap);
        let status = match fraction {
            Some(f) if f >= 1.0 => CostCapStatus::Reached,
            Some(f) if f >= WARNING_FRACTION => CostCapStatus::Warning80,
            _ => CostCapStatus::Ok,
        };
        CostStatus {
            usage: inner.usage,
            cap: inner.cap,
            suspended: inner.suspended,
            status,
            fraction_used: fraction,
        }
    }

    /// Update the configured cap. If the new cap is already breached by the
    /// existing usage, the tracker is suspended immediately.
    pub fn set_cap(&self, cap: CostCap) -> CostStatus {
        let mut inner = self.inner.lock().expect("cost tracker mutex poisoned");
        inner.cap = cap;
        // Re-arm warning emission so the next turn / status check fires the
        // appropriate event under the new cap.
        inner.last_emitted_status = None;
        Self::resolve_status(&mut inner)
    }

    /// Clear the suspension flag while preserving cumulative counters.
    /// Used when the user opts to keep going past a soft cap; the cap itself
    /// must be widened separately or the next `record_turn` will re-suspend.
    ///
    /// Crucially, this does NOT run through `resolve_status` — that helper
    /// re-arms the suspended flag whenever usage is still above 100% of the
    /// cap, which would defeat the lift. The next `record_turn` is what
    /// re-evaluates against the (presumably widened) cap.
    pub fn lift_suspension(&self) -> CostStatus {
        let mut inner = self.inner.lock().expect("cost tracker mutex poisoned");
        inner.suspended = false;
        inner.last_emitted_status = None;
        let fraction = fraction_against_cap(&inner.usage, &inner.cap);
        let status = match fraction {
            Some(f) if f >= 1.0 => CostCapStatus::Reached,
            Some(f) if f >= WARNING_FRACTION => CostCapStatus::Warning80,
            _ => CostCapStatus::Ok,
        };
        CostStatus {
            usage: inner.usage,
            cap: inner.cap,
            suspended: inner.suspended,
            status,
            fraction_used: fraction,
        }
    }

    /// Zero every counter. Called at session end so the next session starts
    /// from a clean slate.
    pub fn reset(&self) -> CostStatus {
        let mut inner = self.inner.lock().expect("cost tracker mutex poisoned");
        inner.usage = CumulativeUsage::default();
        inner.suspended = false;
        inner.last_emitted_status = None;
        Self::resolve_status(&mut inner)
    }

    /// True when inference is currently suspended.
    pub fn is_suspended(&self) -> bool {
        self.inner
            .lock()
            .expect("cost tracker mutex poisoned")
            .suspended
    }

    /// Returns the (cap status, was-this-the-first-time-we-saw-this-status)
    /// pair, but only the first element is exposed publicly. Internally we
    /// use the second element to suppress duplicate warning events.
    fn resolve_status(inner: &mut Inner) -> CostStatus {
        let fraction = fraction_against_cap(&inner.usage, &inner.cap);
        let status = match fraction {
            Some(f) if f >= 1.0 => CostCapStatus::Reached,
            Some(f) if f >= WARNING_FRACTION => CostCapStatus::Warning80,
            _ => CostCapStatus::Ok,
        };

        if matches!(status, CostCapStatus::Reached) {
            inner.suspended = true;
        }

        CostStatus {
            usage: inner.usage,
            cap: inner.cap,
            suspended: inner.suspended,
            status,
            fraction_used: fraction,
        }
    }

    /// Internal helper for callers that want to know whether the most recent
    /// status mutation changed the emitted-warning level. Used by the
    /// orchestrator to fire events only on transitions.
    pub fn record_turn_with_transition(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) -> (CostStatus, bool) {
        let mut inner = self.inner.lock().expect("cost tracker mutex poisoned");
        inner.usage.input_tokens = inner.usage.input_tokens.saturating_add(input_tokens);
        inner.usage.output_tokens = inner.usage.output_tokens.saturating_add(output_tokens);
        inner.usage.total_tokens = inner
            .usage
            .total_tokens
            .saturating_add(input_tokens + output_tokens);
        inner.usage.cost_estimate_usd += cost_usd;

        let status = Self::resolve_status(&mut inner);
        let is_new_status = inner.last_emitted_status != Some(status.status);
        if is_new_status {
            inner.last_emitted_status = Some(status.status);
        }
        (status, is_new_status)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cap_tokens(n: u64) -> CostCap {
        CostCap {
            max_total_tokens: Some(n),
            max_cost_estimate_usd: None,
        }
    }

    fn cap_cost(c: f64) -> CostCap {
        CostCap {
            max_total_tokens: None,
            max_cost_estimate_usd: Some(c),
        }
    }

    #[test]
    fn unbounded_tracker_never_warns_or_suspends() {
        let t = CostTracker::new();
        let status = t.record_turn(1_000_000, 2_000_000, 100.0);
        assert_eq!(status.status, CostCapStatus::Ok);
        assert!(!status.suspended);
        assert!(status.fraction_used.is_none());
    }

    #[test]
    fn token_cap_warns_at_80pct() {
        let t = CostTracker::new();
        t.set_cap(cap_tokens(1_000));
        let status = t.record_turn(400, 400, 0.0);
        assert_eq!(status.status, CostCapStatus::Warning80);
        assert!(!status.suspended);
        assert_eq!(status.usage.total_tokens, 800);
    }

    #[test]
    fn token_cap_reaches_at_100pct_and_suspends() {
        let t = CostTracker::new();
        t.set_cap(cap_tokens(1_000));
        let status = t.record_turn(500, 500, 0.0);
        assert_eq!(status.status, CostCapStatus::Reached);
        assert!(status.suspended);
    }

    #[test]
    fn cost_cap_independent_of_token_cap() {
        let t = CostTracker::new();
        t.set_cap(cap_cost(1.00));
        // Way under the (unset) token cap, but cost crosses 80% of $1.00.
        let status = t.record_turn(100, 100, 0.85);
        assert_eq!(status.status, CostCapStatus::Warning80);
    }

    #[test]
    fn whichever_cap_is_strictest_wins() {
        let t = CostTracker::new();
        t.set_cap(CostCap {
            max_total_tokens: Some(1_000_000),
            max_cost_estimate_usd: Some(1.00),
        });
        // Tokens are 0.0001% of cap; cost is 95% — must Warn.
        let status = t.record_turn(100, 100, 0.95);
        assert_eq!(status.status, CostCapStatus::Warning80);
        assert!(status.fraction_used.unwrap() > 0.90);
    }

    #[test]
    fn lift_suspension_preserves_counters_but_clears_suspended() {
        let t = CostTracker::new();
        t.set_cap(cap_tokens(100));
        t.record_turn(60, 60, 0.0);
        assert!(t.is_suspended());

        let after_lift = t.lift_suspension();
        assert!(!after_lift.suspended);
        assert_eq!(after_lift.usage.total_tokens, 120);
        // Lift does NOT widen the cap, so the next turn re-suspends.
        let next = t.record_turn(10, 10, 0.0);
        assert_eq!(next.status, CostCapStatus::Reached);
        assert!(next.suspended);
    }

    #[test]
    fn reset_zeroes_counters_and_clears_state() {
        let t = CostTracker::new();
        t.set_cap(cap_tokens(100));
        t.record_turn(80, 80, 0.0);
        assert!(t.is_suspended());

        let after_reset = t.reset();
        assert_eq!(after_reset.usage.total_tokens, 0);
        assert_eq!(after_reset.usage.cost_estimate_usd, 0.0);
        assert!(!after_reset.suspended);
        assert_eq!(after_reset.status, CostCapStatus::Ok);
    }

    #[test]
    fn set_cap_below_current_usage_immediately_suspends() {
        let t = CostTracker::new();
        t.record_turn(500, 500, 0.0);
        let status = t.set_cap(cap_tokens(100));
        assert_eq!(status.status, CostCapStatus::Reached);
        assert!(status.suspended);
    }

    #[test]
    fn record_turn_with_transition_only_flags_first_occurrence() {
        let t = CostTracker::new();
        t.set_cap(cap_tokens(1_000));
        let (_, first) = t.record_turn_with_transition(400, 400, 0.0); // 80%
        assert!(first, "first Warning80 should be flagged");
        let (_, second) = t.record_turn_with_transition(50, 50, 0.0); // still 80%
        assert!(!second, "still Warning80 must not re-emit");
        let (_, third) = t.record_turn_with_transition(50, 50, 0.0); // crosses 100%
        assert!(third, "transition to Reached must be flagged");
    }

    #[test]
    fn snapshot_matches_record_turn_result() {
        let t = CostTracker::new();
        t.set_cap(cap_tokens(1_000));
        let after_record = t.record_turn(100, 100, 0.05);
        let snap = t.snapshot();
        assert_eq!(snap.usage, after_record.usage);
        assert_eq!(snap.cap, after_record.cap);
        assert_eq!(snap.suspended, after_record.suspended);
        assert_eq!(snap.status, after_record.status);
    }

    #[test]
    fn zero_or_negative_cap_values_are_ignored() {
        let t = CostTracker::new();
        t.set_cap(CostCap {
            max_total_tokens: Some(0),
            max_cost_estimate_usd: Some(0.0),
        });
        // Both fields treated as unset — no cap active even though Some(0).
        let status = t.record_turn(1_000, 1_000, 100.0);
        assert_eq!(status.status, CostCapStatus::Ok);
        assert!(status.fraction_used.is_none());
    }

    #[test]
    fn cap_is_active_only_when_at_least_one_field_set() {
        assert!(!CostCap::default().is_active());
        assert!(cap_tokens(1).is_active());
        assert!(cap_cost(0.01).is_active());
    }
}
