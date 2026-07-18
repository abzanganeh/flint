import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const signalQuestionEnded = vi.fn();
const triggerResponse = vi.fn();
const cancelInference = vi.fn();
const panicHideOverlay = vi.fn();

vi.mock("../commands", () => ({
  signalQuestionEnded: (...args: unknown[]) => signalQuestionEnded(...args),
  triggerResponse: (...args: unknown[]) => triggerResponse(...args),
  cancelInference: (...args: unknown[]) => cancelInference(...args),
  panicHideOverlay: (...args: unknown[]) => panicHideOverlay(...args),
}));

vi.mock("../events", () => ({
  onHotkeyTrigger: () => Promise.resolve(() => undefined),
  onOverlayVisibility: () => Promise.resolve(() => undefined),
}));

vi.mock("../store/ui", () => ({
  useUIStore: (selector: (state: {
    setAnswerNowMode: () => void;
    setPanicHideActive: () => void;
    clearStreamingBuffers: () => void;
  }) => unknown) =>
    selector({
      setAnswerNowMode: vi.fn(),
      setPanicHideActive: vi.fn(),
      clearStreamingBuffers: vi.fn(),
    }),
}));

import { useHotkeys } from "./useHotkeys";

describe("useHotkeys Ctrl+Q path", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    signalQuestionEnded.mockResolvedValue(undefined);
  });

  it("routes Ctrl+Q to signalQuestionEnded exactly once", async () => {
    renderHook(() => useHotkeys("session-1", "prior question", true));

    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", {
          code: "KeyQ",
          ctrlKey: true,
          bubbles: true,
        }),
      );
    });

    expect(signalQuestionEnded).toHaveBeenCalledTimes(1);
    expect(signalQuestionEnded).toHaveBeenCalledWith("session-1");
    expect(triggerResponse).not.toHaveBeenCalled();
  });
});
