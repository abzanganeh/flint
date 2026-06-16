//! Mock Interview module.
//!
//! Phase 8 — mic-only practice mode driven by the session digest questions.
//! Architecture overview:
//!   - `conductor`   — question sequencer + suggested-answer LLM thread
//!   - `mic_capture` — mic-only VAD+Whisper; emits `mock_user_transcribed`
//!   - `audio_writer`— per-turn WAV persistence (hound RIFF/WAV)
//!   - `coach`       — post-answer structured feedback LLM thread
//!   - `tts`         — platform TTS for AI-voiced questions

pub mod audio_writer;
pub mod coach;
pub mod conductor;
pub mod mic_capture;
pub mod rag;
pub mod tts;
