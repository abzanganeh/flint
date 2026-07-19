import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Settings from "./Settings";

vi.mock("../commands", () => ({
  deleteAccount: vi.fn(),
  exportUserData: vi.fn(),
  getBillingStatus: vi.fn(),
  getCostStatus: vi.fn(),
  getFeatureFlagsSnapshot: vi.fn(),
  getFocusTagCatalog: vi.fn(),
  getSessionFocus: vi.fn(),
  getSessionSnapshot: vi.fn(),
  isFeatureEnabled: vi.fn(),
  liftCostSuspension: vi.fn(),
  logout: vi.fn(),
  refreshFeatureFlags: vi.fn(),
  resetCostTracker: vi.fn(),
  saveSessionFocus: vi.fn(),
  setCostCap: vi.fn(),
  setPhoneCallMode: vi.fn(),
  saveProviderKey: vi.fn(),
  isProviderKeyPresent: vi.fn(),
  clearProviderKey: vi.fn(),
  getPrimaryLlmProvider: vi.fn(),
  setPrimaryLlmProvider: vi.fn(),
}));

describe("Settings feature flags tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders flag snapshot and per-user resolution", async () => {
    const {
      getFeatureFlagsSnapshot,
      isFeatureEnabled,
      refreshFeatureFlags,
    } = await import("../commands");

    vi.mocked(getFeatureFlagsSnapshot).mockResolvedValue({
      origin: "defaults",
      fetchedAt: "2026-07-01T12:00:00Z",
      flagCount: 1,
      flags: [
        {
          name: "post_session_summary",
          enabled: true,
          allowed_plans: ["free", "premium"],
          rollout_percentage: 100,
          ga: true,
        },
      ],
    });
    vi.mocked(isFeatureEnabled).mockResolvedValue(true);
    vi.mocked(refreshFeatureFlags).mockResolvedValue(undefined);

    render(<Settings initialTab="features" />);

    await waitFor(() => {
      expect(screen.getByTestId("feature-flags-table")).toBeTruthy();
    });
    expect(screen.getByText("post_session_summary")).toBeTruthy();
    expect(screen.getByText("On")).toBeTruthy();

    fireEvent.click(screen.getByTestId("feature-flags-refresh"));
    expect(refreshFeatureFlags).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(getFeatureFlagsSnapshot).toHaveBeenCalledTimes(2);
    });
  });
});
