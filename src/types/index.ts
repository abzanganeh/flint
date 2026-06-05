export enum SessionState {
  IDLE = "IDLE",
  CONFIGURING = "CONFIGURING",
  INGESTING = "INGESTING",
  DIGEST_REVIEW = "DIGEST_REVIEW",
  PRE_WARMING = "PRE_WARMING",
  REHEARSING = "REHEARSING",
  READY = "READY",
  LIVE = "LIVE",
  PAUSED = "PAUSED",
  ENDING = "ENDING",
  ENDED = "ENDED",
  CRASHED = "CRASHED",
  RECOVERING = "RECOVERING",
}

export type PanelId =
  | "transcript"
  | "directional"
  | "depth"
  | "clarifying"
  | "context";

export type Speaker = "System" | "Microphone";

export type ConfidenceLevel = "green" | "blue" | "amber" | "amber_low" | "grey" | "red";

export type HardwareTier = 1 | 2 | 3 | 4;

export interface TranscriptionChunk {
  text: string;
  speaker: Speaker;
  timestamp: number;
}

export interface PanelLayout {
  sizes: Record<PanelId, number>;
  collapsed: Record<PanelId, boolean>;
}

export interface Notification {
  id: string;
  message: string;
  level: "info" | "warn" | "error";
}

export interface ClarifyingQuestion {
  question: string;
  rank: number;
}

export interface RagChunk {
  text: string;
  score: number;
}

export interface TokenUsage {
  input: number;
  output: number;
  total: number;
  costEstimate: number;
}

export type CostCapStatus = "ok" | "warning_80" | "reached";

export interface CostCapState {
  status: CostCapStatus;
  suspended: boolean;
  fractionUsed: number | null;
  maxTotalTokens: number | null;
  maxCostEstimateUsd: number | null;
}

export interface UIState {
  panelLayout: PanelLayout;
  focusedPanel: PanelId | null;
  streamingBuffers: {
    directional: string;
    depth: string;
  };
  confidenceLevel: ConfidenceLevel | null;
  depthPrePrepared: boolean;
  digestSummary: string | null;
  lastManualQuestion: string;
  clarifyingQuestions: ClarifyingQuestion[];
  ragChunks: RagChunk[];
  tokenUsage: TokenUsage;
  costCap: CostCapState;
  notificationQueue: Notification[];
  theme: "light" | "dark" | "system";
  overlayMinimised: boolean;
  panicHideActive: boolean;
  answerNowMode: boolean;
}

export interface SessionConfig {
  name: string;
  type: string;
  domain: string;
  templateId?: string;
}

export interface SessionSummary {
  id: string;
  name: string;
  domain: string;
  createdAt: string;
  promoted: boolean;
}
