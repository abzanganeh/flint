import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import LiveSessionStatusBar from "./LiveSessionStatusBar";

const signalQuestionEnded = vi.fn();
const getInterviewerSpanPreview = vi.fn();
const getProviderPriority = vi.fn();
const getTranscriptionProviderPreference = vi.fn();
const setLastManualQuestion = vi.fn();
const pushNotification = vi.fn();

vi.mock("../commands", () => ({
  signalQuestionEnded: (...args: unknown[]) => signalQuestionEnded(...args),
  getInterviewerSpanPreview: (...args: unknown[]) => getInterviewerSpanPreview(...args),
  getProviderPriority: () => getProviderPriority(),
  getTranscriptionProviderPreference: () => getTranscriptionProviderPreference(),
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
  onAnswerToken: (handler: (payload: unknown) => void) => {
    handlers.answer = handler;
    return Promise.resolve(() => undefined);
  },
  onThreadStatus: (handler: (payload: unknown) => void) => {
    handlers.threadStatus = handler;
    return Promise.resolve(() => undefined);
  },
  onTranscriptionFailoverTriggered: (handler: (payload: unknown) => void) => {
    handlers.transcriptionFailover = handler;
    return Promise.resolve(() => undefined);
  },
  onTranscriptionPrimaryRestored: (handler: (payload: unknown) => void) => {
    handlers.transcriptionRestored = handler;
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
    getTranscriptionProviderPreference.mockResolvedValue("whisper");
    signalQuestionEnded.mockResolvedValue(undefined);
    getInterviewerSpanPreview.mockResolvedValue({
      text: "Tell me about yourself.",
      uncertainSpeaker: false,
    });
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

  it("shows backend interviewer span preview instead of a blind 30s window", async () => {
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await waitFor(() => {
      expect(getInterviewerSpanPreview).toHaveBeenCalledWith("session-1");
    });
    expect(screen.getByTestId("live-rolling-transcript").textContent).toContain(
      "Tell me about yourself.",
    );
  });

  it("shows uncertain speaker hint in phone mode", async () => {
    getInterviewerSpanPreview.mockResolvedValue({
      text: "Why this role?",
      uncertainSpeaker: true,
    });
    render(<LiveSessionStatusBar sessionId="session-1" phoneCallMode />);
    await waitFor(() => {
      expect(screen.getByTestId("live-uncertain-speaker-hint").textContent).toContain(
        "uncertain speaker",
      );
    });
  });

  it("uses phone-mode 30s fallback only when span is empty and speaker uncertain", async () => {
    getInterviewerSpanPreview.mockResolvedValue({ text: "", uncertainSpeaker: true });
    render(<LiveSessionStatusBar sessionId="session-1" phoneCallMode />);
    await act(async () => {
      handlers.transcription?.({
        text: "Fallback interviewer line.",
        speaker: "System",
        timestamp: 1_000,
      });
    });
    await waitFor(() => {
      expect(screen.getByTestId("live-rolling-transcript").textContent).toContain(
        "Fallback interviewer line.",
      );
    });
  });

  it("flashes Q button and calls signalQuestionEnded on click", async () => {
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await waitFor(() => expect(getInterviewerSpanPreview).toHaveBeenCalled());

    await act(async () => {
      fireEvent.click(screen.getByTestId("live-q-button"));
    });

    expect(signalQuestionEnded).toHaveBeenCalledWith("session-1");
    expect(screen.getByTestId("live-q-button").className).toContain("live-q-button--flash");
    expect(screen.getByTestId("live-q-button").textContent).toContain("Ask now");
  });

  it("hides the transcription badge when Whisper is the preference", async () => {
    getTranscriptionProviderPreference.mockResolvedValue("whisper");
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await waitFor(() => expect(getInterviewerSpanPreview).toHaveBeenCalled());
    expect(screen.queryByTestId("live-transcription-badge")).toBeNull();
  });

  it("shows the transcription badge and flips to local on Deepgram failover", async () => {
    getTranscriptionProviderPreference.mockResolvedValue("deepgram");
    render(<LiveSessionStatusBar sessionId="session-1" />);
    await waitFor(() => {
      expect(screen.getByTestId("live-transcription-badge").textContent).toContain(
        "Deepgram",
      );
    });
    await act(async () => {
      handlers.transcriptionFailover?.({ from: "deepgram", to: "whisper" });
    });
    expect(screen.getByTestId("live-transcription-badge").textContent).toContain(
      "Deepgram unavailable",
    );
    await act(async () => {
      handlers.transcriptionRestored?.({ provider: "deepgram" });
    });
    expect(screen.getByTestId("live-transcription-badge").textContent).toContain(
      "Deepgram",
    );
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
