import { describe, expect, it } from "vitest";

import { useUIStore } from "../store/ui";

describe("UI store panel layout", () => {
  it("toggles panel collapse", () => {
    const { togglePanelCollapsed, panelLayout } = useUIStore.getState();
    expect(panelLayout.collapsed.answer).toBe(false);
    togglePanelCollapsed("answer");
    expect(useUIStore.getState().panelLayout.collapsed.answer).toBe(true);
  });

  it("enforces minimum panel size on resize", () => {
    const { setPanelSize } = useUIStore.getState();
    setPanelSize("transcript", 0.1);
    expect(useUIStore.getState().panelLayout.sizes.transcript).toBe(0.25);
  });
});

describe("UI store answer/visual streaming buffers", () => {
  it("appendAnswerToken and appendVisualToken accumulate independently", () => {
    useUIStore.setState({ streamingBuffers: { answer: "", visual: "" } });
    const { appendAnswerToken, appendVisualToken } = useUIStore.getState();

    appendAnswerToken("Hello");
    appendAnswerToken(" world");
    appendVisualToken("```mermaid\n");
    appendVisualToken("flowchart TD\n```");

    const s = useUIStore.getState();
    expect(s.streamingBuffers.answer).toBe("Hello world");
    expect(s.streamingBuffers.visual).toBe("```mermaid\nflowchart TD\n```");
  });

  it("clearStreamingBuffers resets both buffers and the pre-prepared flag", () => {
    useUIStore.setState({ streamingBuffers: { answer: "", visual: "" } });
    const { appendAnswerToken, appendVisualToken, setDepthPrePrepared, clearStreamingBuffers } =
      useUIStore.getState();

    appendAnswerToken("draft answer");
    appendVisualToken("draft visual");
    setDepthPrePrepared(true);

    clearStreamingBuffers();

    const s = useUIStore.getState();
    expect(s.streamingBuffers.answer).toBe("");
    expect(s.streamingBuffers.visual).toBe("");
    expect(s.depthPrePrepared).toBe(false);
  });

  it("startTurn archives the completed answer/visual buffers into turn history", () => {
    useUIStore.setState({
      streamingBuffers: { answer: "", visual: "" },
      currentQuestion: "Tell me about a challenge",
      turnHistory: [],
    });
    const { appendAnswerToken, appendVisualToken, setConfidenceLevel, startTurn } =
      useUIStore.getState();

    appendAnswerToken("Brief answer");
    appendVisualToken("Visual detail");
    setConfidenceLevel("green");

    startTurn("What is your greatest strength?", 2);

    const s = useUIStore.getState();
    expect(s.turnHistory).toHaveLength(1);
    expect(s.turnHistory[0]).toMatchObject({
      turn: 1,
      question: "Tell me about a challenge",
      answer: "Brief answer",
      visual: "Visual detail",
      confidenceLevel: "green",
    });
    expect(s.currentQuestion).toBe("What is your greatest strength?");
    expect(s.streamingBuffers.answer).toBe("");
    expect(s.streamingBuffers.visual).toBe("");
    expect(s.confidenceLevel).toBeNull();
  });

  it("startTurn does not archive a turn when no answer/visual content streamed", () => {
    useUIStore.setState({ currentQuestion: "Unanswered question", turnHistory: [] });

    useUIStore.getState().startTurn("Next question", 1);

    expect(useUIStore.getState().turnHistory).toHaveLength(0);
  });
});

describe("UI store orchestrator reset", () => {
  it("resetOrchestratorPanels clears rehearsal carry-over into live", () => {
    const {
      appendAnswerToken,
      appendVisualToken,
      setConfidenceLevel,
      setLastManualQuestion,
      resetOrchestratorPanels,
    } = useUIStore.getState();

    appendAnswerToken("rehearsal answer");
    appendVisualToken("rehearsal visual");
    setConfidenceLevel("green");
    setLastManualQuestion("Tell me about yourself");

    resetOrchestratorPanels();

    const s = useUIStore.getState();
    expect(s.streamingBuffers.answer).toBe("");
    expect(s.streamingBuffers.visual).toBe("");
    expect(s.confidenceLevel).toBeNull();
    expect(s.lastManualQuestion).toBe("");
  });
});
