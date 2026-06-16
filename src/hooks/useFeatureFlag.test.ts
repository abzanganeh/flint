import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import { isFeatureEnabled } from "../commands";
import { useFeatureFlag } from "./useFeatureFlag";

vi.mock("../commands", () => ({
  isFeatureEnabled: vi.fn(),
}));

describe("useFeatureFlag", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("returns fallback until the backend resolves", () => {
    vi.mocked(isFeatureEnabled).mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => useFeatureFlag("post_session_summary", false));
    expect(result.current).toBe(false);
  });

  it("updates when the flag is enabled", async () => {
    vi.mocked(isFeatureEnabled).mockResolvedValue(true);
    const { result } = renderHook(() => useFeatureFlag("post_session_summary", false));
    await waitFor(() => {
      expect(result.current).toBe(true);
    });
    expect(isFeatureEnabled).toHaveBeenCalledWith("post_session_summary");
  });

  it("falls back when the backend call fails", async () => {
    vi.mocked(isFeatureEnabled).mockRejectedValue(new Error("offline"));
    const { result } = renderHook(() => useFeatureFlag("post_session_summary", false));
    await waitFor(() => {
      expect(result.current).toBe(false);
    });
  });
});
