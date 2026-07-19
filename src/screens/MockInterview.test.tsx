import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUIStore } from "../store/ui";
import MockInterview from "./MockInterview";

const triggerMockVisualResponse = vi.fn().mockResolvedValue(undefined);

vi.mock("../commands", () => ({
  startMock: vi.fn().mockResolvedValue(undefined),
  stopMock: vi.fn().mockResolvedValue(undefined),
  askMockQuestion: vi.fn().mockResolvedValue(undefined),
  abortMockTurn: vi.fn().mockResolvedValue(undefined),
  endMockTurn: vi.fn().mockResolvedValue(undefined),
  advanceMockTurn: vi.fn().mockResolvedValue(undefined),
  retryMockTurn: vi.fn().mockResolvedValue(undefined),
  regradeMockTurn: vi.fn().mockResolvedValue(undefined),
  savePreferredAnswer: vi.fn().mockResolvedValue(undefined),
  skipMockTurn: vi.fn().mockResolvedValue(undefined),
  copyTextToClipboard: vi.fn().mockResolvedValue(undefined),
  triggerMockVisualResponse: (...args: unknown[]) =>
    triggerMockVisualResponse(...args),
}));

interface MockQuestionStartedPayload {
  question: string;
  turn_n: number;
  total_questions: number;
  mode: "practice" | "study";
  preferred_hit?: boolean;
}

interface VisualTokenPayload {
  token: string;
}

interface MockSuggestedDonePayload {
  turn_n: number;
  error?: string | null;
}

const handlers: {
  questionStarted?: (payload: MockQuestionStartedPayload) => void;
  visualToken?: (payload: VisualTokenPayload) => void;
  suggestedDone?: (payload: MockSuggestedDonePayload) => void;
} = {};

vi.mock("../events", () => ({
  onMockCoachFeedback: () => Promise.resolve(() => undefined),
  onMockEnded: () => Promise.resolve(() => undefined),
  onMockQuestionStarted: (handler: (payload: MockQuestionStartedPayload) => void) => {
    handlers.questionStarted = handler;
    return Promise.resolve(() => undefined);
  },
  onMockQuestionSpoken: () => Promise.resolve(() => undefined),
  onMockTurnPhase: () => Promise.resolve(() => undefined),
  onMockSuggestedToken: () => Promise.resolve(() => undefined),
  onMockSuggestedDone: (handler: (payload: MockSuggestedDonePayload) => void) => {
    handlers.suggestedDone = handler;
    return Promise.resolve(() => undefined);
  },
  onMockUserTranscribed: () => Promise.resolve(() => undefined),
  onAudioQualityStatus: () => Promise.resolve(() => undefined),
  // Consumed by the real `useOrchestratorStreams` hook (intentionally not
  // mocked away) so visual_token plumbing into VisualPanel is exercised
  // end-to-end, same as Live/Rehearsal.
  onTurnStarted: () => Promise.resolve(() => undefined),
  onAnswerToken: () => Promise.resolve(() => undefined),
  onVisualToken: (handler: (payload: VisualTokenPayload) => void) => {
    handlers.visualToken = handler;
    return Promise.resolve(() => undefined);
  },
  onConfidenceScore: () => Promise.resolve(() => undefined),
  onResponseMetadata: () => Promise.resolve(() => undefined),
}));

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(),
  },
}));

vi.mock("shiki", () => ({
  codeToHtml: vi.fn(),
}));

const startMockQuestion = (payload: Partial<MockQuestionStartedPayload> = {}) => {
  const handler = handlers.questionStarted;
  if (!handler) throw new Error("mock_question_started handler not attached");
  act(() => {
    handler({
      question: "Design a URL shortener.",
      turn_n: 1,
      total_questions: 5,
      mode: "practice",
      preferred_hit: false,
      ...payload,
    });
  });
};

describe("MockInterview Visual support", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    handlers.questionStarted = undefined;
    handlers.visualToken = undefined;
    handlers.suggestedDone = undefined;
    useUIStore.setState({
      streamingBuffers: { answer: "", visual: "" },
      currentQuestion: "",
      confidenceLevel: null,
      depthPrePrepared: false,
      turnHistory: [],
    });
  });

  it("shows the Generate diagram button once a question is active and invokes trigger_mock_visual_response", async () => {
    render(
      <MockInterview sessionId="sess-1" onComplete={vi.fn()} onAbort={vi.fn()} />,
    );

    await waitFor(() => expect(handlers.questionStarted).toBeDefined());
    startMockQuestion();

    const button = await screen.findByTestId("generate-diagram-button");
    fireEvent.click(button);

    await waitFor(() => {
      expect(triggerMockVisualResponse).toHaveBeenCalledWith(
        "sess-1",
        "Design a URL shortener.",
      );
    });
  });

  it("does not show the diagram panel before any question has been asked", () => {
    render(
      <MockInterview sessionId="sess-1" onComplete={vi.fn()} onAbort={vi.fn()} />,
    );

    expect(screen.queryByTestId("generate-diagram-button")).toBeNull();
    expect(screen.queryByTestId("visual-panel")).toBeNull();
  });

  it("renders VisualPanel output when a visual_token event arrives", async () => {
    render(
      <MockInterview sessionId="sess-1" onComplete={vi.fn()} onAbort={vi.fn()} />,
    );

    await waitFor(() => expect(handlers.questionStarted).toBeDefined());
    startMockQuestion();

    await waitFor(() => expect(handlers.visualToken).toBeDefined());
    act(() => {
      handlers.visualToken?.({ token: "A plain-text diagram description." });
    });

    await waitFor(() => {
      expect(screen.getByTestId("visual-raw-fallback").textContent).toContain(
        "A plain-text diagram description.",
      );
    });
  });

  it("hides the diagram panel via the collapse toggle without losing the question gate", async () => {
    render(
      <MockInterview sessionId="sess-1" onComplete={vi.fn()} onAbort={vi.fn()} />,
    );

    await waitFor(() => expect(handlers.questionStarted).toBeDefined());
    startMockQuestion();

    await screen.findByTestId("generate-diagram-button");
    fireEvent.click(screen.getByTestId("mock-visual-toggle-button"));

    expect(screen.queryByTestId("visual-panel")).toBeNull();

    fireEvent.click(screen.getByTestId("mock-visual-toggle-button"));
    expect(await screen.findByTestId("visual-panel")).toBeTruthy();
  });
});

describe("MockInterview suggested-answer generation failure", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    handlers.questionStarted = undefined;
    handlers.visualToken = undefined;
    handlers.suggestedDone = undefined;
    useUIStore.setState({
      streamingBuffers: { answer: "", visual: "" },
      currentQuestion: "",
      confidenceLevel: null,
      depthPrePrepared: false,
      turnHistory: [],
    });
  });

  // Regression: a provider failure before any suggested-answer token streamed
  // used to be swallowed silently, leaving Study mode stuck on "Generating…"
  // forever with no indication anything went wrong.
  it("surfaces an error and stops the streaming indicator when suggested-answer generation fails", async () => {
    render(
      <MockInterview sessionId="sess-1" onComplete={vi.fn()} onAbort={vi.fn()} />,
    );

    await waitFor(() => expect(handlers.questionStarted).toBeDefined());
    startMockQuestion({ mode: "study" });

    await waitFor(() => expect(handlers.suggestedDone).toBeDefined());
    act(() => {
      handlers.suggestedDone?.({
        turn_n: 1,
        error: "Couldn't generate a suggested answer right now — answer from your prep notes.",
      });
    });

    await waitFor(() => {
      expect(
        screen.getByText(
          "Couldn't generate a suggested answer right now — answer from your prep notes.",
        ),
      ).toBeTruthy();
    });
    expect(screen.queryByText("Generating…")).toBeNull();
    expect(screen.queryByText("streaming…")).toBeNull();
  });
});
