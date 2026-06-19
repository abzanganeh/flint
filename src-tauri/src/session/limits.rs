//! Concurrent open-session caps by subscription tier.
//!
//! An "open" session is any row not in `IDLE` or `ENDED` — i.e. still in setup,
//! rehearsal, live, or crash recovery. Completed (`ENDED`) sessions are
//! reopenable for review but do not count toward the cap.

use crate::interfaces::auth::Plan;
use crate::session::state::SessionState;

/// Maximum concurrent open sessions on the free plan.
pub const OPEN_SESSION_LIMIT_FREE: usize = 3;

/// Maximum concurrent open sessions on the premium plan.
pub const OPEN_SESSION_LIMIT_PREMIUM: usize = 6;

/// Returns the open-session cap for the given plan.
pub fn open_session_limit(plan: Plan) -> usize {
    match plan {
        Plan::Free => OPEN_SESSION_LIMIT_FREE,
        Plan::Premium => OPEN_SESSION_LIMIT_PREMIUM,
    }
}

/// Whether a persisted session state counts toward the open-session cap.
pub fn is_open_session_state(state: SessionState) -> bool {
    !matches!(state, SessionState::Idle | SessionState::Ended)
}

/// User-facing error when the open-session cap is reached.
pub fn open_session_limit_message(plan: Plan, limit: usize) -> String {
    match plan {
        Plan::Free => format!(
            "You have {limit} open sessions (free plan limit). \
             Close or delete one from Past Sessions, or upgrade to Premium for up to {premium} open sessions.",
            premium = OPEN_SESSION_LIMIT_PREMIUM
        ),
        Plan::Premium => format!(
            "You have {limit} open sessions (Premium plan limit). \
             Close or delete one from Past Sessions before opening another."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_match_product_tiers() {
        assert_eq!(open_session_limit(Plan::Free), 3);
        assert_eq!(open_session_limit(Plan::Premium), 6);
    }

    #[test]
    fn ended_and_idle_are_not_open() {
        assert!(!is_open_session_state(SessionState::Idle));
        assert!(!is_open_session_state(SessionState::Ended));
    }

    #[test]
    fn draft_and_live_states_are_open() {
        assert!(is_open_session_state(SessionState::Configuring));
        assert!(is_open_session_state(SessionState::Rehearsing));
        assert!(is_open_session_state(SessionState::Live));
        assert!(is_open_session_state(SessionState::Crashed));
    }
}
