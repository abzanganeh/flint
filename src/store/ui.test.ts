import { describe, expect, it } from "vitest";

import { useUIStore } from "../store/ui";

describe("UI store panel layout", () => {
  it("toggles panel collapse", () => {
    const { togglePanelCollapsed, panelLayout } = useUIStore.getState();
    expect(panelLayout.collapsed.directional).toBe(false);
    togglePanelCollapsed("directional");
    expect(useUIStore.getState().panelLayout.collapsed.directional).toBe(true);
  });

  it("enforces minimum panel size on resize", () => {
    const { setPanelSize } = useUIStore.getState();
    setPanelSize("transcript", 0.1);
    expect(useUIStore.getState().panelLayout.sizes.transcript).toBe(0.25);
  });
});

describe("UI store orchestrator reset", () => {
  it("resetOrchestratorPanels clears rehearsal carry-over into live", () => {
    const {
      appendAnswerToken,
      appendVisualToken,
      addClarifyingQuestion,
      setConfidenceLevel,
      setLastManualQuestion,
      resetOrchestratorPanels,
    } = useUIStore.getState();

    appendAnswerToken("rehearsal answer");
    appendVisualToken("rehearsal visual");
    addClarifyingQuestion({ question: "Clarify?", rank: 1 });
    setConfidenceLevel("green");
    setLastManualQuestion("Tell me about yourself");

    resetOrchestratorPanels();

    const s = useUIStore.getState();
    expect(s.streamingBuffers.answer).toBe("");
    expect(s.streamingBuffers.visual).toBe("");
    expect(s.clarifyingQuestions).toEqual([]);
    expect(s.confidenceLevel).toBeNull();
    expect(s.lastManualQuestion).toBe("");
  });
});
