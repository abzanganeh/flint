import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import LivePreview from "./LivePreview";

const cancelLivePreview = vi.fn();
const commitLivePreview = vi.fn();
const getSessionSnapshot = vi.fn();
const getRecordingConsentStatus = vi.fn();
const acceptRecordingConsent = vi.fn();

vi.mock("../commands", () => ({
  cancelLivePreview: (...args: unknown[]) => cancelLivePreview(...args),
  commitLivePreview: (...args: unknown[]) => commitLivePreview(...args),
  getSessionSnapshot: () => getSessionSnapshot(),
  getRecordingConsentStatus: (...args: unknown[]) => getRecordingConsentStatus(...args),
  acceptRecordingConsent: (...args: unknown[]) => acceptRecordingConsent(...args),
}));

vi.mock("../events", () => ({
  onSessionStateChange: () => Promise.resolve(() => undefined),
}));

vi.mock("../panels/TranscriptPanel", () => ({
  default: () => <div data-testid="transcript-panel-stub" />,
}));

describe("LivePreview", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getSessionSnapshot.mockResolvedValue({ phoneCallMode: false });
    getRecordingConsentStatus.mockResolvedValue({ accepted: true, acceptedAt: 1_700_000_000 });
    cancelLivePreview.mockResolvedValue(undefined);
    commitLivePreview.mockResolvedValue(undefined);
  });

  it("renders countdown and transcript panel", () => {
    render(
      <LivePreview sessionId="sess-1" onGoLive={vi.fn()} onBack={vi.fn()} />,
    );
    expect(screen.getByTestId("live-preview-screen")).toBeTruthy();
    expect(screen.getByTestId("live-preview-countdown").textContent).toBe("60s");
    expect(screen.getByTestId("transcript-panel-stub")).toBeTruthy();
  });

  it("shows TEST NOT LIVE watermark and pre-live warnings", () => {
    render(
      <LivePreview sessionId="sess-1" onGoLive={vi.fn()} onBack={vi.fn()} />,
    );
    expect(screen.getByTestId("live-preview-watermark").textContent).toMatch(
      /TEST — NOT LIVE/,
    );
    expect(screen.getByTestId("live-preview-mode-badge").textContent).toMatch(
      /Test — Not Live/i,
    );
    expect(screen.getByTestId("live-preview-warning").textContent).toMatch(
      /2 minutes before/i,
    );
  });

  it("invokes commitLivePreview on Go Live", async () => {
    const onGoLive = vi.fn();
    render(
      <LivePreview sessionId="sess-1" onGoLive={onGoLive} onBack={vi.fn()} />,
    );
    await waitFor(() => {
      expect(getRecordingConsentStatus).toHaveBeenCalled();
    });
    fireEvent.click(screen.getByTestId("live-preview-go-live-button"));
    await waitFor(() => {
      expect(commitLivePreview).toHaveBeenCalledWith("sess-1");
    });
    expect(onGoLive).toHaveBeenCalled();
  });

  it("invokes cancelLivePreview on Back", async () => {
    const onBack = vi.fn();
    render(
      <LivePreview sessionId="sess-1" onGoLive={vi.fn()} onBack={onBack} />,
    );
    fireEvent.click(screen.getByTestId("live-preview-back-button"));
    await waitFor(() => {
      expect(cancelLivePreview).toHaveBeenCalledWith("sess-1");
    });
    expect(onBack).toHaveBeenCalled();
  });
});
