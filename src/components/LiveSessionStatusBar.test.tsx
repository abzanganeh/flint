import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import LiveSessionStatusBar from "./LiveSessionStatusBar";

const signalQuestionEnded = vi.fn();
const getProviderPriority = vi.fn();
const setLastManualQuestion = vi.fn();
const pushNotification = vi.fn();

vi.mock("../commands", () => ({
  signalQuestionEnded: (...args: unknown[]) => signalQuestionEnded(...args),
  getProviderPriority: () => getProviderPriority(),
}));

vi.mock("../store/ui", () => ({
  useUIStore: (
    selector: (state: {
      setLastManualQuestion: typeof setLastManualQuestion;
      pushNotification: typeof pushNotification;
    }) => unknown,
  ) => selector({ setLastManualQuestion, pushNotification }),
}));

const handlers: Record<string, (payload: unknown) => void> = {};

vi.mock("../events", () => ({
  onFailoverTriggered: (handler: (payload: unknown) => void) => {
    handlers.failover = handler;
    return Promise.resolve(() => undefined);
  },
  onPrimaryRestored: (handler: (payload: unknown) => void) => {
    handlers.restored = handler;
    return Promise.resolve(() => undefined);
  },
  onTurnStarted: (handler: (payload: unknown) => void) => {
    handlers.turnStarted = handler;
    return Promise.resolve(() => undefined);
  },
  onDirectionalToken: (handler: (payload: unknown) => void) => {
    handlers.directional = handler;
    return Promise.resolve(() => undefined);
  },
  onThreadStatus: (handler: (payload: unknown) => void) => {
    handlers.threadStatus = handler;
    return Promise.resolve(() => undefined);
  },
}));

vi.mock("../hooks/useTranscriptionStream", () => ({
  useTranscriptionStream: (handler: (line: { text: string; speaker: string; timestamp: number }) => void) => {
    handlers.transcription = handler;
  },
}));

describe("LiveSessionStatusBar", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getProviderPriority.mockResolvedValue(["groq", "deepseek"]);
    signalQuestionEnded.mockResolvedValue(undefined);
    Object.keys(handlers).forEach((key) => delete handlers[key]);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("updates provider badge on failover", async () => {
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      handlers.failover?.({ from: "groq", to: "deepseek" });
    });
    expect(screen.getByTestId("live-provider-badge").textContent).toContain("DeepSeek");
  });

  it("shows rolling transcript from system chunks using wall-clock window", async () => {
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await act(async () => {
      handlers.transcription?.({
        text: "Tell me about yourself.",
        speaker: "System",
        // Backend sends session-relative ms — must not break the rolling window.
        timestamp: 4_200,
      });
    });
    expect(screen.getByTestId("live-rolling-transcript").textContent).toContain(
      "Tell me about yourself.",
    );
  });

  it("flashes Q button and calls signalQuestionEnded on click", async () => {
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await act(async () => {
      handlers.transcription?.({
        text: "Why this role?",
        speaker: "System",
        timestamp: 9_000,
      });
    });

    await act(async () => {
      fireEvent.click(screen.getByTestId("live-q-button"));
    });

    expect(signalQuestionEnded).toHaveBeenCalledWith("session-1");
    expect(screen.getByTestId("live-q-button").className).toContain("live-q-button--flash");
  });

  it("surfaces backend errors when signalQuestionEnded fails", async () => {
    signalQuestionEnded.mockRejectedValue(
      "No interviewer transcript captured since the last question signal.",
    );
    render(<LiveSessionStatusBar sessionId="session-1" />);

    await act(async () => {
      fireEvent.click(screen.getByTestId("live-q-button"));
    });

    expect(pushNotification).toHaveBeenCalled();
    expect(screen.getByTestId("live-q-error").textContent).toContain(
      "No interviewer transcript captured",
    );
  });
});
