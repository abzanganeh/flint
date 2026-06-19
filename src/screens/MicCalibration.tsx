import { useCallback, useEffect, useState } from "react";

import {
  clearMicCalibration,
  getMicCalibrationStatus,
  markMicCalibrationPassed,
  runMicCalibration,
  runSystemAudioCalibration,
  type CalibrationResultDto,
  type MicCalibrationStatusDto,
} from "../commands";

interface Props {
  onComplete: () => void;
  forceRetest?: boolean;
}

type Phase = "skip-gate" | "system" | "mic" | "failed" | "done";

const FAILURE_RECOMMENDATIONS = [
  "Use a headset or close-talk microphone",
  "Move to a quieter room",
  "Check that the correct mic is selected in Settings → Audio Devices",
  "If using a Bluetooth headset, switch to wired if possible",
];

export default function MicCalibration({ onComplete, forceRetest = false }: Props) {
  const [status, setStatus] = useState<MicCalibrationStatusDto | null>(null);
  const [phase, setPhase] = useState<Phase>("system");
  const [loading, setLoading] = useState(true);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [systemResult, setSystemResult] = useState<CalibrationResultDto | null>(null);
  const [micResult, setMicResult] = useState<CalibrationResultDto | null>(null);

  const loadStatus = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const s = await getMicCalibrationStatus();
      setStatus(s);
      if (forceRetest) {
        await clearMicCalibration();
        setStatus({ ...s, passedOnDevice: false });
        setPhase("system");
      } else if (s.passedOnDevice) {
        setPhase("skip-gate");
      } else {
        setPhase("system");
      }
    } catch (e) {
      setError(String(e));
      setPhase("system");
    } finally {
      setLoading(false);
    }
  }, [forceRetest]);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  const finishCalibration = async (forced: boolean) => {
    const werSystem = systemResult?.wer ?? status?.werSystem ?? 0;
    const werMic = micResult?.wer ?? status?.werMic ?? 0;
    await markMicCalibrationPassed(werSystem, werMic, forced);
    onComplete();
  };

  const runSystemPhase = async () => {
    setRunning(true);
    setError(null);
    try {
      const result = await runSystemAudioCalibration();
      setSystemResult(result);
      if (result.passed) {
        setPhase("mic");
      } else {
        setPhase("failed");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  const runMicPhase = async () => {
    setRunning(true);
    setError(null);
    try {
      const result = await runMicCalibration();
      setMicResult(result);
      if (result.passed) {
        setPhase("done");
        await finishCalibration(false);
      } else {
        setPhase("failed");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  if (loading) {
    return (
      <section className="mic-calibration" data-testid="mic-calibration-loading">
        <p>Loading audio calibration…</p>
      </section>
    );
  }

  if (phase === "skip-gate") {
    return (
      <section className="mic-calibration" data-testid="mic-calibration-skip-gate">
        <h2>Mic check</h2>
        <p>
          You&apos;ve already passed the mic check on this device. Is your setup the same?
        </p>
        <div className="mic-calibration-actions">
          <button type="button" onClick={() => setPhase("system")}>
            Run again
          </button>
          <button
            type="button"
            data-testid="mic-calibration-skip"
            onClick={() => void onComplete()}
          >
            Skip — nothing changed
          </button>
        </div>
      </section>
    );
  }

  if (phase === "failed") {
    return (
      <section className="mic-calibration mic-calibration-failed" data-testid="mic-calibration-failed">
        <div className="mic-calibration-warning" role="alert">
          <strong>Audio quality is too low for reliable transcription.</strong>
          <p>
            Flint cannot guarantee accurate coaching or matching during your interview.
          </p>
        </div>
        {systemResult && !systemResult.passed && (
          <p className="mic-calibration-wer">
            System audio WER: {(systemResult.wer * 100).toFixed(0)}% (threshold 20%)
          </p>
        )}
        {micResult && !micResult.passed && (
          <p className="mic-calibration-wer">
            Microphone WER: {(micResult.wer * 100).toFixed(0)}% (threshold 25%)
          </p>
        )}
        <ul>
          {FAILURE_RECOMMENDATIONS.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
        <button type="button" onClick={() => setPhase("system")}>
          Re-test
        </button>
        <button
          type="button"
          data-testid="mic-calibration-continue-anyway"
          onClick={() => void finishCalibration(true)}
        >
          I understand — continue anyway
        </button>
      </section>
    );
  }

  if (phase === "done") {
    return (
      <section className="mic-calibration" data-testid="mic-calibration-done">
        <h2>Calibration passed</h2>
        <p>
          If your transcription quality drops during a session, an amber badge labelled
          &quot;Mic quality low&quot; will appear in the bottom-right corner of your Flint overlay.
          Move to a quieter space or plug in a headset if you see it.
        </p>
      </section>
    );
  }

  const isSystem = phase === "system";

  return (
    <section className="mic-calibration" data-testid="mic-calibration-active">
      <h2>{isSystem ? "Phase 1 — System audio" : "Phase 2 — Microphone"}</h2>
      {isSystem ? (
        <p>
          Flint will play a short clip through your speakers and capture it via system audio
          loopback — the same path used in live sessions.
        </p>
      ) : (
        <>
          <p>Read this paragraph aloud at a natural pace:</p>
          <blockquote className="mic-calibration-paragraph">
            At SecureAuth, I led the design of an adaptive authentication system using ML-based
            risk scoring. The platform supported OAuth 2.0 and OIDC federation across multi-tenant
            SaaS customers. I integrated step-up MFA triggers with identity-aware policy
            enforcement — including Kerberos and LDAP for enterprise directories. My most recent
            work at IdMe24 focused on agentic AI identity: autonomous agents requiring just-in-time
            credential provisioning with zero-standing privilege.
          </blockquote>
        </>
      )}
      {systemResult && (
        <p className="mic-calibration-wer">
          System WER: {(systemResult.wer * 100).toFixed(0)}% —{" "}
          {systemResult.passed ? "passed" : "needs attention"}
        </p>
      )}
      {error && <p className="mic-calibration-error">{error}</p>}
      <button
        type="button"
        disabled={running}
        onClick={() => void (isSystem ? runSystemPhase() : runMicPhase())}
      >
        {running ? "Running…" : isSystem ? "Run system audio test" : "Start mic test"}
      </button>
    </section>
  );
}
