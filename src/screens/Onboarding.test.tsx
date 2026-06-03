import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import Onboarding from "./Onboarding";

vi.mock("../commands", () => ({
  getLegalConsentAccepted: vi.fn().mockResolvedValue(false),
  getCurrentUser: vi.fn().mockRejectedValue(new Error("not logged in")),
  setLegalConsentAccepted: vi.fn().mockResolvedValue(undefined),
  signup: vi.fn(),
  login: vi.fn(),
  setSessionState: vi.fn().mockResolvedValue(undefined),
}));

describe("Onboarding legal consent", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("keeps Continue disabled until the checkbox is ticked", async () => {
    render(<Onboarding initialStep="legal" onComplete={() => undefined} />);

    const continueButton = await screen.findByTestId("legal-consent-continue");
    expect((continueButton as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId("legal-consent-checkbox"));
    expect((continueButton as HTMLButtonElement).disabled).toBe(false);
  });
});
