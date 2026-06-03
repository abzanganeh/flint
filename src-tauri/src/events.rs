#![allow(dead_code)] // most emit helpers wired in later phases

use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptionChunkPayload {
    pub text: String,
    pub speaker: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirectionalTokenPayload {
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DepthTokenPayload {
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClarifyingQuestionPayload {
    pub question: String,
    pub rank: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfidenceScorePayload {
    pub level: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadStatusPayload {
    pub thread: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailoverTriggeredPayload {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrimaryRestoredPayload {
    pub provider: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenUsageUpdatePayload {
    pub input: u64,
    pub output: u64,
    pub total: u64,
    pub cost_estimate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionStateChangePayload {
    pub state: String,
}

pub fn emit_transcription_chunk(app: &AppHandle, payload: TranscriptionChunkPayload) {
    let _ = app.emit("transcription_chunk", payload);
}

pub fn emit_directional_token(_app: &AppHandle, _payload: DirectionalTokenPayload) {}

pub fn emit_depth_token(_app: &AppHandle, _payload: DepthTokenPayload) {}

pub fn emit_clarifying_question(_app: &AppHandle, _payload: ClarifyingQuestionPayload) {}

pub fn emit_confidence_score(_app: &AppHandle, _payload: ConfidenceScorePayload) {}

pub fn emit_thread_status(app: &AppHandle, payload: ThreadStatusPayload) {
    let _ = app.emit("thread_status", payload);
}

pub fn emit_failover_triggered(_app: &AppHandle, _payload: FailoverTriggeredPayload) {}

pub fn emit_primary_restored(_app: &AppHandle, _payload: PrimaryRestoredPayload) {}

pub fn emit_token_usage_update(_app: &AppHandle, _payload: TokenUsageUpdatePayload) {}

pub fn emit_session_state_change(app: &AppHandle, payload: SessionStateChangePayload) {
    let _ = app.emit("session_state_change", payload);
}
