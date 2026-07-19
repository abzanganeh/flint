import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import InfoPopover from "./InfoPopover";

describe("InfoPopover", () => {
  it("shows help text when trigger is clicked", () => {
    render(
      <InfoPopover ariaLabel="Test help">
        <p>Hidden help content</p>
      </InfoPopover>,
    );

    expect(screen.queryByText("Hidden help content")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Test help" }));
    expect(screen.getByText("Hidden help content")).toBeTruthy();
  });
});
