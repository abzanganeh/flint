import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SessionSummary } from "./SessionSummary";

vi.mock("../commands", () => ({
  generateSessionSummary: vi.fn(),
}));

import { generateSessionSummary } from "../commands";

const mockGenerate = vi.mocked(generateSessionSummary);

describe("SessionSummary", () => {
  beforeEach(() => {
    mockGenerate.mockReset();
  });

  it("shows soft unavailable UI with stats when narrative fallback JSON is returned", async () => {
    mockGenerate.mockResolvedValue(
      JSON.stringify({
        date: "",
        domain: "",
        role: "",
        company: "",
        questions_count: 3,
        topics_covered: [],
        confidence_distribution: { high: 2, medium: 1, low: 0 },
        key_moments: [],
        follow_up_actions: [],
        one_line_summary:
          "Summary unavailable — rate limited or offline. Retry from Past Sessions.",
      }),
    );

    render(<SessionSummary sessionId="sess-1" onDone={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-summary")).toBeTruthy();
    });
    expect(screen.getByTestId("session-summary-soft-unavailable")).toBeTruthy();
    expect(screen.getByText("3 questions")).toBeTruthy();
    expect(screen.getByTestId("session-summary-retry")).toBeTruthy();
  });

  it("shows invoke error with retry when command fails", async () => {
    mockGenerate.mockRejectedValue(new Error("No session to summarise."));

    render(<SessionSummary onDone={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-summary-error")).toBeTruthy();
    });
    expect(screen.getByText(/No session to summarise\./)).toBeTruthy();
  });
});
