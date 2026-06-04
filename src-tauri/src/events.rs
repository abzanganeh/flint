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

#[allow(dead_code)] // wired in Phase 5 token usage tracker
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

pub fn emit_directional_token(app: &AppHandle, payload: DirectionalTokenPayload) {
    let _ = app.emit("directional_token", payload);
}

pub fn emit_depth_token(app: &AppHandle, payload: DepthTokenPayload) {
    let _ = app.emit("depth_token", payload);
}

pub fn emit_clarifying_question(app: &AppHandle, payload: ClarifyingQuestionPayload) {
    let _ = app.emit("clarifying_question", payload);
}

pub fn emit_confidence_score(app: &AppHandle, payload: ConfidenceScorePayload) {
    let _ = app.emit("confidence_score", payload);
}

pub fn emit_thread_status(app: &AppHandle, payload: ThreadStatusPayload) {
    let _ = app.emit("thread_status", payload);
}

pub fn emit_failover_triggered(app: &AppHandle, payload: FailoverTriggeredPayload) {
    let _ = app.emit("failover_triggered", payload);
}

pub fn emit_primary_restored(app: &AppHandle, payload: PrimaryRestoredPayload) {
    let _ = app.emit("primary_restored", payload);
}

#[allow(dead_code)] // wired in Phase 5 token usage tracker
pub fn emit_token_usage_update(app: &AppHandle, payload: TokenUsageUpdatePayload) {
    let _ = app.emit("token_usage_update", payload);
}

pub fn emit_session_state_change(app: &AppHandle, payload: SessionStateChangePayload) {
    let _ = app.emit("session_state_change", payload);
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextTruncatedPayload {
    pub session_id: String,
}

pub fn emit_context_truncated(app: &AppHandle, payload: ContextTruncatedPayload) {
    let _ = app.emit("context_truncated", payload);
}
