import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import SessionFocusGate from "./SessionFocusGate";

vi.mock("../commands", () => ({
  getFocusTagCatalog: vi.fn(),
  getSessionFocus: vi.fn(),
  saveSessionFocus: vi.fn(),
}));

const emptyFocus = () => ({
  focusName: "",
  focusTags: [] as string[],
  recruiterBrief: "",
  focusNotes: "",
  focusConfirmedAt: null,
  needsFocusRefresh: false,
});

const catalog = [
  { id: "self-assessment", label: "Self-assessment", description: "d", questionCount: 3 },
  { id: "motivation", label: "Motivation", description: "d", questionCount: 0 },
  { id: "behavioral", label: "Behavioral", description: "d", questionCount: 2 },
  { id: "competency", label: "Competency", description: "d", questionCount: 0 },
  { id: "culture", label: "Culture fit", description: "d", questionCount: 0 },
  { id: "technical", label: "Technical", description: "d", questionCount: 5 },
  { id: "logistics", label: "Logistics", description: "d", questionCount: 0 },
  { id: "general", label: "General", description: "d", questionCount: 1 },
];

describe("SessionFocusGate focus-tag catalog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders all 8 catalog chips, including zero-count ones, with visual distinction", async () => {
    const { getFocusTagCatalog, getSessionFocus } = await import("../commands");
    vi.mocked(getSessionFocus).mockResolvedValue(emptyFocus());
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-focus-gate")).toBeTruthy();
    });

    for (const tag of catalog) {
      expect(screen.getByTestId(`focus-tag-chip-${tag.id}`)).toBeTruthy();
      expect(screen.getByTestId(`focus-tag-count-${tag.id}`).textContent).toBe(
        String(tag.questionCount),
      );
    }

    const zeroCountChip = screen.getByTestId("focus-tag-chip-motivation") as HTMLElement;
    const populatedChip = screen.getByTestId("focus-tag-chip-technical") as HTMLElement;

    expect(zeroCountChip.style.opacity).toBe("0.55");
    expect(populatedChip.style.opacity).toBe("1");
    expect(zeroCountChip.style.opacity).not.toBe(populatedChip.style.opacity);
  });

  it("still allows selecting a zero-count tag and saving it", async () => {
    const { getFocusTagCatalog, getSessionFocus, saveSessionFocus } = await import("../commands");
    vi.mocked(getSessionFocus).mockResolvedValue(emptyFocus());
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(saveSessionFocus).mockResolvedValue(undefined);

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("focus-tag-chip-motivation")).toBeTruthy();
    });

    fireEvent.click(screen.getByTestId("focus-tag-chip-motivation"));
    fireEvent.click(screen.getByTestId("session-focus-continue"));

    await waitFor(() => {
      expect(saveSessionFocus).toHaveBeenCalledWith(
        "s1",
        expect.objectContaining({ focusTags: ["motivation"] }),
      );
    });
  });
});
