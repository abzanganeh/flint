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
  UIState,
} from "../types";

interface UIStore extends UIState {
  setPanelLayout: (panelLayout: PanelLayout) => void;
  setPanelSize: (id: PanelId, size: number) => void;
  togglePanelCollapsed: (id: PanelId) => void;
  setFocusedPanel: (focusedPanel: PanelId | null) => void;
  appendDirectionalToken: (token: string) => void;
  appendDepthToken: (token: string) => void;
  clearStreamingBuffers: () => void;
  setConfidenceLevel: (level: ConfidenceLevel | null) => void;
  setDepthPrePrepared: (depthPrePrepared: boolean) => void;
  setDigestSummary: (digestSummary: string | null) => void;
  setLastManualQuestion: (question: string) => void;
  addClarifyingQuestion: (q: ClarifyingQuestion) => void;
  clearClarifyingQuestions: () => void;
  setRagChunks: (chunks: RagChunk[]) => void;
  setTokenUsage: (usage: TokenUsage) => void;
  accumulateTokenUsage: (
    input: number,
    output: number,
    costDelta: number,
  ) => void;
  setCostCap: (cap: CostCapState) => void;
  setNotificationQueue: (notificationQueue: Notification[]) => void;
  pushNotification: (n: Notification) => void;
  setTheme: (theme: UIState["theme"]) => void;
  setOverlayMinimised: (overlayMinimised: boolean) => void;
  setPanicHideActive: (panicHideActive: boolean) => void;
  setAnswerNowMode: (answerNowMode: boolean) => void;
}

const defaultPanelLayout: PanelLayout = {
  sizes: {
    transcript: 1,
    directional: 1.5,
    depth: 1,
    clarifying: 0.75,
    context: 0.75,
  },
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
};

const defaultCostCap: CostCapState = {
  status: "ok",
  suspended: false,
  fractionUsed: null,
  maxTotalTokens: null,
  maxCostEstimateUsd: null,
};

export const useUIStore = create<UIStore>((set) => ({
  panelLayout: defaultPanelLayout,
  focusedPanel: null,
  streamingBuffers: { directional: "", depth: "" },
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

  setConfidenceLevel: (confidenceLevel) => set({ confidenceLevel }),

  setDepthPrePrepared: (depthPrePrepared) => set({ depthPrePrepared }),

  setDigestSummary: (digestSummary) => set({ digestSummary }),

  setLastManualQuestion: (lastManualQuestion) => set({ lastManualQuestion }),

  addClarifyingQuestion: (q) =>
    set((s) => ({
      clarifyingQuestions: [...s.clarifyingQuestions, q].sort(
        (a, b) => a.rank - b.rank,
      ),
    })),

  clearClarifyingQuestions: () => set({ clarifyingQuestions: [] }),

  setRagChunks: (ragChunks) => set({ ragChunks }),

  setTokenUsage: (tokenUsage) => set({ tokenUsage }),

  accumulateTokenUsage: (input, output, costDelta) =>
    set((s) => ({
      tokenUsage: {
        input: s.tokenUsage.input + input,
        output: s.tokenUsage.output + output,
        total: s.tokenUsage.total + input + output,
        costEstimate: s.tokenUsage.costEstimate + costDelta,
      },
    })),

  setCostCap: (costCap) => set({ costCap }),

  setNotificationQueue: (notificationQueue) => set({ notificationQueue }),

  pushNotification: (n) =>
    set((s) => ({ notificationQueue: [...s.notificationQueue, n] })),

  setTheme: (theme) => set({ theme }),
  setOverlayMinimised: (overlayMinimised) => set({ overlayMinimised }),
  setPanicHideActive: (panicHideActive) => set({ panicHideActive }),
  setAnswerNowMode: (answerNowMode) => set({ answerNowMode }),
}));
