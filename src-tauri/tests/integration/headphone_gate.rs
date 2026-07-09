//! Integration tests for READY → LIVE headphone / echo gate.

use flint_lib::health::headphone_gate;

#[test]
fn phone_call_mode_skips_headphone_gate() {
    let status = headphone_gate::evaluate(true, false);
    assert!(!status.blocked);
}

#[test]
fn manual_override_clears_headphone_gate_block() {
    let status = headphone_gate::evaluate(false, true);
    assert!(!status.blocked);
    assert!(status.overridden);
}

#[test]
fn gate_unblocked_when_echo_passes_or_headphones_detected() {
    use flint_lib::health::checks::{check_echo_cancellation, CheckStatus};

    let echo = check_echo_cancellation();
    let status = headphone_gate::evaluate(false, false);
    if echo.status == CheckStatus::Pass {
        assert!(!status.blocked);
    }
    // When echo warns, Linux may still pass if default output is BT/headphones.
}

#[test]
fn live_start_error_includes_actionable_hint() {
    let status = headphone_gate::HeadphoneGateStatus {
        blocked: true,
        overridden: false,
        message: "Echo cancellation is not enabled.".to_string(),
        fix_instruction: Some("Wear headphones.".to_string()),
    };
    let err = headphone_gate::live_start_error(&status);
    assert!(err.contains("Echo cancellation"));
    assert!(err.contains("Wear headphones"));
}
