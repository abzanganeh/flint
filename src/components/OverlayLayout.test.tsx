import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import OverlayLayout from "./OverlayLayout";
import { useUIStore } from "../store/ui";

const setViewport = (width: number, height: number): void => {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: width,
  });
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    value: height,
  });
  window.dispatchEvent(new Event("resize"));
};

const renderOverlay = () =>
  render(
    <OverlayLayout
      transcript={<div data-testid="slot-transcript">T</div>}
      directional={<div data-testid="slot-directional">D</div>}
      depth={<div data-testid="slot-depth">P</div>}
      clarifying={<div data-testid="slot-clarifying">C</div>}
      context={<div data-testid="slot-context">X</div>}
    />,
  );

const resetStore = (): void => {
  act(() => {
    useUIStore.setState({
      panelLayout: {
        sizes: {
          transcript: 1,
          directional: 1.5,
          depth: 1,
          clarifying: 0.75,
          context: 0.75,
        },
        collapsed: {
          transcript: false,
          directional: false,
          depth: false,
          clarifying: false,
          context: false,
        },
      },
      panicHideActive: false,
      overlayMinimised: false,
    });
  });
};

describe("OverlayLayout viewport rendering", () => {
  afterEach(() => {
    resetStore();
  });

  it("renders all five panels at 1920x1080", () => {
    setViewport(1920, 1080);
    renderOverlay();

    expect(screen.getByTestId("slot-transcript")).toBeDefined();
    expect(screen.getByTestId("slot-directional")).toBeDefined();
    expect(screen.getByTestId("slot-depth")).toBeDefined();
    expect(screen.getByTestId("slot-clarifying")).toBeDefined();
    expect(screen.getByTestId("slot-context")).toBeDefined();
  });

  it("renders all five panels at 2560x1440", () => {
    setViewport(2560, 1440);
    renderOverlay();

    expect(screen.getByTestId("slot-transcript")).toBeDefined();
    expect(screen.getByTestId("slot-directional")).toBeDefined();
    expect(screen.getByTestId("slot-depth")).toBeDefined();
    expect(screen.getByTestId("slot-clarifying")).toBeDefined();
    expect(screen.getByTestId("slot-context")).toBeDefined();
  });

  it("hides overlay when panicHideActive is true", () => {
    setViewport(1920, 1080);
    act(() => {
      useUIStore.getState().setPanicHideActive(true);
    });

    const { container } = renderOverlay();

    expect(container.firstChild).toBeNull();
  });

  it("collapses a panel slot when togglePanelCollapsed is called", () => {
    setViewport(1920, 1080);
    renderOverlay();

    act(() => {
      useUIStore.getState().togglePanelCollapsed("clarifying");
    });

    expect(useUIStore.getState().panelLayout.collapsed.clarifying).toBe(true);
    // Children stay mounted so orchestrator stream listeners are not dropped.
    expect(screen.getByTestId("slot-clarifying")).toBeDefined();
  });
});
