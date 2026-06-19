import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import MicCalibration from "./MicCalibration";

vi.mock("../commands", () => ({
  getMicCalibrationStatus: vi.fn(),
  clearMicCalibration: vi.fn(),
  markMicCalibrationPassed: vi.fn(),
  runSystemAudioCalibration: vi.fn(),
  runMicCalibration: vi.fn(),
}));

describe("MicCalibration", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows skip gate when device already passed", async () => {
    const { getMicCalibrationStatus } = await import("../commands");
    vi.mocked(getMicCalibrationStatus).mockResolvedValue({
      passedOnDevice: true,
      deviceFingerprint: "abc",
      werSystem: 0.05,
      werMic: 0.1,
      forced: false,
      calibratedAt: 1,
    });

    render(<MicCalibration onComplete={vi.fn()} />);

    expect(await screen.findByTestId("mic-calibration-skip-gate")).toBeTruthy();
    expect(screen.getByTestId("mic-calibration-skip")).toBeTruthy();
  });

  it("starts system phase when not yet passed", async () => {
    const { getMicCalibrationStatus } = await import("../commands");
    vi.mocked(getMicCalibrationStatus).mockResolvedValue({
      passedOnDevice: false,
      deviceFingerprint: "abc",
      werSystem: null,
      werMic: null,
      forced: false,
      calibratedAt: null,
    });

    render(<MicCalibration onComplete={vi.fn()} />);

    expect(await screen.findByTestId("mic-calibration-active")).toBeTruthy();
    expect(screen.getByText(/Phase 1 — System audio/)).toBeTruthy();
  });

  it("calls onComplete when skip is chosen", async () => {
    const onComplete = vi.fn();
    const { getMicCalibrationStatus } = await import("../commands");
    vi.mocked(getMicCalibrationStatus).mockResolvedValue({
      passedOnDevice: true,
      deviceFingerprint: "abc",
      werSystem: 0.05,
      werMic: 0.1,
      forced: false,
      calibratedAt: 1,
    });

    render(<MicCalibration onComplete={onComplete} />);
    fireEvent.click(await screen.findByTestId("mic-calibration-skip"));
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
  });
});
