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
      answer={<div data-testid="slot-answer">A</div>}
      visual={<div data-testid="slot-visual">V</div>}
      context={<div data-testid="slot-context">X</div>}
    />,
  );

const resetStore = (): void => {
  act(() => {
    useUIStore.setState({
      panelLayout: {
        sizes: {
          transcript: 1,
          answer: 1.5,
          visual: 1.5,
          context: 1,
        },
        collapsed: {
          transcript: false,
          answer: false,
          visual: false,
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

  it("renders all four panels at 1920x1080", () => {
    setViewport(1920, 1080);
    renderOverlay();

    expect(screen.getByTestId("slot-transcript")).toBeDefined();
    expect(screen.getByTestId("slot-answer")).toBeDefined();
    expect(screen.getByTestId("slot-visual")).toBeDefined();
    expect(screen.getByTestId("slot-context")).toBeDefined();
  });

  it("renders all four panels at 2560x1440", () => {
    setViewport(2560, 1440);
    renderOverlay();

    expect(screen.getByTestId("slot-transcript")).toBeDefined();
    expect(screen.getByTestId("slot-answer")).toBeDefined();
    expect(screen.getByTestId("slot-visual")).toBeDefined();
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
      useUIStore.getState().togglePanelCollapsed("visual");
    });

    expect(useUIStore.getState().panelLayout.collapsed.visual).toBe(true);
    // Children stay mounted so orchestrator stream listeners are not dropped.
    expect(screen.getByTestId("slot-visual")).toBeDefined();
  });
});
