import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import PreLiveTimingModal from "./PreLiveTimingModal";

describe("PreLiveTimingModal", () => {
  it("mentions starting live at least 2 minutes early", () => {
    render(<PreLiveTimingModal onDismiss={vi.fn()} />);
    expect(screen.getByTestId("pre-live-timing-modal").textContent).toMatch(
      /2 minutes before/i,
    );
  });

  it("calls onDismiss when Got it is clicked", () => {
    const onDismiss = vi.fn();
    render(<PreLiveTimingModal onDismiss={onDismiss} />);
    fireEvent.click(screen.getByTestId("pre-live-timing-dismiss"));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
