import { describe, expect, it } from "vitest";

import { appendLine, type TranscriptLine } from "./TranscriptPanel";

const nextId = (() => {
  let n = 0;
  return () => ++n;
})();

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
    // Force the previous line's arrival far into the past so the window lapses.
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
