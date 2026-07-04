import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import SpeakerPicker from "./SpeakerPicker";

vi.mock("../commands", () => ({
  assignSpeaker: vi.fn(),
}));

describe("SpeakerPicker", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("calls assignSpeaker with session and speaker id", async () => {
    const { assignSpeaker } = await import("../commands");
    vi.mocked(assignSpeaker).mockResolvedValue(undefined);
    const onAssigned = vi.fn();

    render(
      <SpeakerPicker
        sessionId="sess-1"
        segments={[
          { speakerId: 0, sampleText: "Tell me about yourself" },
          { speakerId: 1, sampleText: "Sure, I led the team" },
        ]}
        onAssigned={onAssigned}
      />,
    );

    fireEvent.click(screen.getByTestId("speaker-pick-0"));

    await waitFor(() => {
      expect(assignSpeaker).toHaveBeenCalledWith("sess-1", 0);
      expect(onAssigned).toHaveBeenCalledTimes(1);
    });
  });
});
