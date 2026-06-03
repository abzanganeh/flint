import { useCallback, useEffect, useMemo, useState } from "react";

import {
  getHardwareProfile,
  runHealthCheck,
  type HardwareProfileDto,
  type HealthCheckName,
  type HealthCheckResultDto,
} from "../commands";
import "./HealthCheck.css";

const BLACKHOLE_URL = "https://existential.audio/blackhole/";
const PIPEWIRE_LOOPBACK_CMD = "pactl load-module module-loopback latency_msec=1";

const CHECK_LABELS: Record<HealthCheckName, string> = {
  microphone_access: "Microphone access",
  system_audio_loopback: "System audio loopback",
  rnnoise_preprocessing: "RNNoise preprocessing",
  whisper_model: "Whisper model",
  stealth_api: "Stealth mode",
  primary_llm: "Primary LLM",
  ollama_availability: "Ollama",
  os_keychain: "OS keychain",
  local_sqlite: "Local SQLite",
  supabase_connection: "Supabase connection",
  global_hotkey: "Global hotkey",
  panic_hotkey: "Panic hotkey",
};

type OsFamily = "linux" | "macos" | "windows" | "other";

function osFamily(os: string): OsFamily {
  const lower = os.toLowerCase();
  if (lower.includes("linux")) {
    return "linux";
  }
  if (lower.includes("macos") || lower.includes("darwin")) {
    return "macos";
  }
  if (lower.includes("windows")) {
    return "windows";
  }
  return "other";
}

function isX11StealthFailure(results: HealthCheckResultDto[]): boolean {
  const stealth = results.find((r) => r.check === "stealth_api");
  return (
    stealth?.status === "fail" &&
    stealth.message.toLowerCase().includes("x11")
  );
}

export interface HealthCheckProps {
  onComplete: () => void;
}

function PlatformAudioGuidance({ profile }: { profile: HardwareProfileDto }) {
  const family = osFamily(profile.os);

  if (family === "macos") {
    return (
      <div className="platform-banner info" role="note">
        BlackHole virtual audio driver is required for system audio capture.{" "}
        <a href={BLACKHOLE_URL} target="_blank" rel="noreferrer">
          Click here to install
        </a>
        , then create a multi-output device in Audio MIDI Setup.
      </div>
    );
  }

  if (family === "linux") {
    return (
      <div className="platform-banner info" role="note">
        PipeWire loopback is required. Run:{" "}
        <code>{PIPEWIRE_LOOPBACK_CMD}</code>
      </div>
    );
  }

  if (family === "windows") {
    return (
      <div className="platform-banner info" role="note">
        WASAPI loopback is supported natively on Windows.
      </div>
    );
  }

  return null;
}

function CheckRow({
  result,
  expanded,
  onToggle,
}: {
  result: HealthCheckResultDto;
  expanded: boolean;
  onToggle: () => void;
}) {
  const { status, message, fixInstruction } = result;
  const label = CHECK_LABELS[result.check];

  const icon =
    status === "pass" ? (
      <span className="check-icon pass" aria-hidden>
        ✓
      </span>
    ) : status === "warn" ? (
      <span className="check-icon warn" aria-hidden>
        ⚠
      </span>
    ) : (
      <span className="check-icon fail" aria-hidden>
        ✗
      </span>
    );

  const showWarnToggle = status === "warn" && fixInstruction;
  const showFailFix = status === "fail" && fixInstruction;
  const showWarnFix = status === "warn" && fixInstruction && expanded;

  const headerClass =
    showWarnToggle ? "check-item-header warn-toggle" : "check-item-header";

  return (
    <li className="check-item">
      {showWarnToggle ? (
        <button
          type="button"
          className={headerClass}
          onClick={onToggle}
          aria-expanded={expanded}
        >
          {icon}
          <span className="check-body">
            <span className="check-name">{label}</span>
            <span className="check-message">{message}</span>
          </span>
          <span className="check-expand-hint">{expanded ? "Hide" : "Fix"}</span>
        </button>
      ) : (
        <div className={headerClass}>
          {icon}
          <span className="check-body">
            <span className="check-name">{label}</span>
            <span className="check-message">{message}</span>
          </span>
        </div>
      )}
      {showFailFix ? (
        <p className="check-fix fail-fix" role="note">
          {fixInstruction}
        </p>
      ) : null}
      {showWarnFix ? (
        <p className="check-fix" role="note">
          {fixInstruction}
        </p>
      ) : null}
    </li>
  );
}

const HealthCheck = ({ onComplete }: HealthCheckProps) => {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [profile, setProfile] = useState<HardwareProfileDto | null>(null);
  const [results, setResults] = useState<HealthCheckResultDto[]>([]);
  const [expandedWarns, setExpandedWarns] = useState<Set<HealthCheckName>>(
    () => new Set(),
  );

  const runChecks = useCallback(async () => {
    setLoading(true);
    setError(null);
    setExpandedWarns(new Set());
    try {
      const [hardwareProfile, checkResults] = await Promise.all([
        getHardwareProfile(),
        runHealthCheck(),
      ]);
      setProfile(hardwareProfile);
      setResults(checkResults);
    } catch {
      setError("Health check could not complete. Please retry.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void runChecks();
  }, [runChecks]);

  const hasBlockingFailures = useMemo(
    () => results.some((r) => r.status === "fail"),
    [results],
  );

  const x11Warning = useMemo(() => isX11StealthFailure(results), [results]);

  const toggleWarn = (check: HealthCheckName) => {
    setExpandedWarns((prev) => {
      const next = new Set(prev);
      if (next.has(check)) {
        next.delete(check);
      } else {
        next.add(check);
      }
      return next;
    });
  };

  return (
    <div className="health-check" data-testid="health-check-panel">
      <div className="health-check-card">
        <h1 className="health-check-title">Installation health check</h1>
        <p className="health-check-subtitle">
          Flint verifies your device and environment before your first session.
        </p>

        {x11Warning ? (
          <div className="platform-banner danger" role="alert">
            <strong>Stealth mode requires Wayland.</strong> X11 is not supported.
            Screen capture exclusion cannot be guaranteed on X11 — switch to a
            Wayland session before starting a live session.
          </div>
        ) : null}

        {profile && !loading ? (
          <>
            <section className="hardware-profile" aria-labelledby="hardware-heading">
              <h2 id="hardware-heading">Your device</h2>
              <p className="hardware-tier">Tier {profile.tier}</p>
              <ul className="hardware-specs">
                <li>
                  {profile.cpuCores} CPU cores · {profile.ramGb.toFixed(1)} GB RAM
                  {profile.hasGpu
                    ? profile.gpuVramGb != null
                      ? ` · GPU (${profile.gpuVramGb.toFixed(1)} GB VRAM)`
                      : " · GPU detected"
                    : " · No GPU"}
                </li>
                <li>{profile.os}</li>
                <li>
                  Whisper: {profile.recommendedWhisperModel} · Directional:{" "}
                  {profile.recommendedLlmConfig.directional}
                </li>
                <li>Depth: {profile.recommendedLlmConfig.depth}</li>
              </ul>
            </section>
            <PlatformAudioGuidance profile={profile} />
          </>
        ) : null}

        {error ? (
          <p className="health-check-error" role="alert">
            {error}
          </p>
        ) : null}

        {loading ? (
          <div className="health-check-loading" aria-busy="true">
            <div className="spinner" aria-hidden />
            <p>Running health checks…</p>
          </div>
        ) : (
          <ul className="check-list">
            {results.map((result) => (
              <CheckRow
                key={result.check}
                result={result}
                expanded={expandedWarns.has(result.check)}
                onToggle={() => toggleWarn(result.check)}
              />
            ))}
          </ul>
        )}

        <div className="health-check-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => void runChecks()}
            disabled={loading}
          >
            Retry
          </button>
          <button
            type="button"
            className="btn-primary"
            disabled={loading || hasBlockingFailures}
            onClick={onComplete}
            data-testid="health-check-start"
          >
            Start anyway
          </button>
        </div>
      </div>
    </div>
  );
};

export default HealthCheck;
