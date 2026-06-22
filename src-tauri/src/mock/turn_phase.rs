//! Mock turn phase helpers — turn-level silence pause (distinct from live 600ms VAD).

/// Turn-level silence before auto-pause (mock only; live session uses 600ms VAD).
pub const MOCK_TURN_SILENCE_MS: u64 = 3000;

/// Minimum accumulated speech before turn-level pause can trigger.
pub const MOCK_MIN_SPEECH_MS: u64 = 2000;

/// One WebRTC VAD frame at 16 kHz (20 ms).
pub const VAD_FRAME_MS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockMicPhase {
    Off,
    Listening,
    Answering,
    Paused,
}

impl MockMicPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Listening => "listening",
            Self::Answering => "answering",
            Self::Paused => "paused",
        }
    }
}

/// Tracks per-turn speech accumulation for the mock pause gate.
#[derive(Debug, Clone, Default)]
pub struct TurnSpeechTracker {
    turn_speech_ms: u64,
}

impl TurnSpeechTracker {
    pub fn reset(&mut self) {
        self.turn_speech_ms = 0;
    }

    pub fn on_speech_frame(&mut self) {
        self.turn_speech_ms = self.turn_speech_ms.saturating_add(VAD_FRAME_MS as u64);
    }

    pub fn turn_speech_ms(&self) -> u64 {
        self.turn_speech_ms
    }

    /// Returns true when answering should transition to paused.
    pub fn should_pause(&self, ms_since_last_speech: u64) -> bool {
        self.turn_speech_ms >= MOCK_MIN_SPEECH_MS && ms_since_last_speech >= MOCK_TURN_SILENCE_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_requires_min_speech_and_silence() {
        let mut tracker = TurnSpeechTracker::default();
        assert!(!tracker.should_pause(MOCK_TURN_SILENCE_MS));

        for _ in 0..(MOCK_MIN_SPEECH_MS / VAD_FRAME_MS as u64) {
            tracker.on_speech_frame();
        }
        assert!(!tracker.should_pause(MOCK_TURN_SILENCE_MS - 1));
        assert!(tracker.should_pause(MOCK_TURN_SILENCE_MS));
    }

    #[test]
    fn short_answer_never_auto_pauses() {
        let mut tracker = TurnSpeechTracker::default();
        tracker.on_speech_frame();
        assert!(!tracker.should_pause(10_000));
    }

    #[test]
    fn phase_str_matches_event_contract() {
        assert_eq!(MockMicPhase::Listening.as_str(), "listening");
        assert_eq!(MockMicPhase::Answering.as_str(), "answering");
        assert_eq!(MockMicPhase::Paused.as_str(), "paused");
    }
}
