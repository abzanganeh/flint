import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import QuestionBank from "./QuestionBank";

vi.mock("../commands", () => ({
  addToQuestionBank: vi.fn(),
  getFocusTagCatalog: vi.fn(),
  getQuestionBank: vi.fn(),
  removeFromQuestionBank: vi.fn(),
  runRehearsalTurn: vi.fn(),
}));

const catalog = [
  { id: "self-assessment", label: "Self-assessment", description: "d", questionCount: 1 },
  { id: "motivation", label: "Motivation", description: "d", questionCount: 0 },
];

describe("QuestionBank add-question tag picker", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("adds a question with no tags by default (heuristic infer path unchanged)", async () => {
    const { addToQuestionBank, getFocusTagCatalog, getQuestionBank } = await import("../commands");
    vi.mocked(getQuestionBank).mockResolvedValue([]);
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(addToQuestionBank).mockResolvedValue([]);

    render(<QuestionBank sessionId="s1" />);

    await waitFor(() => {
      expect(screen.getByPlaceholderText("Add a question…")).toBeTruthy();
    });

    fireEvent.change(screen.getByPlaceholderText("Add a question…"), {
      target: { value: "New question?" },
    });
    fireEvent.click(screen.getByText("Add"));

    await waitFor(() => {
      expect(addToQuestionBank).toHaveBeenCalledWith("s1", "New question?", undefined);
    });
  });

  it("adds a question with explicit tags picked from the catalog", async () => {
    const { addToQuestionBank, getFocusTagCatalog, getQuestionBank } = await import("../commands");
    vi.mocked(getQuestionBank).mockResolvedValue([]);
    vi.mocked(getFocusTagCatalog).mockResolvedValue(catalog);
    vi.mocked(addToQuestionBank).mockResolvedValue([]);

    render(<QuestionBank sessionId="s1" />);

    await waitFor(() => {
      expect(screen.getByTestId("question-bank-tag-toggle")).toBeTruthy();
    });

    fireEvent.click(screen.getByTestId("question-bank-tag-toggle"));

    await waitFor(() => {
      expect(screen.getByTestId("question-bank-tag-chip-motivation")).toBeTruthy();
    });
    fireEvent.click(screen.getByTestId("question-bank-tag-chip-motivation"));

    fireEvent.change(screen.getByPlaceholderText("Add a question…"), {
      target: { value: "Why this role?" },
    });
    fireEvent.click(screen.getByText("Add"));

    await waitFor(() => {
      expect(addToQuestionBank).toHaveBeenCalledWith("s1", "Why this role?", ["motivation"]);
    });
  });
});
