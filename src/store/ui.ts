import { create } from "zustand";

import type {
  ClarifyingQuestion,
  ConfidenceLevel,
  CostCapState,
  Notification,
  PanelId,
  PanelLayout,
  RagChunk,
  TokenUsage,
  TurnCard,
  UIState,
} from "../types";

/// Completed turns kept for in-panel history. Enough to scroll back over the
/// last few questions without growing the overlay unbounded mid-interview.
const TURN_HISTORY_LIMIT = 8;

interface UIStore extends UIState {
  setPanelLayout: (panelLayout: PanelLayout) => void;
  setPanelSize: (id: PanelId, size: number) => void;
  togglePanelCollapsed: (id: PanelId) => void;
  setLayoutMode: (mode: "stack" | "grid") => void;
  setFocusedPanel: (focusedPanel: PanelId | null) => void;
  appendDirectionalToken: (token: string) => void;
  appendDepthToken: (token: string) => void;
  clearStreamingBuffers: () => void;
  /** Clear panel content when entering LIVE so rehearsal answers do not carry over. */
  resetOrchestratorPanels: () => void;
  /** Turn boundary: archive the current card into history and start fresh. */
  startTurn: (question: string, turn: number) => void;
  setConfidenceLevel: (level: ConfidenceLevel | null) => void;
  setDepthPrePrepared: (depthPrePrepared: boolean) => void;
  setDigestSummary: (digestSummary: string | null) => void;
  setLastManualQuestion: (question: string) => void;
  addClarifyingQuestion: (q: Omit<ClarifyingQuestion, "id">) => void;
  clearClarifyingQuestions: () => void;
  setRagChunks: (chunks: RagChunk[]) => void;
  setTokenUsage: (usage: TokenUsage) => void;
  accumulateTokenUsage: (
    input: number,
    output: number,
    costDelta: number,
    usageCategory?: string,
  ) => void;
  setCostCap: (cap: CostCapState) => void;
  setNotificationQueue: (notificationQueue: Notification[]) => void;
  pushNotification: (n: Notification) => void;
  setTheme: (theme: UIState["theme"]) => void;
  setOverlayMinimised: (overlayMinimised: boolean) => void;
  setPanicHideActive: (panicHideActive: boolean) => void;
  setAnswerNowMode: (answerNowMode: boolean) => void;
}

// Default panel sizes per layout mode.
// Stack mode follows FR-4.6: Transcript 20%, Directional 30%, Depth 30%,
// Clarifying 10%, Context 10% (weights sum to 5).
const DEFAULT_STACK_SIZES: PanelLayout["sizes"] = {
  transcript: 1.0,
  directional: 1.5,
  depth: 1.5,
  clarifying: 0.5,
  context: 0.5,
};

// Grid mode (horizontal): Directional dominant, Transcript and Depth equal,
// Clarifying + Context narrower side panels.
const DEFAULT_GRID_SIZES: PanelLayout["sizes"] = {
  transcript: 1,
  directional: 1.5,
  depth: 1,
  clarifying: 0.75,
  context: 0.75,
};

const defaultPanelLayout: PanelLayout = {
  sizes: DEFAULT_STACK_SIZES,
  collapsed: {
    transcript: false,
    directional: false,
    depth: false,
    clarifying: false,
    context: false,
  },
};

const defaultTokenUsage: TokenUsage = {
  input: 0,
  output: 0,
  total: 0,
  costEstimate: 0,
  breakdown: {},
};

const defaultCostCap: CostCapState = {
  status: "ok",
  suspended: false,
  fractionUsed: null,
  maxTotalTokens: null,
  maxCostEstimateUsd: null,
};

// Safely resolve the persisted layout mode. Defaults to "stack" (FR-4.6) when
// localStorage is missing (e.g. SSR) or contains an unknown value.
function readPersistedLayoutMode(): "stack" | "grid" {
  try {
    const raw = typeof localStorage !== "undefined"
      ? localStorage.getItem("flint_layout_mode")
      : null;
    if (raw === "stack" || raw === "grid") return raw;
  } catch {
    // localStorage may throw in privacy modes; fall through to default.
  }
  return "stack";
}

function persistLayoutMode(mode: "stack" | "grid"): void {
  try {
    localStorage.setItem("flint_layout_mode", mode);
  } catch {
    // Best-effort persistence; toggle still applies in-memory.
  }
}

export const useUIStore = create<UIStore>((set) => ({
  panelLayout: defaultPanelLayout,
  layoutMode: readPersistedLayoutMode(),
  focusedPanel: null,
  streamingBuffers: { directional: "", depth: "" },
  currentQuestion: "",
  turnHistory: [],
  confidenceLevel: null,
  depthPrePrepared: false,
  digestSummary: null,
  lastManualQuestion: "",
  clarifyingQuestions: [],
  ragChunks: [],
  tokenUsage: defaultTokenUsage,
  costCap: defaultCostCap,
  notificationQueue: [],
  theme: "system",
  overlayMinimised: false,
  panicHideActive: false,
  answerNowMode: false,

  setPanelLayout: (panelLayout) => set({ panelLayout }),

  setLayoutMode: (layoutMode) => {
    persistLayoutMode(layoutMode);
    set((s) => ({
      layoutMode,
      // Reseed sizes to that mode's defaults so panel proportions match the
      // spec. Collapsed state is preserved — it's per-panel intent, not layout.
      panelLayout: {
        ...s.panelLayout,
        sizes: layoutMode === "stack" ? DEFAULT_STACK_SIZES : DEFAULT_GRID_SIZES,
      },
    }));
  },

  setPanelSize: (id, size) =>
    set((s) => ({
      panelLayout: {
        ...s.panelLayout,
        sizes: { ...s.panelLayout.sizes, [id]: Math.max(0.25, size) },
      },
    })),

  togglePanelCollapsed: (id) =>
    set((s) => ({
      panelLayout: {
        ...s.panelLayout,
        collapsed: {
          ...s.panelLayout.collapsed,
          [id]: !s.panelLayout.collapsed[id],
        },
      },
    })),

  setFocusedPanel: (focusedPanel) => set({ focusedPanel }),

  appendDirectionalToken: (token) =>
    set((s) => ({
      streamingBuffers: {
        ...s.streamingBuffers,
        directional: s.streamingBuffers.directional + token,
      },
    })),

  appendDepthToken: (token) =>
    set((s) => ({
      streamingBuffers: {
        ...s.streamingBuffers,
        depth: s.streamingBuffers.depth + token,
      },
    })),

  clearStreamingBuffers: () =>
    set({
      streamingBuffers: { directional: "", depth: "" },
      depthPrePrepared: false,
    }),

  resetOrchestratorPanels: () =>
    set({
      streamingBuffers: { directional: "", depth: "" },
      depthPrePrepared: false,
      clarifyingQuestions: [],
      confidenceLevel: null,
      answerNowMode: false,
      currentQuestion: "",
      turnHistory: [],
      lastManualQuestion: "",
      ragChunks: [],
    }),

  startTurn: (question, turn) =>
    set((s) => {
      const hasContent =
        s.streamingBuffers.directional.length > 0 ||
        s.streamingBuffers.depth.length > 0;
      const archived: TurnCard[] = hasContent
        ? [
            {
              id:
                typeof crypto !== "undefined" && "randomUUID" in crypto
                  ? crypto.randomUUID()
                  : `${Date.now()}-${Math.random()}`,
              turn: turn - 1,
              question: s.currentQuestion,
              directional: s.streamingBuffers.directional,
              depth: s.streamingBuffers.depth,
              confidenceLevel: s.confidenceLevel,
            },
            ...s.turnHistory,
          ].slice(0, TURN_HISTORY_LIMIT)
        : s.turnHistory;
      return {
        turnHistory: archived,
        currentQuestion: question,
        streamingBuffers: { directional: "", depth: "" },
        confidenceLevel: null,
        depthPrePrepared: false,
        clarifyingQuestions: [],
        answerNowMode: s.answerNowMode,
      };
    }),

  setConfidenceLevel: (confidenceLevel) => set({ confidenceLevel }),

  setDepthPrePrepared: (depthPrePrepared) => set({ depthPrePrepared }),

  setDigestSummary: (digestSummary) => set({ digestSummary }),

  setLastManualQuestion: (lastManualQuestion) => set({ lastManualQuestion }),

  addClarifyingQuestion: (q) =>
    set((s) => {
      const norm = q.question.trim().toLowerCase();
      if (
        s.clarifyingQuestions.some(
          (existing) => existing.question.trim().toLowerCase() === norm,
        )
      ) {
        return s;
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random()}`;
      return {
        clarifyingQuestions: [...s.clarifyingQuestions, { ...q, id }].sort(
          (a, b) => a.rank - b.rank,
        ),
      };
    }),

  clearClarifyingQuestions: () => set({ clarifyingQuestions: [] }),

  setRagChunks: (ragChunks) => set({ ragChunks }),

  setTokenUsage: (tokenUsage) => set({ tokenUsage }),

  accumulateTokenUsage: (input, output, costDelta, usageCategory) =>
    set((s) => {
      const breakdown = { ...s.tokenUsage.breakdown };
      if (usageCategory) {
        breakdown[usageCategory] = (breakdown[usageCategory] ?? 0) + input + output;
      }
      return {
        tokenUsage: {
          input: s.tokenUsage.input + input,
          output: s.tokenUsage.output + output,
          total: s.tokenUsage.total + input + output,
          costEstimate: s.tokenUsage.costEstimate + costDelta,
          breakdown,
        },
      };
    }),

  setCostCap: (costCap) => set({ costCap }),

  setNotificationQueue: (notificationQueue) => set({ notificationQueue }),

  pushNotification: (n) =>
    set((s) => ({ notificationQueue: [...s.notificationQueue, n] })),

  setTheme: (theme) => set({ theme }),
  setOverlayMinimised: (overlayMinimised) => set({ overlayMinimised }),
  setPanicHideActive: (panicHideActive) => set({ panicHideActive }),
  setAnswerNowMode: (answerNowMode) => set({ answerNowMode }),
}));
