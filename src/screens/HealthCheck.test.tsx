import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import type { HardwareProfileDto, HealthCheckResultDto } from "../commands";
import HealthCheck from "./HealthCheck";

const passResult = (
  check: HealthCheckResultDto["check"],
): HealthCheckResultDto => ({
  check,
  status: "pass",
  message: "OK",
  fixInstruction: null,
});

vi.mock("../commands", () => ({
  getHardwareProfile: vi.fn().mockResolvedValue({
    tier: 3,
    cpuCores: 8,
    ramGb: 16,
    hasGpu: false,
    gpuVramGb: null,
    os: "Linux Ubuntu 24.04",
    recommendedWhisperModel: "small.en",
    recommendedLlmConfig: {
      directional: "Ollama Llama 3.2 3B (local)",
      depth: "Ollama Llama 3.1 8B (local)",
      fallback: "Groq (cloud)",
      cloudRecommended: false,
    },
  } satisfies HardwareProfileDto),
  runHealthCheck: vi.fn(),
}));

describe("HealthCheck", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("disables Start anyway when any check fails", async () => {
    const { runHealthCheck } = await import("../commands");
    vi.mocked(runHealthCheck).mockResolvedValue([
      passResult("os_keychain"),
      {
        check: "stealth_api",
        status: "fail",
        message: "Stealth mode requires Wayland. X11 is not supported.",
        fixInstruction: "Switch to Wayland.",
      },
    ]);

    render(<HealthCheck onComplete={() => undefined} />);

    const startButton = await screen.findByTestId("health-check-start");
    expect((startButton as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("alert").textContent ?? "").toMatch(/X11 is not supported/i);
  });

  it("enables Start anyway when only warnings are present", async () => {
    const { runHealthCheck } = await import("../commands");
    vi.mocked(runHealthCheck).mockResolvedValue([
      passResult("os_keychain"),
      {
        check: "ollama_availability",
        status: "warn",
        message: "Ollama is not reachable.",
        fixInstruction: "Start Ollama.",
      },
    ]);

    render(<HealthCheck onComplete={() => undefined} />);

    const startButton = await screen.findByTestId("health-check-start");
    await waitFor(() => {
      expect((startButton as HTMLButtonElement).disabled).toBe(false);
    });
  });

  it("expands warn fix instructions on click", async () => {
    const { runHealthCheck } = await import("../commands");
    vi.mocked(runHealthCheck).mockResolvedValue([
      {
        check: "whisper_model",
        status: "warn",
        message: "Model missing.",
        fixInstruction: "Download the model.",
      },
    ]);

    render(<HealthCheck onComplete={() => undefined} />);

    await screen.findByText("Whisper model");
    expect(screen.queryByText("Download the model.")).toBeNull();

    fireEvent.click(screen.getByRole("button", { expanded: false }));
    expect(screen.getByText("Download the model.")).toBeTruthy();
  });
});
