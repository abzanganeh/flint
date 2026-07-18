import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUIStore } from "../store/ui";
import AnswerPanel from "./AnswerPanel";

const copyTextToClipboard = vi.fn().mockResolvedValue(undefined);
const rephraseResponse = vi.fn().mockResolvedValue(undefined);

vi.mock("../commands", () => ({
  copyTextToClipboard: (...args: unknown[]) => copyTextToClipboard(...args),
  rephraseResponse: (...args: unknown[]) => rephraseResponse(...args),
}));

const resetStore = (): void => {
  useUIStore.setState({
    streamingBuffers: { answer: "", visual: "" },
    confidenceLevel: null,
    answerNowMode: false,
    lastManualQuestion: "",
    currentQuestion: "",
  });
};

describe("AnswerPanel", () => {
  beforeEach(() => {
    copyTextToClipboard.mockClear();
    rephraseResponse.mockClear();
    resetStore();
  });

  it("shows a waiting placeholder when no answer text has streamed", () => {
    render(<AnswerPanel sessionId="sess-1" />);

    expect(screen.getByText("Waiting for response…")).toBeTruthy();
  });

  it("shows a generating placeholder while awaiting the answer", () => {
    render(<AnswerPanel sessionId="sess-1" isGenerating />);

    expect(screen.getByText("Generating response…")).toBeTruthy();
  });

  it("renders the current question heading and the streamed answer text", () => {
    useUIStore.setState({
      currentQuestion: "Tell me about yourself",
      streamingBuffers: { answer: "I am a software engineer.", visual: "" },
    });

    render(<AnswerPanel sessionId="sess-1" />);

    expect(screen.getByText("Tell me about yourself")).toBeTruthy();
    expect(screen.getByText("I am a software engineer.")).toBeTruthy();
  });

  it("shows the confidence label matching the current confidence level", () => {
    useUIStore.setState({
      streamingBuffers: { answer: "Some answer.", visual: "" },
      confidenceLevel: "green",
    });

    render(<AnswerPanel sessionId="sess-1" />);

    expect(screen.getByText("✓ Grounded")).toBeTruthy();
  });

  it("shows the Answer Now badge when answerNowMode is active", () => {
    useUIStore.setState({
      streamingBuffers: { answer: "Some answer.", visual: "" },
      answerNowMode: true,
    });

    render(<AnswerPanel sessionId="sess-1" />);

    expect(screen.getByText("Answer Now")).toBeTruthy();
  });

  it("does not render the action buttons row when there is no answer text yet", () => {
    render(<AnswerPanel sessionId="sess-1" />);

    expect(screen.queryByText("Answer This")).toBeNull();
    expect(screen.queryByText("Rephrase")).toBeNull();
  });

  it("copies the answer and enters Answer Now mode when Answer This is clicked", async () => {
    useUIStore.setState({
      streamingBuffers: { answer: "Copy me.", visual: "" },
    });

    render(<AnswerPanel sessionId="sess-1" />);
    fireEvent.click(screen.getByText("Answer This"));

    expect(copyTextToClipboard).toHaveBeenCalledWith("Copy me.");
    await waitFor(() => {
      expect(screen.getByText("Copied!")).toBeTruthy();
    });
    expect(useUIStore.getState().answerNowMode).toBe(true);
  });

  it("disables Rephrase when there is no last manual question", () => {
    useUIStore.setState({
      streamingBuffers: { answer: "Some answer.", visual: "" },
      lastManualQuestion: "",
    });

    render(<AnswerPanel sessionId="sess-1" />);

    const button = screen.getByText("Rephrase").closest("button");
    expect(button?.disabled).toBe(true);
  });

  it("disables Rephrase while a response is generating", () => {
    useUIStore.setState({
      streamingBuffers: { answer: "Some answer.", visual: "" },
      lastManualQuestion: "Tell me about yourself",
    });

    render(<AnswerPanel sessionId="sess-1" isGenerating />);

    const button = screen.getByText("Rephrase").closest("button");
    expect(button?.disabled).toBe(true);
  });

  it("clears buffers and triggers a rephrase when Rephrase is clicked", () => {
    useUIStore.setState({
      streamingBuffers: { answer: "Some answer.", visual: "leftover" },
      lastManualQuestion: "Tell me about yourself",
      confidenceLevel: "blue",
    });

    render(<AnswerPanel sessionId="sess-1" />);
    fireEvent.click(screen.getByText("Rephrase"));

    expect(rephraseResponse).toHaveBeenCalledWith(
      "Tell me about yourself",
      "sess-1",
    );
    const s = useUIStore.getState();
    expect(s.streamingBuffers.answer).toBe("");
    expect(s.streamingBuffers.visual).toBe("");
    expect(s.confidenceLevel).toBeNull();
  });
});
