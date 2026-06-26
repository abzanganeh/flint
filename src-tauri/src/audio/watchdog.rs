//! Live audio-flow watchdog.
//!
//! A live session can silently capture nothing — the user picks the wrong
//! loopback/mic device, the OS revokes the stream, or (as happened in the
//! field) the overlay enters LIVE but no frames ever arrive. Without a signal,
//! the user believes they are recording an interview while Flint persists zero
//! transcript chunks and produces no summary.
//!
//! The watchdog polls the existing [`AudioAuditCounters`] after going LIVE. If
//! no chunk is accepted within the grace window it emits a `live_audio_warning`
//! so the UI can surface a prominent, actionable banner. When audio later flows
//! it emits a clearing signal so the banner disappears.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Runtime};

use crate::audio::audit::AudioAuditCounters;
use crate::events::{emit_live_audio_warning, LiveAudioWarningPayload};

/// Grace period after going LIVE before a zero-audio state is treated as a
/// fault. Whisper needs a beat to accept the first segment, and capture
/// startup is not instantaneous, so warning earlier produces false positives.
pub const WATCHDOG_GRACE: Duration = Duration::from_secs(12);

/// How often the watchdog samples the audit counters.
pub const WATCHDOG_TICK: Duration = Duration::from_secs(4);

const NO_AUDIO_MESSAGE: &str = "No audio captured yet — nothing is being recorded. \
Check that the correct loopback (interviewer) and microphone devices are selected, \
then restart the session. In phone mode, make sure the call audio is routed to Flint.";

/// What the watchdog decided to emit on a given tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioHealthSignal {
    /// No chunks accepted past the grace window — warn the user.
    NoAudio,
    /// Audio is flowing again after a prior warning — clear it.
    Recovered,
}

impl AudioHealthSignal {
    fn kind(self) -> &'static str {
        match self {
            AudioHealthSignal::NoAudio => "no_audio",
            AudioHealthSignal::Recovered => "ok",
        }
    }

    fn message(self) -> String {
        match self {
            AudioHealthSignal::NoAudio => NO_AUDIO_MESSAGE.to_string(),
            AudioHealthSignal::Recovered => String::new(),
        }
    }
}

/// Pure decision used by [`run_audio_watchdog`]. Kept side-effect free so the
/// state-transition rules can be unit tested without a Tauri runtime.
///
/// * Emits `NoAudio` exactly once, when still silent past the grace window.
/// * After a warning, emits `Recovered` exactly once when audio resumes.
pub fn evaluate_audio_health(
    accepted_total: u64,
    elapsed: Duration,
    already_warned: bool,
) -> Option<AudioHealthSignal> {
    if already_warned {
        if accepted_total > 0 {
            Some(AudioHealthSignal::Recovered)
        } else {
            None
        }
    } else if accepted_total == 0 && elapsed >= WATCHDOG_GRACE {
        Some(AudioHealthSignal::NoAudio)
    } else {
        None
    }
}

/// Poll the audit counters until aborted, emitting `live_audio_warning` events
/// when the audio-flow state changes. Runs for the lifetime of the live
/// session; the caller stores the [`tokio::task::JoinHandle`] and aborts it on
/// stop.
pub async fn run_audio_watchdog<R: Runtime>(
    app: AppHandle<R>,
    audit: Arc<AudioAuditCounters>,
    started: Instant,
) {
    let mut warned = false;
    let mut ticker = tokio::time::interval(WATCHDOG_TICK);
    // The first tick fires immediately; skip it so we never warn before the
    // grace window can plausibly have elapsed.
    ticker.tick().await;

    loop {
        ticker.tick().await;

        let snap = audit.snapshot();
        let accepted_total = snap.system.accepted + snap.mic.accepted;

        if let Some(signal) = evaluate_audio_health(accepted_total, started.elapsed(), warned) {
            match signal {
                AudioHealthSignal::NoAudio => warned = true,
                AudioHealthSignal::Recovered => warned = false,
            }
            emit_live_audio_warning(
                &app,
                LiveAudioWarningPayload {
                    kind: signal.kind().to_string(),
                    message: signal.message(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_quiet_during_grace_window() {
        assert_eq!(
            evaluate_audio_health(0, Duration::from_secs(5), false),
            None
        );
    }

    #[test]
    fn warns_once_past_grace_with_no_audio() {
        assert_eq!(
            evaluate_audio_health(0, WATCHDOG_GRACE, false),
            Some(AudioHealthSignal::NoAudio)
        );
    }

    #[test]
    fn never_warns_when_audio_present() {
        assert_eq!(evaluate_audio_health(3, WATCHDOG_GRACE * 10, false), None);
    }

    #[test]
    fn clears_after_warning_when_audio_resumes() {
        assert_eq!(
            evaluate_audio_health(1, WATCHDOG_GRACE * 2, true),
            Some(AudioHealthSignal::Recovered)
        );
    }

    #[test]
    fn stays_warned_while_still_silent() {
        assert_eq!(evaluate_audio_health(0, WATCHDOG_GRACE * 5, true), None);
    }

    #[test]
    fn signal_kinds_are_stable_contract() {
        assert_eq!(AudioHealthSignal::NoAudio.kind(), "no_audio");
        assert_eq!(AudioHealthSignal::Recovered.kind(), "ok");
        assert!(!AudioHealthSignal::NoAudio.message().is_empty());
        assert!(AudioHealthSignal::Recovered.message().is_empty());
    }
}
