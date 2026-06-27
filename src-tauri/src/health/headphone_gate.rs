//! READY → LIVE gate for speaker bleed / no-headphones risk.
//!
//! Extends the installation echo-cancellation check: without OS-level AEC or
//! headphones, interviewer audio on laptop speakers bleeds into the mic and
//! breaks dual-channel speaker separation.

use serde::Serialize;

use crate::health::checks::{check_echo_cancellation, CheckStatus};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadphoneGateStatus {
    pub blocked: bool,
    pub overridden: bool,
    pub message: String,
    pub fix_instruction: Option<String>,
}

/// Evaluate whether LIVE may start without a manual override.
///
/// Phone-call mode skips the gate (single mixed channel — headphones advice
/// does not apply to the dual-stream separation model).
pub fn evaluate(phone_call_mode: bool, overridden: bool) -> HeadphoneGateStatus {
    if phone_call_mode {
        return HeadphoneGateStatus {
            blocked: false,
            overridden,
            message: "Phone-call mode — headphone gate skipped.".to_string(),
            fix_instruction: None,
        };
    }

    if overridden {
        return HeadphoneGateStatus {
            blocked: false,
            overridden: true,
            message: "Headphone gate overridden — live capture may mix speakers and mic."
                .to_string(),
            fix_instruction: None,
        };
    }

    let echo = check_echo_cancellation();
    match echo.status {
        CheckStatus::Pass => HeadphoneGateStatus {
            blocked: false,
            overridden: false,
            message: echo.message,
            fix_instruction: None,
        },
        CheckStatus::Warn | CheckStatus::Fail => HeadphoneGateStatus {
            blocked: true,
            overridden: false,
            message: echo.message,
            fix_instruction: echo.fix_instruction,
        },
    }
}

/// Returns a user-facing error when the gate blocks LIVE.
pub fn live_start_error(status: &HeadphoneGateStatus) -> String {
    let hint = status
        .fix_instruction
        .as_deref()
        .unwrap_or("Wear headphones, enable echo cancellation, or override on the live screen.");
    format!("{} {}", status.message, hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_mode_never_blocks() {
        let status = evaluate(true, false);
        assert!(!status.blocked);
    }

    #[test]
    fn override_clears_block() {
        let status = evaluate(false, true);
        assert!(!status.blocked);
        assert!(status.overridden);
    }

    #[test]
    fn evaluate_matches_echo_check_status() {
        let echo = check_echo_cancellation();
        let status = evaluate(false, false);
        match echo.status {
            CheckStatus::Pass => assert!(!status.blocked),
            CheckStatus::Warn | CheckStatus::Fail => assert!(status.blocked),
        }
    }

    #[test]
    fn live_start_error_includes_fix_hint() {
        let status = HeadphoneGateStatus {
            blocked: true,
            overridden: false,
            message: "Echo risk detected.".to_string(),
            fix_instruction: Some("Wear headphones.".to_string()),
        };
        let err = live_start_error(&status);
        assert!(err.contains("Echo risk detected."));
        assert!(err.contains("Wear headphones."));
    }
}
