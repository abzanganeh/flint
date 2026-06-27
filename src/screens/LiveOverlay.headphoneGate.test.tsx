import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  transformCallback: vi.fn(() => 0),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

import LiveOverlay from "./LiveOverlay";

vi.mock("../components/PanicRestoreShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("../components/OverlayLayout", () => ({
  default: () => <div data-testid="overlay-layout-mock" />,
}));
vi.mock("../components/LiveSessionStatusBar", () => ({
  default: () => null,
}));
vi.mock("../components/SessionContextBadges", () => ({
  default: () => null,
}));
vi.mock("../components/TokenBudgetIndicator", () => ({
  default: () => null,
}));
vi.mock("../components/MicQualityBadge", () => ({
  default: () => null,
}));
vi.mock("../components/WaylandCaptureHint", () => ({
  default: () => null,
}));

vi.mock("../commands", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../commands")>();
  return {
    ...actual,
    getSessionSnapshot: vi.fn(),
    getHeadphoneGateStatus: vi.fn(),
    setHeadphoneGateOverride: vi.fn(),
    startSession: vi.fn(),
    stopSession: vi.fn(),
  };
});

vi.mock("../hooks/useAudioRoutingWarning", () => ({
  useAudioRoutingWarning: vi.fn(),
}));

vi.mock("../hooks/useLiveAudioWarning", () => ({
  useLiveAudioWarning: vi.fn(() => null),
}));

vi.mock("../hooks/useCostCap", () => ({
  useCostCap: vi.fn(),
}));

vi.mock("../hooks/useHotkeys", () => ({
  useHotkeys: vi.fn(),
}));

vi.mock("../hooks/useOrchestratorStreams", () => ({
  useOrchestratorStreams: vi.fn(),
}));

vi.mock("../hooks/useTokenUsage", () => ({
  useTokenUsage: vi.fn(),
}));

describe("LiveOverlay headphone gate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows gate UI when headphone check blocks live start", async () => {
    const { getSessionSnapshot, getHeadphoneGateStatus, startSession } = await import(
      "../commands"
    );
    vi.mocked(getSessionSnapshot).mockResolvedValue({
      state: "READY",
      phoneCallMode: false,
    } as never);
    vi.mocked(getHeadphoneGateStatus).mockResolvedValue({
      blocked: true,
      overridden: false,
      message: "Echo cancellation is not enabled.",
      fixInstruction: "Wear headphones.",
    });
    vi.mocked(startSession).mockResolvedValue();

    render(
      <LiveOverlay sessionId="sess-1" onEnded={vi.fn()} onReturnToSetup={vi.fn()} />,
    );

    expect(await screen.findByTestId("live-headphone-gate")).toBeTruthy();
    expect(screen.getByTestId("live-headphone-retry")).toBeTruthy();
    expect(screen.getByTestId("live-headphone-override")).toBeTruthy();
    expect(startSession).not.toHaveBeenCalled();
  });

  it("override clears gate and starts session", async () => {
    const {
      getSessionSnapshot,
      getHeadphoneGateStatus,
      setHeadphoneGateOverride,
      startSession,
    } = await import("../commands");
    vi.mocked(getSessionSnapshot).mockResolvedValue({
      state: "READY",
      phoneCallMode: false,
    } as never);
    vi.mocked(getHeadphoneGateStatus).mockResolvedValue({
      blocked: true,
      overridden: false,
      message: "Echo cancellation is not enabled.",
      fixInstruction: "Wear headphones.",
    });
    vi.mocked(setHeadphoneGateOverride).mockResolvedValue();
    vi.mocked(startSession).mockResolvedValue();

    render(
      <LiveOverlay sessionId="sess-1" onEnded={vi.fn()} onReturnToSetup={vi.fn()} />,
    );

    fireEvent.click(await screen.findByTestId("live-headphone-override"));
    await waitFor(() => expect(setHeadphoneGateOverride).toHaveBeenCalledWith(true));
    await waitFor(() => expect(startSession).toHaveBeenCalledWith("sess-1"));
    await waitFor(() =>
      expect(screen.queryByTestId("live-headphone-gate")).toBeNull(),
    );
  });

  it("skips gate check in phone call mode", async () => {
    const { getSessionSnapshot, getHeadphoneGateStatus, startSession } = await import(
      "../commands"
    );
    vi.mocked(getSessionSnapshot).mockResolvedValue({
      state: "READY",
      phoneCallMode: true,
    } as never);
    vi.mocked(getHeadphoneGateStatus).mockResolvedValue({
      blocked: true,
      overridden: false,
      message: "blocked",
      fixInstruction: null,
    });
    vi.mocked(startSession).mockResolvedValue();

    render(
      <LiveOverlay sessionId="sess-1" onEnded={vi.fn()} onReturnToSetup={vi.fn()} />,
    );

    await waitFor(() => expect(startSession).toHaveBeenCalled());
    expect(getHeadphoneGateStatus).not.toHaveBeenCalled();
  });
});
