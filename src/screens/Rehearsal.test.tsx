import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUIStore } from "../store/ui";
import Rehearsal from "./Rehearsal";

const runRehearsalTurn = vi.fn().mockResolvedValue(undefined);
const getCostStatus = vi.fn().mockResolvedValue({ suspended: false, totalTokens: 0 });
const getSessionContextFields = vi.fn().mockResolvedValue({
  jobDescription: "",
  profile: "",
  companyOverview: "",
  leadershipPrinciples: "",
  roleExpectations: "",
  technicalPrep: "",
  strategyNotes: "",
  speakingStyle: "polished",
  sessionVocabulary: "",
});

vi.mock("../commands", () => ({
  completeRehearsal: vi.fn().mockResolvedValue(undefined),
  getCostStatus: (...args: unknown[]) => getCostStatus(...args),
  getSessionContextFields: (...args: unknown[]) => getSessionContextFields(...args),
  runRehearsalTurn: (...args: unknown[]) => runRehearsalTurn(...args),
}));

vi.mock("../hooks/useCostCap", () => ({
  useCostCap: () => undefined,
}));

vi.mock("../hooks/useHotkeys", () => ({
  useHotkeys: () => undefined,
  isRehearsalSubmitChord: () => false,
}));

vi.mock("../hooks/useOrchestratorStreams", () => ({
  useOrchestratorStreams: () => undefined,
}));

vi.mock("../hooks/useRagChunks", () => ({
  useRagChunks: () => undefined,
}));

vi.mock("../hooks/useTokenUsage", () => ({
  useTokenUsage: () => undefined,
}));

vi.mock("../panels/AnswerPanel", () => ({
  default: () => <div data-testid="answer-panel-stub" />,
}));

vi.mock("../panels/TranscriptPanel", () => ({
  default: () => <div data-testid="transcript-panel-stub" />,
}));

vi.mock("../panels/ContextPanel", () => ({
  default: () => <div data-testid="context-panel-stub" />,
}));

vi.mock("../components/PreferredAnswerPanel", () => ({
  default: () => null,
}));

vi.mock("../components/AddContextPanel", () => ({
  default: () => null,
}));

vi.mock("../components/StoryEditor", () => ({
  default: () => null,
}));

vi.mock("../components/PrepChecklist", () => ({
  default: () => null,
}));

vi.mock("../components/QuestionBank", () => ({
  default: () => null,
}));

vi.mock("../components/ResearchChat", () => ({
  default: () => null,
}));

vi.mock("../components/FirstRunRehearsalModal", () => ({
  default: () => null,
  isFirstRunModalDismissed: () => true,
}));

vi.mock("../components/MicQualityBadge", () => ({
  default: () => null,
}));

vi.mock("../components/PanicRestoreShell", () => ({
  default: ({ children }: { children: unknown }) => children,
}));

vi.mock("../components/TokenBudgetIndicator", () => ({
  default: () => null,
}));

vi.mock("../components/SessionContextBadges", () => ({
  default: () => null,
}));

vi.mock("../components/UsageWidget", () => ({
  default: () => null,
}));

describe("Rehearsal Generate diagram", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    runRehearsalTurn.mockResolvedValue(undefined);
    getCostStatus.mockResolvedValue({ suspended: false, totalTokens: 0 });
    useUIStore.setState({
      streamingBuffers: { answer: "", visual: "" },
      currentQuestion: "Design a URL shortener.",
      lastManualQuestion: "Design a URL shortener.",
      confidenceLevel: null,
      ragChunks: [],
      costCap: { suspended: false, totalTokens: 0, softEstimate: 0 },
      depthPrePrepared: false,
    });
  });

  it("invokes runRehearsalTurn with forceVisual true when Generate diagram is clicked", async () => {
    render(
      <Rehearsal sessionId="sess-1" onComplete={vi.fn()} />,
    );

    fireEvent.click(screen.getByTestId("generate-diagram-button"));

    await waitFor(() => {
      expect(runRehearsalTurn).toHaveBeenCalledWith(
        "sess-1",
        "Design a URL shortener.",
        undefined,
        true,
      );
    });
  });
});
