import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import TranscriptPanel, {
  __relabelImpl,
  __signalQuestionEndedImpl,
  __triggerResponseImpl,
  appendLine,
  applyChunkRelabel,
  linesToPlainText,
  splitIntoSentences,
  type TranscriptLine,
} from "./TranscriptPanel";
import type { Speaker } from "../types";

const nextId = (() => {
  let n = 0;
  return () => ++n;
})();

// ── Capture the live transcription handler so the test can inject chunks. ──

type StreamHandler = (line: {
  text: string;
  speaker: Speaker;
  timestamp: number;
  labelSource?: string;
  chunkId?: string;
}) => void;

const streamHandlerRef: { current: StreamHandler | null } = { current: null };

vi.mock("../hooks/useTranscriptionStream", () => ({
  useTranscriptionStream: (handler: StreamHandler) => {
    streamHandlerRef.current = handler;
  },
}));

vi.mock("../commands", () => ({
  triggerResponse: vi.fn(),
  signalQuestionEnded: vi.fn(),
  copyTextToClipboard: vi.fn(),
  relabelTranscriptChunk: vi.fn(),
}));

vi.mock("../events", () => ({
  onTranscriptChunkRelabeled: () => Promise.resolve(() => undefined),
  onSpeakerRefined: () => Promise.resolve(() => undefined),
}));

import { copyTextToClipboard } from "../commands";

function pushChunk(chunk: {
  text: string;
  speaker: Speaker;
  labelSource?: string;
  chunkId?: string;
}): void {
  const handler = streamHandlerRef.current;
  if (!handler) throw new Error("transcription stream handler not attached");
  act(() => {
    handler({
      text: chunk.text,
      speaker: chunk.speaker,
      timestamp: 0,
      labelSource: chunk.labelSource,
      chunkId: chunk.chunkId,
    });
  });
}

describe("appendLine aggregation", () => {
  it("merges consecutive same-speaker fragments into one utterance", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Tell me about", "System", 1, "channel", "c1", nextId);
    lines = appendLine(lines, "a tight deadline", "System", 2, "channel", "c2", nextId);
    lines = appendLine(lines, "you handled.", "System", 3, "channel", "c3", nextId);

    expect(lines).toHaveLength(1);
    expect(lines[0].text).toBe("Tell me about a tight deadline you handled.");
    expect(lines[0].speaker).toBe("System");
    expect(lines[0].chunkIds).toEqual(["c1", "c2", "c3"]);
  });

  it("starts a new bubble when the speaker changes", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Tell me about yourself.", "System", 1, "channel", "c1", nextId);
    lines = appendLine(lines, "I am an architect", "Microphone", 2, "channel", "c2", nextId);

    expect(lines).toHaveLength(2);
    expect(lines[1].speaker).toBe("Microphone");
    expect(lines[1].text).toBe("I am an architect");
  });

  it("does not merge across the merge window", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "First sentence.", "System", 1, "channel", "c1", nextId);
    lines[0].lastArrivalMs = Date.now() - 60_000;
    lines = appendLine(lines, "Second sentence.", "System", 2, "channel", "c2", nextId);

    expect(lines).toHaveLength(2);
  });

  it("marks the utterance corrected when any fragment was auto-relabeled", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "I led the platform", "System", 1, "heuristic", "c1", nextId);

    expect(lines[0].corrected).toBe(true);
  });

  it("never merges audio-gap markers", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Some answer.", "Microphone", 1, "channel", "c1", nextId);
    lines = appendLine(lines, "[audio gap 3s]", "Microphone", 2, "channel", undefined, nextId);

    expect(lines).toHaveLength(2);
    expect(lines[1].text).toBe("[audio gap 3s]");
  });

  it("applyChunkRelabel updates lines containing the chunk id", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Tell me about yourself.", "System", 1, "heuristic", "abc", nextId);
    lines = applyChunkRelabel(lines, "abc", "Microphone", "user");
    expect(lines[0].speaker).toBe("Microphone");
    expect(lines[0].labelSource).toBe("user");
  });
});

describe("splitIntoSentences", () => {
  it("splits on terminal punctuation and keeps it attached", () => {
    expect(splitIntoSentences("Hi there! How are you today?")).toEqual([
      "Hi there!",
      "How are you today?",
    ]);
  });

  it("treats trailing fragments without terminal punctuation as one sentence", () => {
    expect(splitIntoSentences("Tell me about yourself")).toEqual([
      "Tell me about yourself",
    ]);
  });

  it("collapses multiple terminators into one sentence boundary", () => {
    expect(splitIntoSentences("Really?!  Wow.")).toEqual(["Really?!", "Wow."]);
  });

  it("drops empty and ultra-short fragments", () => {
    // Any chunk shorter than the min-character floor is dropped before display.
    const sentences = splitIntoSentences("Yes. . . Tell me more.");
    expect(sentences).toContain("Yes.");
    expect(sentences).toContain("Tell me more.");
    expect(sentences.every((s) => s.length >= 3)).toBe(true);
  });

  it("handles empty input", () => {
    expect(splitIntoSentences("   ")).toEqual([]);
  });
});

describe("Q-per-utterance chip", () => {
  beforeEach(() => {
    streamHandlerRef.current = null;
    __triggerResponseImpl.fn = vi.fn().mockResolvedValue(undefined);
    __signalQuestionEndedImpl.fn = vi.fn().mockResolvedValue(undefined);
    __relabelImpl.fn = vi.fn().mockResolvedValue(undefined);
    vi.mocked(copyTextToClipboard).mockReset();
    vi.mocked(copyTextToClipboard).mockResolvedValue(undefined);
  });

  it("renders one Q chip per merged interviewer utterance and none for user lines", () => {
    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Hi there! How are you today?", speaker: "System" });
    pushChunk({ text: "I am doing well, thank you.", speaker: "Microphone" });

    const chips = screen.getAllByTestId("q-chip");
    expect(chips).toHaveLength(1);
    expect(chips[0].textContent).toBe("Q");
  });

  it("dispatches the latest merged interviewer utterance via signalQuestionEnded", async () => {
    const signal = vi.fn().mockResolvedValue(undefined);
    __signalQuestionEndedImpl.fn = signal;

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Hi there!", speaker: "System" });
    pushChunk({ text: "Tell me about yourself.", speaker: "System" });

    const chip = screen.getByTestId("q-chip") as HTMLButtonElement;
    fireEvent.click(chip);

    await waitFor(() => {
      expect(signal).toHaveBeenCalledTimes(1);
    });
    expect(signal).toHaveBeenCalledWith("sess-1");
    expect(__triggerResponseImpl.fn).not.toHaveBeenCalled();
    expect(chip.getAttribute("data-status")).toBe("asking");
    expect(chip.disabled).toBe(true);
  });

  it("dispatches older interviewer lines via triggerResponse", async () => {
    const trigger = vi.fn().mockResolvedValue(undefined);
    __triggerResponseImpl.fn = trigger;

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "First question?", speaker: "System" });
    pushChunk({ text: "I am answering.", speaker: "Microphone" });
    pushChunk({ text: "Second question?", speaker: "System" });

    const chips = screen.getAllByTestId("q-chip");
    expect(chips).toHaveLength(2);
    fireEvent.click(chips[0]);

    await waitFor(() => {
      expect(trigger).toHaveBeenCalledTimes(1);
    });
    expect(trigger).toHaveBeenCalledWith("First question?", "sess-1");
    expect(__signalQuestionEndedImpl.fn).not.toHaveBeenCalled();
  });

  it("only one chip is in 'asking' state at a time", async () => {
    const trigger = vi.fn().mockResolvedValue(undefined);
    const signal = vi.fn().mockResolvedValue(undefined);
    __triggerResponseImpl.fn = trigger;
    __signalQuestionEndedImpl.fn = signal;

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "First question?", speaker: "System" });
    pushChunk({ text: "I am answering.", speaker: "Microphone" });
    pushChunk({ text: "Second question?", speaker: "System" });

    const chips = screen.getAllByTestId("q-chip");
    expect(chips).toHaveLength(2);
    fireEvent.click(chips[0] as HTMLButtonElement);
    await waitFor(() => expect(trigger).toHaveBeenCalledTimes(1));

    fireEvent.click(chips[1] as HTMLButtonElement);
    await waitFor(() => expect(signal).toHaveBeenCalledTimes(1));

    expect(chips[0].getAttribute("data-status")).toBe("idle");
    expect(chips[1].getAttribute("data-status")).toBe("asking");
  });

  it("linesToPlainText formats interviewer and user labels", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Tell me about yourself.", "System", 1, "channel", "c1", nextId);
    lines = appendLine(lines, "I am an architect.", "Microphone", 2, "channel", "c2", nextId);
    expect(linesToPlainText(lines)).toBe(
      "INTERVIEWER: Tell me about yourself.\n\nYOU: I am an architect.",
    );
  });

  it("swap button invokes relabel for all chunk ids on the line", async () => {
    const relabel = vi.fn().mockResolvedValue(undefined);
    __relabelImpl.fn = relabel;

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({
      text: "Tell me about yourself.",
      speaker: "System",
      chunkId: "chunk-a",
    });

    const swapButtons = screen.getAllByTestId("speaker-swap-btn");
    fireEvent.click(swapButtons[0] as HTMLButtonElement);

    await waitFor(() => {
      expect(relabel).toHaveBeenCalledWith("chunk-a", "Microphone");
    });
  });

  it("copies transcript via native clipboard command", async () => {
    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Tell me about yourself.", speaker: "System" });
    pushChunk({ text: "I am an architect.", speaker: "Microphone" });

    fireEvent.click(screen.getByTestId("copy-transcript-btn"));

    await waitFor(() => {
      expect(copyTextToClipboard).toHaveBeenCalledWith(
        "INTERVIEWER: Tell me about yourself.\n\nYOU: I am an architect.",
      );
    });
  });

  it("surfaces an error message and re-enables the chip on failure", async () => {
    __signalQuestionEndedImpl.fn = vi.fn().mockRejectedValue(new Error("offline"));

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Tell me about yourself.", speaker: "System" });

    const chip = screen.getByTestId("q-chip");
    fireEvent.click(chip);

    await waitFor(() => {
      expect(screen.getByTestId("transcript-ask-error").textContent).toContain(
        "offline",
      );
    });
    expect(chip.getAttribute("data-status")).toBe("idle");
    expect((chip as HTMLButtonElement).disabled).toBe(false);
  });
});
