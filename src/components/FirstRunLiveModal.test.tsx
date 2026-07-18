import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import FirstRunLiveModal, {
  isFirstRunLiveModalDismissed,
} from "./FirstRunLiveModal";
import LiveHelpDrawer from "./LiveHelpDrawer";

describe("FirstRunLiveModal", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("lists Answer, Visual, and Context panel sections", () => {
    render(<FirstRunLiveModal onDismiss={vi.fn()} />);
    const modal = screen.getByTestId("first-run-live-modal");
    expect(modal.textContent).toContain("Answer");
    expect(modal.textContent).toContain("Visual");
    expect(modal.textContent).toContain("Context");
    expect(modal.textContent).toContain("Transcript");
  });

  it("persists dismiss flag when don't show again is checked", () => {
    const onDismiss = vi.fn();
    render(<FirstRunLiveModal onDismiss={onDismiss} />);
    fireEvent.click(screen.getByTestId("first-run-live-dont-show"));
    fireEvent.click(screen.getByTestId("first-run-live-dismiss"));
    expect(onDismiss).toHaveBeenCalled();
    expect(isFirstRunLiveModalDismissed()).toBe(true);
  });
});

describe("LiveHelpDrawer", () => {
  it("renders glossary sections when open", () => {
    render(<LiveHelpDrawer open onClose={vi.fn()} />);
    expect(screen.getByTestId("live-help-drawer").textContent).toContain(
      "Answer panel",
    );
    expect(screen.getByTestId("live-help-drawer").textContent).toContain(
      "Visual panel",
    );
  });

  it("calls onClose when backdrop is clicked", () => {
    const onClose = vi.fn();
    render(<LiveHelpDrawer open onClose={onClose} />);
    fireEvent.click(screen.getByTestId("live-help-drawer-backdrop"));
    expect(onClose).toHaveBeenCalled();
  });
});
