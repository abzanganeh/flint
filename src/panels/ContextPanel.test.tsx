import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUIStore } from "../store/ui";
import ContextPanel from "./ContextPanel";

vi.mock("../hooks/useRagChunks", () => ({
  useRagChunks: vi.fn(),
}));

describe("ContextPanel", () => {
  beforeEach(() => {
    useUIStore.setState({
      turnHistory: [],
      ragChunks: [],
      digestSummary: null,
    });
  });

  it("renders turn history in Earlier questions section", () => {
    useUIStore.setState({
      turnHistory: [
        {
          id: "turn-1",
          question: "Tell me about yourself",
          answer: "Brief answer",
          visual: "Longer visual answer",
          confidenceLevel: "green",
        },
      ],
    });

    render(<ContextPanel sessionId="sess-1" />);

    expect(screen.getByText("Earlier questions")).toBeTruthy();
    expect(screen.getAllByText("Tell me about yourself").length).toBe(2);
    expect(screen.getByText("Brief answer")).toBeTruthy();
    expect(screen.getByText("Longer visual answer")).toBeTruthy();
  });

  it("shows empty context when no chunks or history", () => {
    render(<ContextPanel sessionId="sess-1" />);

    expect(screen.getByText("No context chunks yet.")).toBeTruthy();
    expect(screen.queryByText("Earlier questions")).toBeNull();
  });
});
