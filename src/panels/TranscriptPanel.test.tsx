import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import TranscriptPanel, {
  __triggerResponseImpl,
  appendLine,
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
}) => void;

const streamHandlerRef: { current: StreamHandler | null } = { current: null };

vi.mock("../hooks/useTranscriptionStream", () => ({
  useTranscriptionStream: (handler: StreamHandler) => {
    streamHandlerRef.current = handler;
  },
}));

function pushChunk(chunk: {
  text: string;
  speaker: Speaker;
  labelSource?: string;
}): void {
  const handler = streamHandlerRef.current;
  if (!handler) throw new Error("transcription stream handler not attached");
  act(() => {
    handler({
      text: chunk.text,
      speaker: chunk.speaker,
      timestamp: 0,
      labelSource: chunk.labelSource,
    });
  });
}

describe("appendLine aggregation", () => {
  it("merges consecutive same-speaker fragments into one utterance", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Tell me about", "System", 1, "channel", nextId);
    lines = appendLine(lines, "a tight deadline", "System", 2, "channel", nextId);
    lines = appendLine(lines, "you handled.", "System", 3, "channel", nextId);

    expect(lines).toHaveLength(1);
    expect(lines[0].text).toBe("Tell me about a tight deadline you handled.");
    expect(lines[0].speaker).toBe("System");
  });

  it("starts a new bubble when the speaker changes", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Tell me about yourself.", "System", 1, "channel", nextId);
    lines = appendLine(lines, "I am an architect", "Microphone", 2, "channel", nextId);

    expect(lines).toHaveLength(2);
    expect(lines[1].speaker).toBe("Microphone");
    expect(lines[1].text).toBe("I am an architect");
  });

  it("does not merge across the merge window", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "First sentence.", "System", 1, "channel", nextId);
    lines[0].lastArrivalMs = Date.now() - 60_000;
    lines = appendLine(lines, "Second sentence.", "System", 2, "channel", nextId);

    expect(lines).toHaveLength(2);
  });

  it("marks the utterance corrected when any fragment was auto-relabeled", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "I led the platform", "System", 1, "heuristic", nextId);

    expect(lines[0].corrected).toBe(true);
  });

  it("never merges audio-gap markers", () => {
    let lines: TranscriptLine[] = [];
    lines = appendLine(lines, "Some answer.", "Microphone", 1, "channel", nextId);
    lines = appendLine(lines, "[audio gap 3s]", "Microphone", 2, "channel", nextId);

    expect(lines).toHaveLength(2);
    expect(lines[1].text).toBe("[audio gap 3s]");
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

describe("Q-per-sentence chip", () => {
  beforeEach(() => {
    streamHandlerRef.current = null;
    __triggerResponseImpl.fn = vi.fn().mockResolvedValue(undefined);
  });

  it("renders one Q chip per interviewer sentence and none for user lines", () => {
    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Hi there! How are you today?", speaker: "System" });
    pushChunk({ text: "I am doing well, thank you.", speaker: "Microphone" });

    const chips = screen.getAllByTestId("q-chip");
    expect(chips).toHaveLength(2);
    chips.forEach((chip) => {
      expect(chip.textContent).toBe("Q");
      expect(chip.getAttribute("data-status")).toBe("idle");
    });
  });

  it("dispatches only the clicked sentence via triggerResponse", async () => {
    const trigger = vi.fn().mockResolvedValue(undefined);
    __triggerResponseImpl.fn = trigger;

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Hi there! Tell me about yourself.", speaker: "System" });

    const chips = screen.getAllByTestId("q-chip");
    expect(chips).toHaveLength(2);
    const second = chips[1] as HTMLButtonElement;

    fireEvent.click(second);

    await waitFor(() => {
      expect(trigger).toHaveBeenCalledTimes(1);
    });
    expect(trigger).toHaveBeenCalledWith("Tell me about yourself.", "sess-1");
    expect(second.getAttribute("data-status")).toBe("asking");
    expect(second.disabled).toBe(true);
  });

  it("only one chip is in 'asking' state at a time", async () => {
    const trigger = vi.fn().mockResolvedValue(undefined);
    __triggerResponseImpl.fn = trigger;

    render(<TranscriptPanel sessionId="sess-1" />);
    pushChunk({ text: "Hi there! Tell me about yourself.", speaker: "System" });

    const chips = screen.getAllByTestId("q-chip");
    const first = chips[0] as HTMLButtonElement;
    const second = chips[1] as HTMLButtonElement;
    fireEvent.click(first);
    await waitFor(() => expect(trigger).toHaveBeenCalledTimes(1));

    fireEvent.click(second);
    await waitFor(() => expect(trigger).toHaveBeenCalledTimes(2));

    expect(first.getAttribute("data-status")).toBe("idle");
    expect(second.getAttribute("data-status")).toBe("asking");
  });

  it("surfaces an error message and re-enables the chip on failure", async () => {
    __triggerResponseImpl.fn = vi.fn().mockRejectedValue(new Error("offline"));

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
