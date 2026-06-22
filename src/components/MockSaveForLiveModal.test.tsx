import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import MockSaveForLiveModal from "./MockSaveForLiveModal";

describe("MockSaveForLiveModal", () => {
  it("does not render when closed", () => {
    render(
      <MockSaveForLiveModal
        open={false}
        previewText="My answer"
        saving={false}
        saved={false}
        error={null}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />,
    );
    expect(screen.queryByTestId("mock-save-for-live-modal")).toBeNull();
  });

  it("shows read-only preview and confirm actions", () => {
    const onConfirm = vi.fn();
    render(
      <MockSaveForLiveModal
        open
        previewText="I led the IAM migration."
        saving={false}
        saved={false}
        error={null}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />,
    );

    expect(screen.getByTestId("mock-save-for-live-preview").textContent).toBe(
      "I led the IAM migration.",
    );
    fireEvent.click(screen.getByTestId("mock-save-for-live-confirm"));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("calls onCancel without saving", () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(
      <MockSaveForLiveModal
        open
        previewText="Draft answer"
        saving={false}
        saved={false}
        error={null}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />,
    );

    fireEvent.click(screen.getByTestId("mock-save-for-live-cancel"));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
