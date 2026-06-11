import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { ConfidenceLevel, SessionState, Speaker } from "../types";

export interface TranscriptionChunkEventPayload {
  text: string;
  speaker: Speaker;
  timestamp: number;
}

export interface TurnStartedEventPayload {
  question: string;
  turn: number;
}

export interface DirectionalTokenEventPayload {
  token: string;
}

export interface DepthTokenEventPayload {
  token: string;
}

export interface ClarifyingQuestionEventPayload {
  question: string;
  rank: number;
}

export interface ConfidenceScoreEventPayload {
  level: ConfidenceLevel;
}

export type ThreadStatus = "ok" | "error" | "idle";

export interface ThreadStatusEventPayload {
  thread: string;
  status: ThreadStatus;
}

export interface FailoverTriggeredEventPayload {
  from: string;
  to: string;
}

export interface PrimaryRestoredEventPayload {
  provider: string;
}

export interface TokenUsageUpdateEventPayload {
  input: number;
  output: number;
  total: number;
  cost_estimate: number;
  usage_category: string;
}

export interface SessionStateChangeEventPayload {
  state: SessionState;
}

export interface ContextTruncatedEventPayload {
  session_id: string;
}

export interface RagChunkEventPayload {
  text: string;
  score: number;
}

export interface RagChunksUpdateEventPayload {
  chunks: RagChunkEventPayload[];
}

export interface ResponseMetadataEventPayload {
  pre_prepared: boolean;
}

export interface OverlayVisibilityEventPayload {
  hidden: boolean;
}

export interface HotkeyTriggerEventPayload {
  action: string;
}

export type CostCapStatusName = "ok" | "warning_80" | "reached";

export interface CostCapStatusEventPayload {
  status: CostCapStatusName;
  suspended: boolean;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cost_estimate_usd: number;
  max_total_tokens: number | null;
  max_cost_estimate_usd: number | null;
  fraction_used: number | null;
}

export interface InferenceSuspendedEventPayload {
  reason: string;
  total_tokens: number;
  cost_estimate_usd: number;
}

export const onTranscriptionChunk = (
  handler: (payload: TranscriptionChunkEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<TranscriptionChunkEventPayload>("transcription_chunk", (event) =>
    handler(event.payload),
  );

export const onTurnStarted = (
  handler: (payload: TurnStartedEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<TurnStartedEventPayload>("turn_started", (event) =>
    handler(event.payload),
  );

export const onDirectionalToken = (
  handler: (payload: DirectionalTokenEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<DirectionalTokenEventPayload>("directional_token", (event) =>
    handler(event.payload),
  );

export const onDepthToken = (
  handler: (payload: DepthTokenEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<DepthTokenEventPayload>("depth_token", (event) =>
    handler(event.payload),
  );

export const onClarifyingQuestion = (
  handler: (payload: ClarifyingQuestionEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ClarifyingQuestionEventPayload>("clarifying_question", (event) =>
    handler(event.payload),
  );

export const onConfidenceScore = (
  handler: (payload: ConfidenceScoreEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ConfidenceScoreEventPayload>("confidence_score", (event) =>
    handler(event.payload),
  );

export const onThreadStatus = (
  handler: (payload: ThreadStatusEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ThreadStatusEventPayload>("thread_status", (event) =>
    handler(event.payload),
  );

export const onFailoverTriggered = (
  handler: (payload: FailoverTriggeredEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<FailoverTriggeredEventPayload>("failover_triggered", (event) =>
    handler(event.payload),
  );

export const onPrimaryRestored = (
  handler: (payload: PrimaryRestoredEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<PrimaryRestoredEventPayload>("primary_restored", (event) =>
    handler(event.payload),
  );

export const onTokenUsageUpdate = (
  handler: (payload: TokenUsageUpdateEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<TokenUsageUpdateEventPayload>("token_usage_update", (event) =>
    handler(event.payload),
  );

export const onSessionStateChange = (
  handler: (payload: SessionStateChangeEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<SessionStateChangeEventPayload>("session_state_change", (event) =>
    handler(event.payload),
  );

export const onContextTruncated = (
  handler: (payload: ContextTruncatedEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ContextTruncatedEventPayload>("context_truncated", (event) =>
    handler(event.payload),
  );

export const onRagChunksUpdate = (
  handler: (payload: RagChunksUpdateEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<RagChunksUpdateEventPayload>("rag_chunks_update", (event) =>
    handler(event.payload),
  );

export const onResponseMetadata = (
  handler: (payload: ResponseMetadataEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ResponseMetadataEventPayload>("response_metadata", (event) =>
    handler(event.payload),
  );

export const onOverlayVisibility = (
  handler: (payload: OverlayVisibilityEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<OverlayVisibilityEventPayload>("overlay_visibility", (event) =>
    handler(event.payload),
  );

export const onHotkeyTrigger = (
  handler: (payload: HotkeyTriggerEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<HotkeyTriggerEventPayload>("hotkey_trigger", (event) =>
    handler(event.payload),
  );

export const onCostCapStatus = (
  handler: (payload: CostCapStatusEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<CostCapStatusEventPayload>("cost_cap_status", (event) =>
    handler(event.payload),
  );

export const onInferenceSuspended = (
  handler: (payload: InferenceSuspendedEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<InferenceSuspendedEventPayload>("inference_suspended", (event) =>
    handler(event.payload),
  );

export const onSmartResumeImportToken = (
  handler: (token: string) => void,
): Promise<UnlistenFn> =>
  listen<string>("smart_resume_import_token", (event) => handler(event.payload));

// ── Phase 5.5.6 — Research chat events ────────────────────────────────────

export interface ResearchTokenEventPayload {
  token: string;
}

export interface ResearchCitationEventPayload {
  chunks: string[];
  webSources?: WebSourceCitation[];
  source?: "rag" | "web" | "rag_and_web" | "none";
  canAddToContext?: boolean;
}

export interface WebSourceCitation {
  title: string;
  url: string;
  snippet: string;
}

export const onResearchToken = (
  handler: (payload: ResearchTokenEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ResearchTokenEventPayload>("research_token", (event) =>
    handler(event.payload),
  );

export const onResearchCitation = (
  handler: (payload: ResearchCitationEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<ResearchCitationEventPayload>("research_citation", (event) =>
    handler(event.payload),
  );
