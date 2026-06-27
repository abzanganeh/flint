//! Rolling transcript context for Whisper `initial_prompt` carry-over.

/// Maximum words retained per channel for `transcribe_with_context`.
pub const ROLLING_CONTEXT_WORD_LIMIT: usize = 40;

/// Per-channel rolling word buffer fed into Whisper as soft prior text.
#[derive(Debug, Default, Clone)]
pub struct RollingTranscriptContext {
    words: Vec<String>,
}

impl RollingTranscriptContext {
    pub fn append(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        for word in trimmed.split_whitespace() {
            self.words.push(word.to_string());
        }
        if self.words.len() > ROLLING_CONTEXT_WORD_LIMIT {
            let keep_from = self.words.len() - ROLLING_CONTEXT_WORD_LIMIT;
            self.words.drain(..keep_from);
        }
    }

    pub fn clear(&mut self) {
        self.words.clear();
    }

    pub fn as_str(&self) -> String {
        self.words.join(" ")
    }

    pub fn word_count(&self) -> usize {
        self.words.len()
    }
}

/// Rolling context for System and Microphone capture channels.
#[derive(Debug, Default)]
pub struct ChannelRollingContexts {
    pub system: RollingTranscriptContext,
    pub mic: RollingTranscriptContext,
}

impl ChannelRollingContexts {
    pub fn context_for(&self, source: crate::audio::capture::AudioSource) -> String {
        match source {
            crate::audio::capture::AudioSource::System => self.system.as_str(),
            crate::audio::capture::AudioSource::Microphone => self.mic.as_str(),
        }
    }

    pub fn append(&mut self, source: crate::audio::capture::AudioSource, text: &str) {
        match source {
            crate::audio::capture::AudioSource::System => self.system.append(text),
            crate::audio::capture::AudioSource::Microphone => self.mic.append(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_keeps_last_40_words() {
        let mut ctx = RollingTranscriptContext::default();
        for i in 0..60 {
            ctx.append(&format!("word{i}"));
        }
        assert_eq!(ctx.word_count(), ROLLING_CONTEXT_WORD_LIMIT);
        let text = ctx.as_str();
        assert!(text.contains("word59"));
        assert!(!text.contains("word0"));
    }

    #[test]
    fn append_skips_empty_and_joins_phrases() {
        let mut ctx = RollingTranscriptContext::default();
        ctx.append("Fisher Investors");
        ctx.append("  ");
        ctx.append("IAM fiduciary");
        assert_eq!(ctx.as_str(), "Fisher Investors IAM fiduciary");
    }

    #[test]
    fn clear_resets_buffer() {
        let mut ctx = RollingTranscriptContext::default();
        ctx.append("alpha bravo charlie");
        ctx.clear();
        assert_eq!(ctx.word_count(), 0);
        assert!(ctx.as_str().is_empty());
    }

    #[test]
    fn channel_contexts_are_independent() {
        let mut channels = ChannelRollingContexts::default();
        channels.append(
            crate::audio::capture::AudioSource::System,
            "interviewer asks",
        );
        channels.append(
            crate::audio::capture::AudioSource::Microphone,
            "user answers",
        );
        assert_eq!(
            channels.context_for(crate::audio::capture::AudioSource::System),
            "interviewer asks"
        );
        assert_eq!(
            channels.context_for(crate::audio::capture::AudioSource::Microphone),
            "user answers"
        );
    }
}
