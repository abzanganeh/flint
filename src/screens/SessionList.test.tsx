import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { SessionList } from "./SessionList";

vi.mock("../commands", () => ({
  deleteSession: vi.fn().mockResolvedValue(undefined),
  demoteSession: vi.fn(),
  getDigest: vi.fn(),
  getOpenSessionLimits: vi.fn().mockResolvedValue({
    openCount: 0,
    openLimit: 3,
    plan: "free",
  }),
  getSessionContext: vi.fn(),
  getSessionContextFields: vi.fn(),
  listSessions: vi.fn().mockResolvedValue([
    {
      id: "sess-1",
      state: "ENDED",
      createdAt: 1_700_000_000,
      expiresInSecs: 86400,
      promoted: true,
      name: "Fisher IAM Architect",
      sessionType: "interview",
      domain: "Information Security",
    },
  ]),
  promoteSession: vi.fn(),
}));

describe("SessionList delete confirmation", () => {
  it("requires explicit confirmation before deleting a session", async () => {
    const { deleteSession } = await import("../commands");

    render(<SessionList onBack={() => undefined} />);

    await waitFor(() => {
      expect(screen.getByText(/Fisher IAM Architect/)).toBeTruthy();
    });

    fireEvent.click(screen.getByText(/Fisher IAM Architect/));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    expect(screen.getByTestId("session-delete-dialog")).toBeTruthy();
    expect(deleteSession).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId("session-delete-confirm"));

    await waitFor(() => {
      expect(deleteSession).toHaveBeenCalledWith("sess-1");
    });
  });
});
