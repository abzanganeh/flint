//! Deepgram cloud STT adapter — implements [`TranscriptionProvider`] against
//! Deepgram's pre-recorded `/v1/listen` endpoint, one call per closed VAD chunk.
//!
//! Not a streaming client: this is deliberately batch-per-chunk to keep the
//! existing RNNoise -> VAD chunk pipeline unchanged (per the milestone scope
//! decision). Deepgram receives raw 16 kHz mono PCM16-LE bytes and returns
//! the transcript + word-level timings, which we adapt into the same
//! [`TranscriptionResult`] shape the local Whisper path produces.
//!
//! ## `avg_logprob` handling
//!
//! Whisper reports a mean per-token log-probability that other subsystems
//! (mic-quality monitoring) key off. Deepgram reports a 0..1 confidence
//! score — a different scale, not a log-probability. We deliberately leave
//! [`TranscriptionResult::avg_logprob`] as `None` here rather than
//! silently mapping across scales; mic-quality monitoring simply falls
//! back to its no-signal path when Deepgram is the active provider.
//!
//! ## `rolling_context` handling
//!
//! Deepgram does not accept an arbitrary prompt to seed its decoder the
//! way Whisper does. The rolling ~40-word context is ignored here.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::warn;

use crate::audio::capture::AudioSource;
use crate::audio::vad::VadChunk;
use crate::transcription::engine::{TranscriptionResult, WordTimestamp};
use crate::transcription::provider::TranscriptionProvider;
use crate::transcription::sanitizer::{
    collapse_repeated_ngrams, is_known_hallucination, validate_segment,
};

pub const DEEPGRAM_DEFAULT_BASE_URL: &str = "https://api.deepgram.com";
pub const DEEPGRAM_DEFAULT_MODEL: &str = "nova-3";
/// Deliberately short. A slow network call must never stall the pipeline —
/// on timeout the router falls back to local Whisper.
pub const DEEPGRAM_DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// Runtime configuration for [`DeepgramTranscriptionProvider`].
///
/// `base_url` is public so tests can point at a `wiremock::MockServer`.
#[derive(Debug, Clone)]
pub struct DeepgramConfig {
    pub base_url: String,
    pub model: String,
    pub request_timeout: Duration,
}

impl Default for DeepgramConfig {
    fn default() -> Self {
        Self {
            base_url: DEEPGRAM_DEFAULT_BASE_URL.to_string(),
            model: DEEPGRAM_DEFAULT_MODEL.to_string(),
            request_timeout: DEEPGRAM_DEFAULT_TIMEOUT,
        }
    }
}

pub struct DeepgramTranscriptionProvider {
    api_key: SecretString,
    client: reqwest::Client,
    config: DeepgramConfig,
}

// ──────────────────────────────────────────────────────────────────────────
// Deepgram response schema (subset)
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DeepgramResponse {
    results: DeepgramResults,
}

#[derive(Debug, Deserialize)]
struct DeepgramResults {
    channels: Vec<DeepgramChannel>,
}

#[derive(Debug, Deserialize)]
struct DeepgramChannel {
    alternatives: Vec<DeepgramAlternative>,
}

#[derive(Debug, Deserialize)]
struct DeepgramAlternative {
    transcript: String,
    #[serde(default)]
    words: Vec<DeepgramWord>,
}

#[derive(Debug, Deserialize)]
struct DeepgramWord {
    word: String,
    start: f32,
    end: f32,
}

impl DeepgramTranscriptionProvider {
    pub fn new(api_key: SecretString, config: DeepgramConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("Failed to build Deepgram HTTP client")?;
        Ok(Self {
            api_key,
            client,
            config,
        })
    }

    fn build_url(&self) -> String {
        format!(
            "{}/v1/listen?model={}&language=en&smart_format=true&punctuate=true&encoding=linear16&sample_rate=16000&channels=1",
            self.config.base_url.trim_end_matches('/'),
            self.config.model,
        )
    }

    /// Convert Whisper-shaped normalized float samples (`-1.0..=1.0`) into
    /// little-endian PCM16 bytes as required by Deepgram's `linear16` encoding.
    fn samples_to_pcm16_le(samples: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let clipped = s.clamp(-1.0, 1.0);
            let i = (clipped * i16::MAX as f32).round() as i16;
            out.extend_from_slice(&i.to_le_bytes());
        }
        out
    }

    fn parse_response(
        body: &str,
        chunk_source: AudioSource,
        chunk_duration_ms: u32,
    ) -> Result<Option<TranscriptionResult>> {
        let parsed: DeepgramResponse =
            serde_json::from_str(body).context("Deepgram response JSON parse failed")?;

        let Some(channel) = parsed.results.channels.into_iter().next() else {
            return Ok(None);
        };
        let Some(alt) = channel.alternatives.into_iter().next() else {
            return Ok(None);
        };

        let transcript = alt.transcript.trim().to_string();
        if transcript.is_empty() {
            return Ok(None);
        }
        if is_known_hallucination(&transcript) {
            tracing::debug!(
                source = ?chunk_source,
                "Deepgram result discarded — known hallucination string"
            );
            return Ok(None);
        }
        let collapsed = collapse_repeated_ngrams(&transcript);
        if !validate_segment(&collapsed, chunk_duration_ms) {
            tracing::debug!(
                source = ?chunk_source,
                duration_ms = chunk_duration_ms,
                "Deepgram result discarded — implausible word/sec ratio"
            );
            return Ok(None);
        }

        let word_timestamps = alt
            .words
            .into_iter()
            .map(|w| {
                let start_ms = (w.start.max(0.0) * 1000.0).round() as u32;
                let end_ms = (w.end.max(0.0) * 1000.0).round() as u32;
                WordTimestamp {
                    word: w.word,
                    start_ms,
                    end_ms: end_ms.max(start_ms),
                }
            })
            .collect();

        Ok(Some(TranscriptionResult {
            text: collapsed,
            source: chunk_source,
            word_timestamps,
            avg_logprob: None,
        }))
    }
}

#[async_trait]
impl TranscriptionProvider for DeepgramTranscriptionProvider {
    async fn transcribe(
        &self,
        chunk: VadChunk,
        _rolling_context: String,
    ) -> Result<Option<TranscriptionResult>> {
        if chunk.samples.is_empty() {
            return Ok(None);
        }

        let body = Self::samples_to_pcm16_le(&chunk.samples);
        let url = self.build_url();

        let response = self
            .client
            .post(url)
            .header(
                AUTHORIZATION,
                format!("Token {}", self.api_key.expose_secret()),
            )
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .context("Deepgram request failed")?;

        let status = response.status();
        if !status.is_success() {
            let snippet: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            warn!(status = %status, "Deepgram non-2xx response");
            bail!("Deepgram API error {status}: {snippet}");
        }

        let text = response.text().await.context("Deepgram body read failed")?;

        Self::parse_response(&text, chunk.source, chunk.duration_ms)
    }

    fn name(&self) -> &str {
        "deepgram"
    }

    fn is_available(&self) -> bool {
        !self.api_key.expose_secret().is_empty()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::capture::AudioSource;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn chunk_with_speech(duration_ms: u32) -> VadChunk {
        // 16 kHz mono, non-silent samples so `samples.is_empty()` guard doesn't
        // short-circuit. Content doesn't matter — the mock server ignores body.
        let samples = vec![0.1_f32; (duration_ms as usize) * 16];
        VadChunk {
            samples,
            source: AudioSource::System,
            duration_ms,
        }
    }

    fn provider(base_url: String, timeout: Duration) -> DeepgramTranscriptionProvider {
        DeepgramTranscriptionProvider::new(
            SecretString::new("test-key".into()),
            DeepgramConfig {
                base_url,
                model: "nova-3".to_string(),
                request_timeout: timeout,
            },
        )
        .expect("provider builds")
    }

    #[test]
    fn samples_to_pcm16_round_trips_extremes() {
        let bytes = DeepgramTranscriptionProvider::samples_to_pcm16_le(&[1.0, -1.0, 0.0]);
        assert_eq!(bytes.len(), 6);
        // 1.0 -> i16::MAX, -1.0 -> -i16::MAX (rounded), 0.0 -> 0
        let a = i16::from_le_bytes([bytes[0], bytes[1]]);
        let b = i16::from_le_bytes([bytes[2], bytes[3]]);
        let c = i16::from_le_bytes([bytes[4], bytes[5]]);
        assert_eq!(a, i16::MAX);
        assert_eq!(b, -i16::MAX); // (-32768 -> -32767 after round)
        assert_eq!(c, 0);
    }

    #[test]
    fn samples_to_pcm16_clamps_out_of_range_values() {
        let bytes = DeepgramTranscriptionProvider::samples_to_pcm16_le(&[2.0, -2.0]);
        let a = i16::from_le_bytes([bytes[0], bytes[1]]);
        let b = i16::from_le_bytes([bytes[2], bytes[3]]);
        assert_eq!(a, i16::MAX);
        assert_eq!(b, -i16::MAX);
    }

    #[test]
    fn build_url_contains_required_query_params() {
        let p = provider("https://example.test".to_string(), Duration::from_secs(1));
        let url = p.build_url();
        assert!(url.contains("/v1/listen"));
        assert!(url.contains("model=nova-3"));
        assert!(url.contains("encoding=linear16"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("channels=1"));
    }

    #[test]
    fn parse_response_happy_path_maps_words_to_ms() {
        let body = r#"{
          "results": {
            "channels": [{
              "alternatives": [{
                "transcript": "hello world",
                "words": [
                  {"word": "hello", "start": 0.0, "end": 0.5, "confidence": 0.99},
                  {"word": "world", "start": 0.6, "end": 1.1, "confidence": 0.97}
                ]
              }]
            }]
          }
        }"#;
        let result = DeepgramTranscriptionProvider::parse_response(body, AudioSource::System, 1200)
            .expect("parses")
            .expect("non-empty");
        assert_eq!(result.text, "hello world");
        assert_eq!(result.source, AudioSource::System);
        assert_eq!(result.word_timestamps.len(), 2);
        assert_eq!(result.word_timestamps[0].start_ms, 0);
        assert_eq!(result.word_timestamps[0].end_ms, 500);
        assert_eq!(result.word_timestamps[1].start_ms, 600);
        assert_eq!(result.word_timestamps[1].end_ms, 1100);
        assert!(result.avg_logprob.is_none());
    }

    #[test]
    fn parse_response_empty_transcript_returns_none() {
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"","words":[]}]}]}}"#;
        let result =
            DeepgramTranscriptionProvider::parse_response(body, AudioSource::System, 500).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_response_missing_channels_returns_none() {
        let body = r#"{"results":{"channels":[]}}"#;
        let result =
            DeepgramTranscriptionProvider::parse_response(body, AudioSource::System, 500).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_response_malformed_json_errors() {
        let body = r#"{not valid json"#;
        let err = DeepgramTranscriptionProvider::parse_response(body, AudioSource::System, 500)
            .expect_err("malformed JSON must fail");
        assert!(err
            .to_string()
            .contains("Deepgram response JSON parse failed"));
    }

    #[test]
    fn is_available_reflects_key_presence() {
        let empty = DeepgramTranscriptionProvider::new(
            SecretString::new(String::new()),
            DeepgramConfig::default(),
        )
        .unwrap();
        assert!(!empty.is_available());

        let keyed = DeepgramTranscriptionProvider::new(
            SecretString::new("k".into()),
            DeepgramConfig::default(),
        )
        .unwrap();
        assert!(keyed.is_available());
    }

    // ── wiremock-driven HTTP integration ─────────────────────────────────

    #[tokio::test]
    async fn happy_path_returns_transcript() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .and(header("authorization", "Token test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"results":{"channels":[{"alternatives":[{
                    "transcript":"tell me about your experience",
                    "words":[]
                }]}]}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let p = provider(server.uri(), Duration::from_secs(2));
        let out = p
            .transcribe(chunk_with_speech(1500), String::new())
            .await
            .unwrap()
            .expect("non-empty");
        assert_eq!(out.text, "tell me about your experience");
    }

    #[tokio::test]
    async fn empty_transcript_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"results":{"channels":[{"alternatives":[{"transcript":"","words":[]}]}]}}"#,
            ))
            .mount(&server)
            .await;

        let p = provider(server.uri(), Duration::from_secs(2));
        let out = p
            .transcribe(chunk_with_speech(500), String::new())
            .await
            .unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn unauthorized_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"err":"invalid key"}"#))
            .mount(&server)
            .await;

        let p = provider(server.uri(), Duration::from_secs(2));
        let err = p
            .transcribe(chunk_with_speech(500), String::new())
            .await
            .expect_err("401 must surface as Err for the router to fail over");
        assert!(err.to_string().contains("401"));
    }

    #[tokio::test]
    async fn server_error_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let p = provider(server.uri(), Duration::from_secs(2));
        let err = p
            .transcribe(chunk_with_speech(500), String::new())
            .await
            .expect_err("5xx must surface as Err");
        assert!(err.to_string().contains("503"));
    }

    #[tokio::test]
    async fn malformed_body_from_server_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<not json>"))
            .mount(&server)
            .await;

        let p = provider(server.uri(), Duration::from_secs(2));
        let err = p
            .transcribe(chunk_with_speech(500), String::new())
            .await
            .expect_err("malformed body must error");
        assert!(err.to_string().contains("JSON parse failed"));
    }

    #[tokio::test]
    async fn timeout_surfaces_as_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_string(r#"{"results":{"channels":[]}}"#),
            )
            .mount(&server)
            .await;

        let p = provider(server.uri(), Duration::from_millis(50));
        let err = p
            .transcribe(chunk_with_speech(500), String::new())
            .await
            .expect_err("timeout must surface for router failover");
        assert!(
            err.to_string().to_lowercase().contains("timeout")
                || err.to_string().contains("Deepgram request failed"),
        );
    }

    #[tokio::test]
    async fn empty_samples_short_circuits_without_hitting_server() {
        // If we ever fired the request the mock server would return an
        // unexpected-hit failure at drop. Absence of that is the check.
        let server = MockServer::start().await;
        let p = provider(server.uri(), Duration::from_secs(1));
        let empty_chunk = VadChunk {
            samples: Vec::new(),
            source: AudioSource::System,
            duration_ms: 0,
        };
        let out = p.transcribe(empty_chunk, String::new()).await.unwrap();
        assert!(out.is_none());
    }
}
