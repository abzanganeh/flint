import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { ConfidenceLevel, SessionState, Speaker } from "../types";

export interface TranscriptionChunkEventPayload {
  text: string;
  speaker: Speaker;
  timestamp: number;
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
}

export interface SessionStateChangeEventPayload {
  state: SessionState;
}

export interface ContextTruncatedEventPayload {
  session_id: string;
}

export const onTranscriptionChunk = (
  handler: (payload: TranscriptionChunkEventPayload) => void,
): Promise<UnlistenFn> =>
  listen<TranscriptionChunkEventPayload>("transcription_chunk", (event) =>
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
