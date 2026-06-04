import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import WaylandCaptureHint from "./WaylandCaptureHint";

const STORAGE_KEY = "flint:wayland-capture-hint-dismissed";

const setUserAgent = (ua: string): void => {
  Object.defineProperty(window.navigator, "userAgent", {
    configurable: true,
    value: ua,
  });
};

describe("WaylandCaptureHint", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders on Linux when not yet dismissed", () => {
    setUserAgent("Mozilla/5.0 (X11; Linux x86_64)");
    render(<WaylandCaptureHint />);
    expect(screen.getByTestId("wayland-capture-hint")).toBeDefined();
  });

  it("does not render on non-Linux", () => {
    setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    const { container } = render(<WaylandCaptureHint />);
    expect(container.firstChild).toBeNull();
  });

  it("hides after dismiss and persists to localStorage", () => {
    setUserAgent("Mozilla/5.0 (X11; Linux x86_64)");
    render(<WaylandCaptureHint />);

    act(() => {
      fireEvent.click(screen.getByTestId("wayland-capture-hint-dismiss"));
    });

    expect(screen.queryByTestId("wayland-capture-hint")).toBeNull();
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe("1");
  });

  it("does not render when already dismissed", () => {
    setUserAgent("Mozilla/5.0 (X11; Linux x86_64)");
    window.localStorage.setItem(STORAGE_KEY, "1");
    const { container } = render(<WaylandCaptureHint />);
    expect(container.firstChild).toBeNull();
  });
});
