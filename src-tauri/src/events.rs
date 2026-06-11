use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptionChunkPayload {
    pub text: String,
    pub speaker: String,
    pub timestamp: i64,
}

/// Emitted at the start of every orchestrator turn (live and rehearsal).
/// The frontend uses this as the turn boundary: archive the previous
/// answer card and start a fresh one headed by `question`.
#[derive(Debug, Clone, Serialize)]
pub struct TurnStartedPayload {
    pub question: String,
    pub turn: usize,
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

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct TokenUsageUpdatePayload {
    pub input: u64,
    pub output: u64,
    pub total: u64,
    pub cost_estimate: f64,
    /// Categorises the spend so the UI can break it down per activity.
    /// Values: `"rehearsal_turn"`, `"live_turn"`, `"research_chat"`, `"digest"`, `"pre_warm"`.
    pub usage_category: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionStateChangePayload {
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextTruncatedPayload {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagChunkPayload {
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagChunksUpdatePayload {
    pub chunks: Vec<RagChunkPayload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseMetadataPayload {
    pub pre_prepared: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayVisibilityPayload {
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HotkeyTriggerPayload {
    pub action: String,
}

/// Phase 7.4 — emitted on every transition through the cost-cap status
/// machine. `status` is one of `ok` / `warning_80` / `reached`. `suspended`
/// reflects whether the orchestrator is currently blocked from dispatching
/// new turns.
#[derive(Debug, Clone, Serialize)]
pub struct CostCapStatusPayload {
    pub status: &'static str,
    pub suspended: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_estimate_usd: f64,
    pub max_total_tokens: Option<u64>,
    pub max_cost_estimate_usd: Option<f64>,
    /// Fraction of the strictest active cap consumed (`0.0..=1.0+`), or
    /// `null` if no cap is configured.
    pub fraction_used: Option<f64>,
}

/// Phase 7.4 — fired exactly once when the orchestrator refuses a turn
/// because the cap is at 100%. The frontend should surface a non-dismissible
/// banner and disable the trigger-response control until the user lifts
/// the suspension via the cost-cap settings.
#[derive(Debug, Clone, Serialize)]
pub struct InferenceSuspendedPayload {
    pub reason: &'static str,
    pub total_tokens: u64,
    pub cost_estimate_usd: f64,
}

pub fn emit_transcription_chunk<R: Runtime>(
    app: &AppHandle<R>,
    payload: TranscriptionChunkPayload,
) {
    let _ = app.emit("transcription_chunk", payload);
}

pub fn emit_turn_started<R: Runtime>(app: &AppHandle<R>, payload: TurnStartedPayload) {
    let _ = app.emit("turn_started", payload);
}

pub fn emit_directional_token<R: Runtime>(app: &AppHandle<R>, payload: DirectionalTokenPayload) {
    let _ = app.emit("directional_token", payload);
}

pub fn emit_depth_token<R: Runtime>(app: &AppHandle<R>, payload: DepthTokenPayload) {
    let _ = app.emit("depth_token", payload);
}

pub fn emit_clarifying_question<R: Runtime>(
    app: &AppHandle<R>,
    payload: ClarifyingQuestionPayload,
) {
    let _ = app.emit("clarifying_question", payload);
}

pub fn emit_confidence_score<R: Runtime>(app: &AppHandle<R>, payload: ConfidenceScorePayload) {
    let _ = app.emit("confidence_score", payload);
}

pub fn emit_thread_status<R: Runtime>(app: &AppHandle<R>, payload: ThreadStatusPayload) {
    let _ = app.emit("thread_status", payload);
}

pub fn emit_failover_triggered<R: Runtime>(app: &AppHandle<R>, payload: FailoverTriggeredPayload) {
    let _ = app.emit("failover_triggered", payload);
}

pub fn emit_primary_restored<R: Runtime>(app: &AppHandle<R>, payload: PrimaryRestoredPayload) {
    let _ = app.emit("primary_restored", payload);
}

pub fn emit_token_usage_update<R: Runtime>(app: &AppHandle<R>, payload: TokenUsageUpdatePayload) {
    let _ = app.emit("token_usage_update", payload);
}

pub fn emit_cost_cap_status<R: Runtime>(app: &AppHandle<R>, payload: CostCapStatusPayload) {
    let _ = app.emit("cost_cap_status", payload);
}

pub fn emit_inference_suspended<R: Runtime>(
    app: &AppHandle<R>,
    payload: InferenceSuspendedPayload,
) {
    let _ = app.emit("inference_suspended", payload);
}

pub fn emit_rag_chunks_update<R: Runtime>(app: &AppHandle<R>, payload: RagChunksUpdatePayload) {
    let _ = app.emit("rag_chunks_update", payload);
}

pub fn emit_response_metadata<R: Runtime>(app: &AppHandle<R>, payload: ResponseMetadataPayload) {
    let _ = app.emit("response_metadata", payload);
}

pub fn emit_overlay_visibility<R: Runtime>(app: &AppHandle<R>, payload: OverlayVisibilityPayload) {
    let _ = app.emit("overlay_visibility", payload);
}

pub fn emit_hotkey_trigger<R: Runtime>(app: &AppHandle<R>, payload: HotkeyTriggerPayload) {
    let _ = app.emit("hotkey_trigger", payload);
}

pub fn emit_session_state_change<R: Runtime>(
    app: &AppHandle<R>,
    payload: SessionStateChangePayload,
) {
    let _ = app.emit("session_state_change", payload);
}

pub fn emit_context_truncated<R: Runtime>(app: &AppHandle<R>, payload: ContextTruncatedPayload) {
    let _ = app.emit("context_truncated", payload);
}

// ── Mock Interview events ─────────────────────────────────────────────────────

/// Emitted when the conductor moves to a new question (text shown in transcript
/// immediately; TTS speaks it in the background).
#[derive(Debug, Clone, Serialize)]
pub struct MockQuestionStartedPayload {
    pub question: String,
    pub turn_n: u32,
    pub total_questions: u32,
}

/// Emitted when the user's mic VAD chunk has been transcribed.
#[derive(Debug, Clone, Serialize)]
pub struct MockUserTranscribedPayload {
    pub turn_n: u32,
    pub text: String,
    /// Absolute local path to the WAV file, or empty string if audio was not saved.
    pub audio_path: String,
}

/// Streaming suggested-answer token (single merged panel, replaces directional+depth).
#[derive(Debug, Clone, Serialize)]
pub struct MockSuggestedTokenPayload {
    pub token: String,
}

/// Structured coach feedback emitted once after coach LLM completes.
#[derive(Debug, Clone, Serialize)]
pub struct MockCoachFeedbackPayload {
    pub turn_n: u32,
    /// Serialised `CoachFeedback` JSON for the frontend to parse.
    pub coach_json: String,
    pub score: u8,
}

/// Emitted when the mock interview ends (all questions answered or user exits).
#[derive(Debug, Clone, Serialize)]
pub struct MockEndedPayload {
    pub session_id: String,
    pub turns_completed: u32,
}

pub fn emit_mock_question_started<R: Runtime>(
    app: &AppHandle<R>,
    payload: MockQuestionStartedPayload,
) {
    let _ = app.emit("mock_question_started", payload);
}

pub fn emit_mock_user_transcribed<R: Runtime>(
    app: &AppHandle<R>,
    payload: MockUserTranscribedPayload,
) {
    let _ = app.emit("mock_user_transcribed", payload);
}

pub fn emit_mock_suggested_token<R: Runtime>(
    app: &AppHandle<R>,
    payload: MockSuggestedTokenPayload,
) {
    let _ = app.emit("mock_suggested_token", payload);
}

pub fn emit_mock_coach_feedback<R: Runtime>(app: &AppHandle<R>, payload: MockCoachFeedbackPayload) {
    let _ = app.emit("mock_coach_feedback", payload);
}

pub fn emit_mock_ended<R: Runtime>(app: &AppHandle<R>, payload: MockEndedPayload) {
    let _ = app.emit("mock_ended", payload);
}
