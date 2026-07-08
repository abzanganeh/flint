//! READY → LIVE gate for speaker bleed / no-headphones risk.
//!
//! Live may start when any of: phone-call mode, user override, OS echo-cancel
//! module loaded, or Linux default output is headphones / Bluetooth / external.

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
    if echo.status == CheckStatus::Pass {
        return HeadphoneGateStatus {
            blocked: false,
            overridden: false,
            message: echo.message,
            fix_instruction: None,
        };
    }

    #[cfg(target_os = "linux")]
    if headphones_likely_in_use() {
        return HeadphoneGateStatus {
            blocked: false,
            overridden: false,
            message: "Headphones or external audio output detected — live capture allowed."
                .to_string(),
            fix_instruction: None,
        };
    }

    HeadphoneGateStatus {
        blocked: true,
        overridden: false,
        message: echo.message,
        fix_instruction: echo.fix_instruction,
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

#[cfg(target_os = "linux")]
fn headphones_likely_in_use() -> bool {
    let Some(sink) = pactl_default_sink() else {
        return false;
    };
    sink_name_indicates_headphones(&sink)
        || sink_active_port_indicates_headphones(&sink)
}

#[cfg(target_os = "linux")]
fn pactl_default_sink() -> Option<String> {
    let output = std::process::Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sink.is_empty() {
        None
    } else {
        Some(sink)
    }
}

#[cfg(target_os = "linux")]
fn sink_name_indicates_headphones(sink: &str) -> bool {
    let s = sink.to_lowercase();
    // Bluetooth sinks are virtually always a headset/headphones accessory.
    s.contains("bluez")
        || s.contains("headphone")
        || s.contains("headset")
        || s.contains("buds")
        || s.contains("airpods")
}

#[cfg(target_os = "linux")]
fn sink_active_port_indicates_headphones(default_sink: &str) -> bool {
    let Ok(output) = std::process::Command::new("pactl")
        .args(["list", "sinks"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(block) = extract_sink_block(&text, default_sink) else {
        return false;
    };
    parse_active_port_headphones(&block)
}

#[cfg(target_os = "linux")]
fn extract_sink_block(list_output: &str, sink_name: &str) -> Option<String> {
    let needle = format!("Name: {sink_name}");
    let start = list_output.find(&needle)?;
    let rest = &list_output[start..];
    // Next sink block starts at "Sink #"; first line is our Name.
    let end = rest[1..]
        .find("\nSink #")
        .map(|i| start + 1 + i)
        .unwrap_or(list_output.len());
    Some(list_output[start..end].to_string())
}

#[cfg(target_os = "linux")]
fn parse_active_port_headphones(block: &str) -> bool {
    for line in block.lines() {
        let trimmed = line.trim();
        let Some(port) = trimmed.strip_prefix("Active Port:") else {
            continue;
        };
        let port = port.trim().to_lowercase();
        if port.is_empty() || port == "none" {
            return false;
        }
        if port.contains("headphone") || port.contains("headset") {
            return true;
        }
        // Explicit laptop speakers — not headphones.
        if port.contains("speaker") && !port.contains("headphone") {
            return false;
        }
        // USB DAC / HDMI / line-out — external, not built-in speakers.
        if port.contains("usb") || port.contains("hdmi") || port.contains("line-out") {
            return true;
        }
        return false;
    }
    false
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

    #[cfg(target_os = "linux")]
    #[test]
    fn bluez_sink_name_is_headphones() {
        assert!(sink_name_indicates_headphones(
            "bluez_output.88_08_94_86_78_A1.1"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn analog_active_port_headphones() {
        let block = "Name: alsa_output.pci.analog-stereo\n\tActive Port: analog-output-headphones\n";
        assert!(parse_active_port_headphones(block));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn analog_active_port_speakers() {
        let block = "Name: alsa_output.pci.analog-stereo\n\tActive Port: analog-output-speaker\n";
        assert!(!parse_active_port_headphones(block));
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
