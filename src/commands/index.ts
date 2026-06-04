import { invoke } from "@tauri-apps/api/core";

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
  | "panic_hotkey";

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

export const logout = (): Promise<void> => invoke<void>("logout");

export const getCurrentUser = (): Promise<UserDto> =>
  invoke<UserDto>("get_current_user");

export const startSession = (sessionId: string): Promise<void> =>
  invoke<void>("start_session", { sessionId });

export const stopSession = (): Promise<void> => invoke<void>("stop_session");

export const triggerResponse = (
  question: string,
  sessionId: string,
  rephrase?: boolean,
): Promise<void> =>
  invoke<void>("trigger_response", { question, sessionId, rephrase: rephrase ?? null });

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

export const rephraseResponse = (
  question: string,
  sessionId: string,
): Promise<void> => triggerResponse(question, sessionId, true);

export const switchProvider = (name: string): Promise<void> =>
  invoke<void>("switch_provider", { name });

// ──────────────────────────────────────────────────────────────────────────────
// Session design commands (Phase 2)
// ──────────────────────────────────────────────────────────────────────────────

export interface SessionConfigDto {
  name: string;
  /** "interview" | "meeting" | "presentation" | "negotiation" */
  sessionType: string;
  domain: string;
}

export interface DigestDto {
  role: string;
  company: string;
  domain: string;
  keySkills: string[];
  seniority: string;
  likelyQuestions: string[];
  topicsToAvoid: string[];
}

export interface SessionSnapshotDto {
  sessionId: string | null;
  state: string;
  digest: DigestDto | null;
}

/** Create a new session. Returns the session UUID string. */
export const createSession = (config: SessionConfigDto): Promise<string> =>
  invoke<string>("create_session", { config });

/** Chunk, embed, and ingest context text; extract the digest. */
export const ingestContext = (sessionId: string, text: string): Promise<void> =>
  invoke<void>("ingest_context", { sessionId, text });

/** Accept the (possibly edited) digest and trigger pre-warming. */
export const confirmDigest = (sessionId: string, digest: DigestDto): Promise<void> =>
  invoke<void>("confirm_digest", { sessionId, digest });

/** Return the current digest for the active session. */
export const getDigest = (sessionId: string): Promise<DigestDto> =>
  invoke<DigestDto>("get_digest", { sessionId });

/** Return the full session state snapshot for React resync. */
export const getSessionSnapshot = (): Promise<SessionSnapshotDto> =>
  invoke<SessionSnapshotDto>("get_session_snapshot");
