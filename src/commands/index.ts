import { invoke } from "@tauri-apps/api/core";

import type { MockStudyMode } from "../events";

export type UserPlan = "free" | "premium";

export interface UserDto {
  id: string;
  email: string;
  plan: UserPlan;
}

export type HealthCheckName =
  | "microphone_access"
  | "system_audio_loopback"
  | "rnnoise_preprocessing"
  | "whisper_model"
  | "stealth_api"
  | "primary_llm"
  | "ollama_availability"
  | "os_keychain"
  | "local_sqlite"
  | "supabase_connection"
  | "global_hotkey"
  | "panic_hotkey"
  | "echo_cancellation";

export type CheckStatus = "pass" | "warn" | "fail";

export interface HealthCheckResultDto {
  check: HealthCheckName;
  status: CheckStatus;
  message: string;
  fixInstruction: string | null;
}

export interface LlmConfigDto {
  directional: string;
  depth: string;
  fallback: string | null;
  cloudRecommended: boolean;
}

export interface HardwareProfileDto {
  tier: number;
  cpuCores: number;
  ramGb: number;
  hasGpu: boolean;
  gpuVramGb: number | null;
  os: string;
  recommendedWhisperModel: string;
  recommendedLlmConfig: LlmConfigDto;
}

export const getHardwareProfile = (): Promise<HardwareProfileDto> =>
  invoke<HardwareProfileDto>("get_hardware_profile");

export const runHealthCheck = (): Promise<HealthCheckResultDto[]> =>
  invoke<HealthCheckResultDto[]>("run_health_check");

export const getLegalConsentAccepted = (): Promise<boolean> =>
  invoke<boolean>("get_legal_consent_accepted");

export const setLegalConsentAccepted = (): Promise<void> =>
  invoke<void>("set_legal_consent_accepted");

export const signup = (email: string, password: string): Promise<void> =>
  invoke<void>("signup", { email, password });

export const setSessionState = (state: string): Promise<void> =>
  invoke<void>("set_session_state", { state });

export const login = (email: string, password: string): Promise<void> =>
  invoke<void>("login", { email, password });

export const startGoogleOAuth = (): Promise<void> =>
  invoke<void>("start_google_oauth");

export const cancelGoogleOAuth = (): Promise<void> =>
  invoke<void>("cancel_google_oauth");

export const logout = (): Promise<void> => invoke<void>("logout");

export const getCurrentUser = (): Promise<UserDto> =>
  invoke<UserDto>("get_current_user");

const liveStartInflight = new Map<string, Promise<void>>();

/** Dedupe concurrent starts (React StrictMode double-mount in dev). */
export const startSession = (sessionId: string): Promise<void> => {
  const existing = liveStartInflight.get(sessionId);
  if (existing) return existing;

  let resolve!: () => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  liveStartInflight.set(sessionId, promise);

  void invoke<void>("start_session", { sessionId })
    .then(resolve, reject)
    .finally(() => {
      liveStartInflight.delete(sessionId);
    });

  return promise;
};

export const stopSession = (): Promise<void> => invoke<void>("stop_session");

/** Manual question boundary — Ctrl+Q during live sessions. */
export const signalQuestionEnded = (sessionId: string): Promise<void> =>
  invoke<void>("signal_question_ended", { sessionId });

export const assignSpeaker = (sessionId: string, speakerId: number): Promise<void> =>
  invoke<void>("assign_speaker", { sessionId, speakerId });

/** M13 S4 — manual speaker override for a previously emitted chunk. */
export const relabelTranscriptChunk = (
  chunkId: string,
  newSpeaker: "System" | "Microphone",
): Promise<void> =>
  invoke<void>("relabel_transcript_chunk", { chunkId, newSpeaker });

/** Manual turn: rehearsal uses `run_rehearsal_turn`; live uses `trigger_response`. */
export const triggerResponse = async (
  question: string,
  sessionId: string,
  rephrase?: boolean,
): Promise<void> => {
  const snapshot = await getSessionSnapshot();
  if (snapshot.state === "REHEARSING") {
    return runRehearsalTurn(sessionId, question, rephrase);
  }
  return invoke<void>("trigger_response", {
    question,
    sessionId,
    rephrase: rephrase ?? null,
  });
};

export const cancelInference = (): Promise<void> =>
  invoke<void>("cancel_inference");

export const panicHideOverlay = (): Promise<boolean> =>
  invoke<boolean>("panic_hide_overlay");

export const getRehearsalCompleted = (): Promise<boolean> =>
  invoke<boolean>("get_rehearsal_completed");

export const runRehearsalTurn = (
  sessionId: string,
  question: string,
  rephrase?: boolean,
): Promise<void> =>
  invoke<void>("run_rehearsal_turn", {
    sessionId,
    question,
    rephrase: rephrase ?? null,
  });

export const completeRehearsal = (sessionId: string): Promise<void> =>
  invoke<void>("complete_rehearsal", { sessionId });

/** Return to Session Design to edit pasted context (incl. company intel). */
export const returnToSessionDesign = (
  sessionId: string,
): Promise<SessionSnapshotDto> =>
  invoke<SessionSnapshotDto>("return_to_session_design", { sessionId });

export const rephraseResponse = (
  question: string,
  sessionId: string,
): Promise<void> => triggerResponse(question, sessionId, true);

export const copyTextToClipboard = (text: string): Promise<void> =>
  invoke<void>("copy_text_to_clipboard", { text });

export const switchProvider = (name: PrimaryLlmProvider): Promise<void> =>
  invoke<void>("switch_provider", { name });

export const getPreferredPrimaryProvider = (): Promise<string | null> =>
  invoke<string | null>("get_preferred_primary_provider");

export interface ConfiguredProviderDto {
  name: string;
  hasKey: boolean;
  isReachable: boolean;
}

export const getProviderPriority = (): Promise<string[]> =>
  invoke<string[]>("get_provider_priority");

export const setProviderPriority = (order: string[]): Promise<void> =>
  invoke<void>("set_provider_priority", { order });

export const getConfiguredProviders = (): Promise<ConfiguredProviderDto[]> =>
  invoke<ConfiguredProviderDto[]>("get_configured_providers");

// ──────────────────────────────────────────────────────────────────────────────
// Session design commands (Phase 2)
// ──────────────────────────────────────────────────────────────────────────────

export interface SessionConfigDto {
  name: string;
  /** "interview" | "meeting" | "presentation" | "negotiation" */
  sessionType: string;
  domain: string;
  /**
   * When true the interviewer is on a phone call near the laptop.
   * Flint captures both channels from the microphone and skips
   * the system audio loopback calibration phase.
   */
  phoneCallMode?: boolean;
}

export interface CompanyIntelDto {
  mission: string;
  values: string[];
  cultureNotes: string;
}

export interface SmartResumeImportDto {
  sessionName: string;
  sessionType: string;
  domain: string;
  jdText: string;
  resumeSummary: string;
  smartResumeSessionId: string;
  exportVersion: number;
  /** Present when Smart Resume extracted company signals from the JD. */
  companyIntel?: CompanyIntelDto;
}

export const importFromSmartResume = (
  token: string,
): Promise<SmartResumeImportDto> =>
  invoke<SmartResumeImportDto>("import_from_smart_resume", { token });

/**
 * Return and clear the cold-start import token stored by Rust before the
 * WebView mounted. Returns null if Flint was opened without a deep link, or
 * after the first call. Used in bootstrap; warm-path uses the event listener.
 */
export const getPendingImportToken = (): Promise<string | null> =>
  invoke<string | null>("get_pending_import_token");

export interface DigestDto {
  role: string;
  company: string;
  domain: string;
  keySkills: string[];
  seniority: string;
  likelyQuestions: string[];
  topicsToAvoid: string[];
}

/** Structured Session Design context fields (Phase 5.5.1). */
export interface SessionContextFields {
  jobDescription: string;
  profile: string;
  companyOverview: string;
  leadershipPrinciples: string;
  roleExpectations: string;
  technicalPrep: string;
  strategyNotes: string;
  /** `natural` | `polished` — how mock coach judges delivery. */
  speakingStyle: string;
  /** Comma-separated domain terms for Whisper (e.g. RBAC, OIDC). */
  sessionVocabulary: string;
}

export interface SessionSnapshotDto {
  sessionId: string | null;
  state: string;
  digest: DigestDto | null;
  name?: string;
  sessionType?: string;
  domain?: string;
  /** Assembled RAG blob — kept for backward compat. */
  contextText?: string;
  /** Structured fields (Phase 5.5.1). Present from CONFIGURING onward. */
  contextFields?: SessionContextFields;
  /** True when the session was created with phone-call mode enabled. */
  phoneCallMode?: boolean;
}

/** Create a new session. Returns the session UUID string. */
export const createSession = (config: SessionConfigDto): Promise<string> =>
  invoke<string>("create_session", { config });

/** Chunk, embed, and ingest context text; extract the digest. */
export const ingestContext = (sessionId: string, text: string): Promise<void> =>
  invoke<void>("ingest_context", { sessionId, text });

/**
 * Ingest structured Session Design fields (Phase 5.5.1).
 *
 * Validates `jobDescription` and `profile` are non-empty on the Rust side,
 * assembles a labelled RAG blob, stores each field in its own SQLite column,
 * then runs the full embed → digest pipeline. Use this instead of `ingestContext`
 * for all v1.5 sessions.
 */
export const ingestStructuredContext = (
  sessionId: string,
  fields: SessionContextFields,
): Promise<void> =>
  invoke<void>("ingest_structured_context", { sessionId, fields });

/**
 * Load persisted structured context fields for a session.
 *
 * All fields default to empty string for sessions created before v6. Check
 * `jobDescription.length > 0` to detect whether structured fields were stored.
 */
export const getSessionContextFields = (sessionId: string): Promise<SessionContextFields> =>
  invoke<SessionContextFields>("get_session_context_fields", { sessionId });

/** Save session vocabulary from Rehearsal (no re-ingest). Updates Whisper STT bias. */
export const updateSessionVocabulary = (
  sessionId: string,
  sessionVocabulary: string,
): Promise<void> =>
  invoke<void>("update_session_vocabulary", { sessionId, sessionVocabulary });

/** Discard in-progress session setup and return the state machine to IDLE. */
export const abandonSessionDraft = (): Promise<void> =>
  invoke<void>("abandon_session_draft");

/** Accept the (possibly edited) digest and trigger pre-warming. */
export const confirmDigest = (sessionId: string, digest: DigestDto): Promise<void> =>
  invoke<void>("confirm_digest", { sessionId, digest });

/** Return the current digest for the active session. */
export const getDigest = (sessionId: string): Promise<DigestDto> =>
  invoke<DigestDto>("get_digest", { sessionId });

/** Re-run digest extraction without re-embedding (DIGEST_REVIEW only). */
export const reextractDigest = (sessionId: string): Promise<DigestDto> =>
  invoke<DigestDto>("reextract_digest", { sessionId });

/** Return persisted context text for any session (Past Sessions → Start similar). */
export const getSessionContext = (sessionId: string): Promise<string> =>
  invoke<string>("get_session_context", { sessionId });

/** Re-bind the active session to a past row (fields, digest, question bank). */
export const reopenSession = (sessionId: string): Promise<SessionSnapshotDto> =>
  invoke<SessionSnapshotDto>("reopen_session", { sessionId });

/** Return the full session state snapshot for React resync. */
export const getSessionSnapshot = (): Promise<SessionSnapshotDto> =>
  invoke<SessionSnapshotDto>("get_session_snapshot");

/** Reopen an ENDED session at Rehearsal (Past Sessions). */
export const reopenPastSession = (sessionId: string): Promise<SessionSnapshotDto> =>
  invoke<SessionSnapshotDto>("reopen_past_session", { sessionId });

/** Restore the most recent pre-live draft from SQLite (startup). */
export const restoreDraftSession = (): Promise<boolean> =>
  invoke<boolean>("restore_draft_session");

// ──────────────────────────────────────────────────────────────────────────────
// Phase 6 — crash recovery + post-session
// ──────────────────────────────────────────────────────────────────────────────

export interface RecoveryOffer {
  sessionId: string;
  interruptedState: string;
  transcriptChunkCount: number;
  responseCount: number;
  name: string;
  sessionType: string;
  domain: string;
  createdAt: number;
  lastChunkTimestampMs: number | null;
  additionalCrashedCount: number;
}

export interface SessionSummaryDto {
  id: string;
  state: string;
  createdAt: number;
  expiresInSecs: number;
  promoted: boolean;
  name: string;
  sessionType: string;
  domain: string;
}

export interface OpenSessionLimitsDto {
  openCount: number;
  openLimit: number;
  plan: "free" | "premium";
}

/** On app startup: check for a crashed session. Returns null if none. */
export const checkCrashRecovery = (): Promise<RecoveryOffer | null> =>
  invoke<RecoveryOffer | null>("check_crash_recovery");

/** Resume a crashed session: RECOVERING → READY. */
export const resumeCrashedSession = (): Promise<void> =>
  invoke<void>("resume_crashed_session");

/** Discard a crashed session and return to IDLE. */
export const discardCrashedSession = (): Promise<void> =>
  invoke<void>("discard_crashed_session");

/** Generate a structured post-session summary using the session_essence prompt. */
export const generateSessionSummary = (): Promise<string> =>
  invoke<string>("generate_session_summary");

export interface ReviewChunkDto {
  speaker: "System" | "Microphone";
  text: string;
  timestampMs: number;
  labelSource: string;
}

export interface SessionReviewDto {
  sessionId: string;
  state: string;
  transcript: ReviewChunkDto[];
  questionsCount: number;
  directionalCount: number;
  depthCount: number;
  clarifyingCount: number;
}

/** Load a past session's transcript + AI-suggestion counts for review. */
export const getSessionReview = (sessionId: string): Promise<SessionReviewDto> =>
  invoke<SessionReviewDto>("get_session_review", { sessionId });

/** List all sessions stored locally. */
export const listSessions = (): Promise<SessionSummaryDto[]> =>
  invoke<SessionSummaryDto[]>("list_sessions");

/** Concurrent open-session cap for the current plan. */
export const getOpenSessionLimits = (): Promise<OpenSessionLimitsDto> =>
  invoke<OpenSessionLimitsDto>("get_open_session_limits");

/** Mark a session as promoted (exempt from 30-day expiry). */
export const promoteSession = (sessionId: string): Promise<void> =>
  invoke<void>("promote_session", { sessionId });

/** Remove the promoted flag — session resumes normal 30-day expiry. */
export const demoteSession = (sessionId: string): Promise<void> =>
  invoke<void>("demote_session", { sessionId });

/** Delete a session and all its data from local SQLite. */
export const deleteSession = (sessionId: string): Promise<void> =>
  invoke<void>("delete_session", { sessionId });

// ──────────────────────────────────────────────────────────────────────────────
// Phase 7.4 — cost cap enforcement
// ──────────────────────────────────────────────────────────────────────────────

export type CostCapStatusName = "ok" | "warning_80" | "reached";

export interface CostStatusDto {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  costEstimateUsd: number;
  maxTotalTokens: number | null;
  maxCostEstimateUsd: number | null;
  suspended: boolean;
  status: CostCapStatusName;
  fractionUsed: number | null;
}

interface RawCostStatus {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cost_estimate_usd: number;
  max_total_tokens: number | null;
  max_cost_estimate_usd: number | null;
  suspended: boolean;
  status: CostCapStatusName;
  fraction_used: number | null;
}

const adaptCostStatus = (raw: RawCostStatus): CostStatusDto => ({
  inputTokens: raw.input_tokens,
  outputTokens: raw.output_tokens,
  totalTokens: raw.total_tokens,
  costEstimateUsd: raw.cost_estimate_usd,
  maxTotalTokens: raw.max_total_tokens,
  maxCostEstimateUsd: raw.max_cost_estimate_usd,
  suspended: raw.suspended,
  status: raw.status,
  fractionUsed: raw.fraction_used,
});

/** Snapshot cumulative usage, cap, and suspension flag. */
export const getCostStatus = async (): Promise<CostStatusDto> =>
  adaptCostStatus(await invoke<RawCostStatus>("get_cost_status"));

/** Configure caps. Pass null on either field to remove that dimension. */
export const setCostCap = async (
  maxTotalTokens: number | null,
  maxCostEstimateUsd: number | null,
): Promise<CostStatusDto> =>
  adaptCostStatus(
    await invoke<RawCostStatus>("set_cost_cap", {
      maxTotalTokens,
      maxCostEstimateUsd,
    }),
  );

/** Clear the suspended flag; counters and cap unchanged. */
export const liftCostSuspension = async (): Promise<CostStatusDto> =>
  adaptCostStatus(await invoke<RawCostStatus>("lift_cost_suspension"));

/** Zero all cumulative counters. */
export const resetCostTracker = async (): Promise<CostStatusDto> =>
  adaptCostStatus(await invoke<RawCostStatus>("reset_cost_tracker"));

// ──────────────────────────────────────────────────────────────────────────────
// Phase 7.5 — GDPR right-to-deletion + right-to-export
// ──────────────────────────────────────────────────────────────────────────────

export interface DeleteAccountReport {
  supabaseDeleted: boolean;
  supabaseError: string | null;
  keychainCleared: boolean;
  keychainError: string | null;
  vectorStoreCleared: boolean;
  vectorStoreError: string | null;
  sqliteCleared: boolean;
  sqliteError: string | null;
  sessionsCleared: number;
}

interface RawDeleteAccountReport {
  supabase_deleted: boolean;
  supabase_error: string | null;
  keychain_cleared: boolean;
  keychain_error: string | null;
  vector_store_cleared: boolean;
  vector_store_error: string | null;
  sqlite_cleared: boolean;
  sqlite_error: string | null;
  sessions_cleared: number;
}

const adaptDeleteAccountReport = (raw: RawDeleteAccountReport): DeleteAccountReport => ({
  supabaseDeleted: raw.supabase_deleted,
  supabaseError: raw.supabase_error,
  keychainCleared: raw.keychain_cleared,
  keychainError: raw.keychain_error,
  vectorStoreCleared: raw.vector_store_cleared,
  vectorStoreError: raw.vector_store_error,
  sqliteCleared: raw.sqlite_cleared,
  sqliteError: raw.sqlite_error,
  sessionsCleared: raw.sessions_cleared,
});

/**
 * Run the GDPR right-to-deletion flow end-to-end.
 *
 * Wipes the Supabase auth user, the OS keychain, the local SQLite database,
 * and the per-session vector store. Each step is independently best-effort —
 * inspect the returned report to surface partial failures to the user.
 */
export const deleteAccount = async (): Promise<DeleteAccountReport> =>
  adaptDeleteAccountReport(await invoke<RawDeleteAccountReport>("delete_account"));

/**
 * Return a JSON blob of every locally-stored session, transcript, and
 * response. The caller is responsible for writing it to disk (or sharing it
 * via the system share sheet).
 */
export const exportUserData = (): Promise<string> => invoke<string>("export_user_data");

// ── Phase 7.6 — Feature flags ────────────────────────────────────────────────

export type FlagsOrigin = "remote" | "cache" | "defaults";

export interface FeatureFlag {
  name: string;
  enabled: boolean;
  allowed_plans: UserPlan[];
  rollout_percentage: number;
  ga: boolean;
}

export interface FeatureFlagsSnapshot {
  origin: FlagsOrigin;
  fetchedAt: string;
  flagCount: number;
  flags: FeatureFlag[];
}

interface RawFeatureFlagsSnapshot {
  origin: FlagsOrigin;
  fetched_at: string;
  flag_count: number;
  flags: FeatureFlag[];
}

const adaptSnapshot = (raw: RawFeatureFlagsSnapshot): FeatureFlagsSnapshot => ({
  origin: raw.origin,
  fetchedAt: raw.fetched_at,
  flagCount: raw.flag_count,
  flags: raw.flags,
});

/**
 * Resolve a flag against the currently authenticated user's plan + a
 * stable hash of their UUID. Pure read — never hits the network.
 */
export const isFeatureEnabled = (flag: string): Promise<boolean> =>
  invoke<boolean>("is_feature_enabled", { flag });

/**
 * Pull the latest flag set from the Supabase `/flags` Edge Function and
 * write it through to the local cache. Failures leave the previous flag
 * set authoritative — caller can ignore the rejection.
 */
export const refreshFeatureFlags = (): Promise<void> =>
  invoke<void>("refresh_feature_flags");

/**
 * Diagnostics: which source is currently authoritative (remote / cache /
 * compiled defaults), when it was last fetched, and the full flag list.
 * Useful for the dev dashboard and for "why is this flag off?" reports.
 */
export const getFeatureFlagsSnapshot = async (): Promise<FeatureFlagsSnapshot> =>
  adaptSnapshot(await invoke<RawFeatureFlagsSnapshot>("get_feature_flags_snapshot"));

// ── Phase 7.7 — Provider API key management ──────────────────────────────────

export type ApiKeyProvider =
  | "groq"
  | "deepseek"
  | "openrouter"
  | "openai"
  | "anthropic"
  | "tavily";

export type PrimaryLlmProvider = "groq" | "openai" | "anthropic" | "deepseek";

/** @deprecated Use ApiKeyProvider */
export type LlmProvider = Extract<ApiKeyProvider, "groq" | "openai" | "anthropic">;

/**
 * Store an LLM provider API key in the OS keychain. The plaintext value
 * lives in JS for the duration of this call only and is never echoed by
 * the backend.
 */
export const saveProviderKey = (provider: ApiKeyProvider, key: string): Promise<void> =>
  invoke<void>("save_provider_key", { provider, key });

/**
 * Whether a key is currently stored for `provider`. The actual key value
 * is never sent over IPC.
 */
export const isProviderKeyPresent = (provider: ApiKeyProvider): Promise<boolean> =>
  invoke<boolean>("is_provider_key_present", { provider });

/**
 * Remove `provider`'s key from the OS keychain. Safe to call when no key
 * is stored.
 */
export const clearProviderKey = (provider: ApiKeyProvider): Promise<void> =>
  invoke<void>("clear_provider_key", { provider });

// ── Phase 5.5.3 — Question bank ──────────────────────────────────────────────

export interface QuestionBankEntry {
  question: string;
  satisfied: boolean;
  confidenceScore: number;
  coachScore: number;
  lastSource: string | null;
  hasPreferredAnswer: boolean;
  tags?: string[];
}

export interface SessionFocusDto {
  focusName: string;
  focusTags: string[];
  recruiterBrief: string;
  focusNotes: string;
  focusConfirmedAt: number | null;
  needsFocusRefresh: boolean;
}

export const getSessionFocus = (sessionId: string): Promise<SessionFocusDto> =>
  invoke<SessionFocusDto>("get_session_focus", { sessionId });

export const saveSessionFocus = (
  sessionId: string,
  focus: SessionFocusDto,
): Promise<void> => invoke<void>("save_session_focus", { sessionId, focus });

export const listQuestionBankTags = (sessionId: string): Promise<string[]> =>
  invoke<string[]>("list_question_bank_tags", { sessionId });

export const setPhoneCallMode = (enabled: boolean): Promise<void> =>
  invoke<void>("set_phone_call_mode", { enabled });

export const getPreferredAnswer = (
  sessionId: string,
  question: string,
): Promise<string> =>
  invoke<string>("get_preferred_answer", { sessionId, question });

export const savePreferredAnswer = (
  sessionId: string,
  question: string,
  answer: string,
): Promise<void> =>
  invoke<void>("save_preferred_answer", { sessionId, question, answer });

export const getQuestionBank = (
  sessionId: string,
  shuffle = true,
  filterByFocus = true,
): Promise<QuestionBankEntry[]> =>
  invoke<QuestionBankEntry[]>("get_question_bank", {
    sessionId,
    shuffle,
    filterByFocus,
  });

export const addToQuestionBank = (sessionId: string, question: string): Promise<string[]> =>
  invoke<string[]>("add_to_question_bank", { sessionId, question });

export const removeFromQuestionBank = (sessionId: string, question: string): Promise<string[]> =>
  invoke<string[]>("remove_from_question_bank", { sessionId, question });

// ── Phase 5.5.6 — Research chat ──────────────────────────────────────────────

export const runResearchChat = (sessionId: string, message: string): Promise<void> =>
  invoke<void>("run_research_chat", { sessionId, message });

export interface WebSource {
  title: string;
  url: string;
  snippet: string;
}

export interface AppendResearchResult {
  chunksAdded: number;
}

export const appendResearchToContext = (
  sessionId: string,
  question: string,
  answer: string,
  webSources: WebSource[],
): Promise<AppendResearchResult> =>
  invoke<AppendResearchResult>("append_research_to_context", {
    sessionId,
    question,
    answer,
    webSources,
  });

// ── Phase 8 — Mock Interview ──────────────────────────────────────────────────

export interface MockTurn {
  id: string;
  turn_n: number;
  question: string;
  user_text: string;
  audio_path: string;
  coach_json: string;
  suggested: string;
  score: number;
}

export interface GrammarIssue {
  original: string;
  fix: string;
  why: string;
}

export interface CoachAxes {
  content: number;
  specificity: number;
  company_alignment: number;
  delivery: number;
}

export interface CoachFeedback {
  grammar_issues: GrammarIssue[];
  tone: { assessment: string; suggestion: string };
  context_gaps: string[];
  corrected_answer: string;
  score: number;
  axes?: CoachAxes;
}

export type { MockStudyMode } from "../events";

export const startMock = (
  guided = false,
  mode: MockStudyMode = "practice",
  shuffle = false,
): Promise<void> => invoke<void>("start_mock", { guided, mode, shuffle });

export const askMockQuestion = (): Promise<void> =>
  invoke<void>("ask_mock_question");

export const startMockTurn = (): Promise<void> => invoke<void>("start_mock_turn");

export const abortMockTurn = (): Promise<void> => invoke<void>("abort_mock_turn");

export const endMockTurn = (): Promise<void> => invoke<void>("end_mock_turn");

export const advanceMockTurn = (): Promise<void> => invoke<void>("advance_mock_turn");

export const retryMockTurn = (): Promise<void> => invoke<void>("retry_mock_turn");

export const regradeMockTurn = (userText: string): Promise<void> =>
  invoke<void>("regrade_mock_turn", { userText });

export const skipMockTurn = (): Promise<void> => invoke<void>("skip_mock_turn");

export const stopMock = (finish = false): Promise<void> =>
  invoke<void>("stop_mock", { finish });

export const getMockTurns = (): Promise<MockTurn[]> =>
  invoke<MockTurn[]>("get_mock_turns");

export const readMockAudioDataUrl = (path: string): Promise<string> =>
  invoke<string>("read_mock_audio_data_url", { path });

export interface HeadphoneGateStatusDto {
  blocked: boolean;
  overridden: boolean;
  message: string;
  fixInstruction: string | null;
}

export interface MicCalibrationStatusDto {
  passedOnDevice: boolean;
  deviceFingerprint: string;
  werSystem: number | null;
  werMic: number | null;
  forced: boolean;
  calibratedAt: number | null;
}

export interface CalibrationResultDto {
  wer: number;
  passed: boolean;
  transcript: string;
}

export const getMicCalibrationStatus = (): Promise<MicCalibrationStatusDto> =>
  invoke<MicCalibrationStatusDto>("get_mic_calibration_status");

export const getHeadphoneGateStatus = (): Promise<HeadphoneGateStatusDto> =>
  invoke<HeadphoneGateStatusDto>("get_headphone_gate_status");

export const setHeadphoneGateOverride = (enabled: boolean): Promise<void> =>
  invoke<void>("set_headphone_gate_override", { enabled });

export const markMicCalibrationPassed = (
  werSystem: number,
  werMic: number,
  forced = false,
): Promise<void> =>
  invoke<void>("mark_mic_calibration_passed", { werSystem, werMic, forced });

export const clearMicCalibration = (): Promise<void> =>
  invoke<void>("clear_mic_calibration");

export const runSystemAudioCalibration = (): Promise<CalibrationResultDto> =>
  invoke<CalibrationResultDto>("run_system_audio_calibration");

export const runMicCalibration = (): Promise<CalibrationResultDto> =>
  invoke<CalibrationResultDto>("run_mic_calibration");
