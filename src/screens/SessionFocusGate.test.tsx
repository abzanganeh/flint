import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import SessionFocusGate from "./SessionFocusGate";

vi.mock("../commands", () => ({
  getFocusTagCatalog: vi.fn(),
  getSessionFocus: vi.fn(),
  inferRoundTypeFromBrief: vi.fn(),
  ingestRoundDebrief: vi.fn(),
  saveSessionFocus: vi.fn(),
}));

const emptyFocus = () => ({
  focusName: "",
  focusTags: [] as string[],
  recruiterBrief: "",
  focusNotes: "",
  focusConfirmedAt: null,
  needsFocusRefresh: false,
  roundType: "",
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

describe("SessionFocusGate round-type selector", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("pre-selects round type from inferred recruiter brief on blur", async () => {
    const { getFocusTagCatalog, getSessionFocus, inferRoundTypeFromBrief } =
      await import("../commands");
    vi.mocked(getSessionFocus).mockResolvedValue(emptyFocus());
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(inferRoundTypeFromBrief).mockResolvedValue("recruiter_screen");

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-focus-recruiter-brief")).toBeTruthy();
    });

    const brief = screen.getByTestId("session-focus-recruiter-brief");
    fireEvent.change(brief, {
      target: { value: "30 minute phone screen with our internal recruiter" },
    });
    fireEvent.blur(brief);

    await waitFor(() => {
      expect(inferRoundTypeFromBrief).toHaveBeenCalledWith(
        "30 minute phone screen with our internal recruiter",
      );
    });
    await waitFor(() => {
      expect(
        (screen.getByTestId("session-focus-round-type") as HTMLSelectElement).value,
      ).toBe("recruiter_screen");
    });
  });

  it("does not overwrite a manual round-type selection after brief edit", async () => {
    const { getFocusTagCatalog, getSessionFocus, inferRoundTypeFromBrief } =
      await import("../commands");
    vi.mocked(getSessionFocus).mockResolvedValue(emptyFocus());
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(inferRoundTypeFromBrief).mockResolvedValue("recruiter_screen");

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-focus-round-type")).toBeTruthy();
    });

    fireEvent.change(screen.getByTestId("session-focus-round-type"), {
      target: { value: "technical" },
    });
    expect(
      (screen.getByTestId("session-focus-round-type") as HTMLSelectElement).value,
    ).toBe("technical");

    const brief = screen.getByTestId("session-focus-recruiter-brief");
    fireEvent.change(brief, {
      target: { value: "This is a recruiter phone screen" },
    });
    fireEvent.blur(brief);

    await waitFor(() => {
      // Give the async blur handler a chance to run if it were going to call infer.
      expect(
        (screen.getByTestId("session-focus-round-type") as HTMLSelectElement).value,
      ).toBe("technical");
    });
    expect(inferRoundTypeFromBrief).not.toHaveBeenCalled();
  });

  it("includes roundType when saving session focus", async () => {
    const { getFocusTagCatalog, getSessionFocus, saveSessionFocus } = await import(
      "../commands"
    );
    vi.mocked(getSessionFocus).mockResolvedValue(emptyFocus());
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(saveSessionFocus).mockResolvedValue(undefined);

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-focus-round-type")).toBeTruthy();
    });

    fireEvent.change(screen.getByTestId("session-focus-round-type"), {
      target: { value: "hiring_manager" },
    });
    fireEvent.click(screen.getByTestId("focus-tag-chip-behavioral"));
    fireEvent.click(screen.getByTestId("session-focus-continue"));

    await waitFor(() => {
      expect(saveSessionFocus).toHaveBeenCalledWith(
        "s1",
        expect.objectContaining({
          roundType: "hiring_manager",
          focusTags: ["behavioral"],
        }),
      );
    });
  });
});

describe("SessionFocusGate next-round prep", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows debrief field and suggests the next round after a live session", async () => {
    const { getFocusTagCatalog, getSessionFocus } = await import("../commands");
    vi.mocked(getSessionFocus).mockResolvedValue({
      ...emptyFocus(),
      roundType: "recruiter_screen",
      needsFocusRefresh: true,
      focusTags: ["motivation"],
      focusConfirmedAt: 1,
    });
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-focus-next-round-banner")).toBeTruthy();
    });
    expect(screen.getByTestId("session-focus-debrief")).toBeTruthy();
    expect(
      (screen.getByTestId("session-focus-round-type") as HTMLSelectElement).value,
    ).toBe("technical");
  });

  it("ingests debrief before saving focus when debrief text is present", async () => {
    const { getFocusTagCatalog, getSessionFocus, ingestRoundDebrief, saveSessionFocus } =
      await import("../commands");
    vi.mocked(getSessionFocus).mockResolvedValue({
      ...emptyFocus(),
      roundType: "recruiter_screen",
      needsFocusRefresh: true,
    });
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(ingestRoundDebrief).mockResolvedValue({ chunksAdded: 2 });
    vi.mocked(saveSessionFocus).mockResolvedValue(undefined);

    render(<SessionFocusGate sessionId="s1" onComplete={() => {}} />);

    await waitFor(() => {
      expect(screen.getByTestId("session-focus-debrief")).toBeTruthy();
    });

    fireEvent.change(screen.getByTestId("session-focus-debrief"), {
      target: { value: "Jacob runs system design next week." },
    });
    fireEvent.click(screen.getByTestId("focus-tag-chip-technical"));
    fireEvent.click(screen.getByTestId("session-focus-continue"));

    await waitFor(() => {
      expect(ingestRoundDebrief).toHaveBeenCalledWith(
        "s1",
        "technical",
        "Jacob runs system design next week.",
      );
    });
    expect(saveSessionFocus).toHaveBeenCalledWith(
      "s1",
      expect.objectContaining({ needsFocusRefresh: false }),
    );
  });
});
