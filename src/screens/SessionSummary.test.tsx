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

  it("shows prepare for next round when callback is provided", async () => {
    mockGenerate.mockResolvedValue(
      JSON.stringify({
        date: "Jul 20",
        domain: "job interview",
        role: "Engineer",
        company: "DAT",
        questions_count: 5,
        topics_covered: ["motivation"],
        confidence_distribution: { high: 3, medium: 2, low: 0 },
        key_moments: [],
        follow_up_actions: ["Prep system design with Jacob"],
        one_line_summary: "Solid recruiter screen.",
      }),
    );

    const onPrepare = vi.fn();
    render(
      <SessionSummary
        sessionId="sess-dat"
        onDone={() => undefined}
        onPrepareNextRound={onPrepare}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("session-summary-prepare-next")).toBeTruthy();
    });

    screen.getByTestId("session-summary-prepare-next").click();
    expect(onPrepare).toHaveBeenCalledTimes(1);
  });
});
