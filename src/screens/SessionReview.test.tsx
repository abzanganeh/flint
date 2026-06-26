import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SessionReview, mergeReviewChunks } from "./SessionReview";
import type { ReviewChunkDto, SessionReviewDto } from "../commands";

vi.mock("../commands", () => ({
  getSessionReview: vi.fn(),
}));

const chunk = (
  speaker: "System" | "Microphone",
  text: string,
  labelSource = "channel",
): ReviewChunkDto => ({ speaker, text, timestampMs: 0, labelSource });

describe("mergeReviewChunks", () => {
  it("merges consecutive same-speaker chunks into one utterance", () => {
    const merged = mergeReviewChunks([
      chunk("System", "Tell me about"),
      chunk("System", "your background."),
      chunk("Microphone", "I led the platform."),
    ]);
    expect(merged).toHaveLength(2);
    expect(merged[0]).toMatchObject({
      speaker: "System",
      text: "Tell me about your background.",
    });
    expect(merged[1].speaker).toBe("Microphone");
  });

  it("flags an utterance as corrected when a chunk was relabeled", () => {
    const merged = mergeReviewChunks([chunk("System", "I am an architect", "heuristic")]);
    expect(merged[0].corrected).toBe(true);
  });

  it("does not flag channel-derived labels as corrected", () => {
    const merged = mergeReviewChunks([chunk("System", "Where are you located?")]);
    expect(merged[0].corrected).toBe(false);
  });

  it("returns an empty list for no chunks", () => {
    expect(mergeReviewChunks([])).toEqual([]);
  });
});

describe("SessionReview screen", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  const review = (overrides: Partial<SessionReviewDto> = {}): SessionReviewDto => ({
    sessionId: "sess-1",
    state: "ENDED",
    transcript: [
      chunk("System", "Tell me about yourself."),
      chunk("Microphone", "I am an IAM architect."),
    ],
    questionsCount: 1,
    directionalCount: 1,
    depthCount: 1,
    clarifyingCount: 0,
    ...overrides,
  });

  it("renders the transcript with speaker labels", async () => {
    const { getSessionReview } = await import("../commands");
    vi.mocked(getSessionReview).mockResolvedValue(review());

    render(<SessionReview sessionId="sess-1" onBack={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByText("Tell me about yourself.")).toBeTruthy();
    });
    expect(screen.getByText("I am an IAM architect.")).toBeTruthy();
    expect(screen.getAllByTestId("review-utterance")).toHaveLength(2);
  });

  it("shows the single-channel note when no mic audio was captured", async () => {
    const { getSessionReview } = await import("../commands");
    vi.mocked(getSessionReview).mockResolvedValue(
      review({
        transcript: [
          chunk("System", "Hello?"),
          chunk("System", "Tell me about yourself."),
        ],
      }),
    );

    render(<SessionReview sessionId="sess-1" onBack={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-review-single-channel-note")).toBeTruthy();
    });
  });

  it("shows an empty-state message when nothing was recorded", async () => {
    const { getSessionReview } = await import("../commands");
    vi.mocked(getSessionReview).mockResolvedValue(
      review({ transcript: [], questionsCount: 0, directionalCount: 0, depthCount: 0 }),
    );

    render(<SessionReview sessionId="sess-1" onBack={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-review-empty")).toBeTruthy();
    });
  });

  it("surfaces an error when the command fails", async () => {
    const { getSessionReview } = await import("../commands");
    vi.mocked(getSessionReview).mockRejectedValue(new Error("db locked"));

    render(<SessionReview sessionId="sess-1" onBack={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-review-error").textContent).toContain(
        "db locked",
      );
    });
  });
});
