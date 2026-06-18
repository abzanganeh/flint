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
  startGoogleOAuth: vi.fn().mockResolvedValue(undefined),
  cancelGoogleOAuth: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../events", () => ({
  onAuthOAuthComplete: vi.fn().mockResolvedValue(() => undefined),
  onAuthOAuthError: vi.fn().mockResolvedValue(() => undefined),
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

describe("Onboarding Google OAuth", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows cancel while waiting for Google and calls cancel_google_oauth", async () => {
    const { startGoogleOAuth, cancelGoogleOAuth } = await import("../commands");
    render(<Onboarding initialStep="auth" onComplete={() => undefined} />);

    fireEvent.click(screen.getByTestId("google-sign-in-button"));
    expect(await screen.findByText("Waiting for Google…")).toBeTruthy();

    const cancelButton = screen.getByTestId("google-sign-in-cancel");
    fireEvent.click(cancelButton);

    expect(startGoogleOAuth).toHaveBeenCalledTimes(1);
    expect(cancelGoogleOAuth).toHaveBeenCalledTimes(1);
  });
});
