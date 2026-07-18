import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUIStore } from "../store/ui";
import VisualPanel from "./VisualPanel";

const mermaidRender = vi.fn();

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: (...args: unknown[]) => mermaidRender(...args),
  },
}));

const codeToHtml = vi.fn();

vi.mock("shiki", () => ({
  codeToHtml: (...args: unknown[]) => codeToHtml(...args),
}));

const triggerVisualResponse = vi.fn().mockResolvedValue(undefined);

vi.mock("../commands", () => ({
  copyTextToClipboard: vi.fn().mockResolvedValue(undefined),
  triggerVisualResponse: (...args: unknown[]) => triggerVisualResponse(...args),
}));

const setVisualBuffer = (text: string) => {
  useUIStore.setState((s) => ({
    streamingBuffers: { ...s.streamingBuffers, visual: text },
  }));
};

describe("VisualPanel", () => {
  beforeEach(() => {
    mermaidRender.mockReset();
    codeToHtml.mockReset();
    triggerVisualResponse.mockClear();
    triggerVisualResponse.mockResolvedValue(undefined);
    useUIStore.setState({
      streamingBuffers: { answer: "", visual: "" },
      depthPrePrepared: false,
      currentQuestion: "",
    });
  });

  it("shows a waiting placeholder when no visual text has streamed", () => {
    render(<VisualPanel />);

    expect(screen.getByText("Waiting for visual response…")).toBeTruthy();
  });

  it("shows a generating placeholder while awaiting the visual response", () => {
    render(<VisualPanel isGenerating />);

    expect(screen.getByText("Generating visual response…")).toBeTruthy();
  });

  it("renders a disabled Refine diagram button as a Tier 2b stub", () => {
    render(<VisualPanel />);

    const button = screen.getByTestId("refine-diagram-button");
    expect(button.hasAttribute("disabled")).toBe(true);
  });

  const diagramFixtures: Array<{ name: string; code: string }> = [
    { name: "flowchart", code: "flowchart TD\nA-->B" },
    { name: "graph", code: "graph LR\nA-->B" },
    { name: "sequenceDiagram", code: "sequenceDiagram\nAlice->>Bob: Hello" },
    { name: "classDiagram", code: "classDiagram\nClass01 <|-- Class02" },
    { name: "erDiagram", code: "erDiagram\nCUSTOMER ||--o{ ORDER : places" },
    { name: "stateDiagram-v2", code: "stateDiagram-v2\n[*] --> Still" },
    { name: "quadrantChart", code: 'quadrantChart\ntitle Reach and engagement' },
    { name: "timeline", code: "timeline\ntitle History of Social Media" },
    { name: "mindmap", code: "mindmap\nroot((mindmap))" },
    { name: "pie", code: 'pie title Pets\n"Dogs" : 50' },
    { name: "xychart-beta", code: 'xychart-beta\ntitle "Sales"' },
  ];

  it.each(diagramFixtures)(
    "renders $name diagrams via mermaid.render",
    async ({ code }) => {
      mermaidRender.mockResolvedValue({ svg: "<svg>rendered</svg>" });

      setVisualBuffer("```mermaid\n" + code + "\n```");
      render(<VisualPanel />);

      await waitFor(() => {
        expect(screen.getByTestId("visual-mermaid-svg")).toBeTruthy();
      });
      expect(mermaidRender).toHaveBeenCalledWith(
        expect.stringContaining("visual-mermaid-"),
        code,
      );
      expect(codeToHtml).not.toHaveBeenCalled();
    },
  );

  it("detects mermaid diagrams even without an explicit ```mermaid fence tag", async () => {
    mermaidRender.mockResolvedValue({ svg: "<svg>untagged</svg>" });

    setVisualBuffer("```\nflowchart TD\nA-->B\n```");
    render(<VisualPanel />);

    await waitFor(() => {
      expect(screen.getByTestId("visual-mermaid-svg")).toBeTruthy();
    });
  });

  it("falls back to raw text when mermaid fails to render invalid syntax", async () => {
    mermaidRender.mockRejectedValue(new Error("parse error"));

    setVisualBuffer("```mermaid\nflowchart TD\nA--\n```");
    render(<VisualPanel />);

    await waitFor(() => {
      expect(screen.getByTestId("visual-raw-fallback")).toBeTruthy();
    });
    expect(screen.getByTestId("visual-raw-fallback").textContent).toContain(
      "flowchart TD",
    );
    expect(triggerVisualResponse).not.toHaveBeenCalled();
  });

  it("auto-retries once on mermaid render failure then shows the diagram", async () => {
    mermaidRender
      .mockRejectedValueOnce(new Error("parse error"))
      .mockResolvedValue({ svg: "<svg>retried</svg>" });

    useUIStore.setState({
      currentQuestion: "Design a notification microservice.",
    });
    setVisualBuffer("```mermaid\nflowchart TD\nA--\n```");
    render(<VisualPanel sessionId="sess-1" />);

    await waitFor(() => {
      expect(triggerVisualResponse).toHaveBeenCalledTimes(1);
    });
    expect(triggerVisualResponse).toHaveBeenCalledWith(
      "Design a notification microservice.",
      "sess-1",
    );

    setVisualBuffer("```mermaid\nflowchart TD\nA-->B\n```");

    await waitFor(() => {
      expect(screen.getByTestId("visual-mermaid-svg")).toBeTruthy();
    });
    expect(screen.queryByTestId("visual-raw-fallback")).toBeNull();
  });

  it("falls back to raw text without looping when mermaid fails twice", async () => {
    mermaidRender.mockRejectedValue(new Error("parse error"));

    useUIStore.setState({
      currentQuestion: "Design a notification microservice.",
    });
    setVisualBuffer("```mermaid\nflowchart TD\nA--\n```");
    render(<VisualPanel sessionId="sess-1" />);

    await waitFor(() => {
      expect(triggerVisualResponse).toHaveBeenCalledTimes(1);
    });

    setVisualBuffer("```mermaid\nflowchart TD\nB--\n```");

    await waitFor(() => {
      expect(screen.getByTestId("visual-raw-fallback")).toBeTruthy();
    });
    expect(triggerVisualResponse).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("visual-raw-fallback").textContent).toContain(
      "flowchart TD",
    );
  });

  it("highlights non-diagram code fences via shiki", async () => {
    codeToHtml.mockResolvedValue("<pre><code>highlighted</code></pre>");

    setVisualBuffer('```python\nprint("hi")\n```');
    render(<VisualPanel />);

    await waitFor(() => {
      expect(screen.getByTestId("visual-code-block")).toBeTruthy();
    });
    expect(codeToHtml).toHaveBeenCalledWith(
      'print("hi")',
      expect.objectContaining({ lang: "python" }),
    );
    expect(mermaidRender).not.toHaveBeenCalled();
  });

  it("falls back to raw text when the stream has no fenced block yet", () => {
    setVisualBuffer("Here is some prose without a fence.");
    render(<VisualPanel />);

    expect(screen.getByTestId("visual-raw-fallback").textContent).toBe(
      "Here is some prose without a fence.",
    );
  });

  it("shows the pre-prepared badge when the cache served the visual answer", () => {
    codeToHtml.mockResolvedValue("<pre><code>highlighted</code></pre>");
    useUIStore.setState({ depthPrePrepared: true });
    setVisualBuffer('```python\nprint("hi")\n```');

    render(<VisualPanel />);

    expect(screen.getByText("pre-prepared")).toBeTruthy();
  });

  describe("manual visual trigger", () => {
    it("hides the Generate diagram button when no sessionId is provided (e.g. Rehearsal)", () => {
      useUIStore.setState({ currentQuestion: "Design a URL shortener." });

      render(<VisualPanel />);

      expect(screen.queryByTestId("generate-diagram-button")).toBeNull();
    });

    it("hides the Generate diagram button when there is no current question", () => {
      useUIStore.setState({ currentQuestion: "" });

      render(<VisualPanel sessionId="sess-1" />);

      expect(screen.queryByTestId("generate-diagram-button")).toBeNull();
    });

    it("hides the Generate diagram button while a response is generating", () => {
      useUIStore.setState({ currentQuestion: "Design a URL shortener." });

      render(<VisualPanel sessionId="sess-1" isGenerating />);

      expect(screen.queryByTestId("generate-diagram-button")).toBeNull();
    });

    it("forces a visual response for the current question when clicked", () => {
      useUIStore.setState({ currentQuestion: "Design a URL shortener." });

      render(<VisualPanel sessionId="sess-1" />);
      fireEvent.click(screen.getByTestId("generate-diagram-button"));

      expect(triggerVisualResponse).toHaveBeenCalledWith(
        "Design a URL shortener.",
        "sess-1",
      );
    });
  });
});
