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
  phoneCallMode?: boolean;
}

type Phase = "skip-gate" | "system" | "mic" | "failed" | "done";

const FAILURE_RECOMMENDATIONS = [
  "Use a headset or close-talk microphone",
  "Move to a quieter room",
  "Check that the correct mic is selected in Settings → Audio Devices",
  "If using a Bluetooth headset, switch to wired if possible",
];

export default function MicCalibration({ onComplete, forceRetest = false, phoneCallMode = false }: Props) {
  const [status, setStatus] = useState<MicCalibrationStatusDto | null>(null);
  const [phase, setPhase] = useState<Phase>("system");
  const [loading, setLoading] = useState(true);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [systemResult, setSystemResult] = useState<CalibrationResultDto | null>(null);
  const [micResult, setMicResult] = useState<CalibrationResultDto | null>(null);
  // Show paragraph text before user starts mic test so they can read it first.
  const [micReady, setMicReady] = useState(false);

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
    // In phone-call mode skip system loopback test entirely — there is no
    // system audio to capture. Jump straight to the microphone phase.
    if (phoneCallMode) {
      setPhase("mic");
      setLoading(false);
      return;
    }
    void loadStatus();
  }, [loadStatus, phoneCallMode]);

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
      // Always move to mic phase — system audio failure is informational only.
      // The user can still calibrate their mic even if loopback doesn't work.
      setPhase("mic");
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
    const systemFailed = systemResult && !systemResult.passed;
    const micFailed = micResult && !micResult.passed;
    return (
      <section className="mic-calibration mic-calibration-failed" data-testid="mic-calibration-failed">
        <div className="mic-calibration-warning" role="alert">
          <strong>Audio quality is too low for reliable transcription.</strong>
          <p>
            Flint cannot guarantee accurate coaching or matching during your interview.
          </p>
        </div>
        {systemFailed && (
          <p className="mic-calibration-wer">
            System audio WER: {(systemResult.wer * 100).toFixed(0)}% (threshold 20%)
          </p>
        )}
        {micFailed && (
          <p className="mic-calibration-wer">
            Microphone WER: {(micResult.wer * 100).toFixed(0)}% (threshold 25%)
          </p>
        )}
        <ul>
          {FAILURE_RECOMMENDATIONS.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
        <div className="mic-calibration-actions">
          <button type="button" onClick={() => { setPhase("system"); setSystemResult(null); setMicResult(null); }}>
            Re-test
          </button>
          {systemFailed && !micFailed && (
            <button
              type="button"
              onClick={() => { setPhase("mic"); setMicReady(false); }}
            >
              Continue to mic test anyway
            </button>
          )}
          <button
            type="button"
            data-testid="mic-calibration-continue-anyway"
            onClick={() => void finishCalibration(true)}
          >
            I understand — continue anyway
          </button>
        </div>
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
      <h2>{isSystem ? "Phase 1 — System audio" : phoneCallMode ? "Microphone check" : "Phase 2 — Microphone"}</h2>
      {phoneCallMode && (
        <p className="mic-calibration-phone-banner">
          Phone interview mode is on. Put your phone on speaker near your laptop mic, then read the paragraph below.
        </p>
      )}
      {isSystem ? (
        <>
          <p>
            Flint will play a short clip through your speakers and capture it via system audio
            loopback — the same path used in live sessions.{" "}
            <strong>You do not need to speak during this step.</strong>
          </p>
          <details className="mic-calibration-details">
            <summary>How does Flint hear the interviewer?</summary>
            <div className="mic-calibration-details-body">
              <p>
                Flint listens to <strong>system audio output</strong> — whatever plays through
                your speakers or headset — not your microphone. During a phone screen or video
                call, the interviewer&apos;s voice plays through your headset, and Flint captures
                that audio to detect questions and generate responses.
              </p>
              <p>
                <strong>On Linux</strong> this uses a PipeWire monitor source (virtual loopback).
                Common issues:
              </p>
              <ul>
                <li>
                  <strong>Bluetooth headset:</strong> loopback capture through Bluetooth can fail
                  or return no audio. Try switching audio output to your laptop speakers for the
                  test, then back to your headset for the real session.
                </li>
                <li>
                  <strong>TTS not audible:</strong> if you don&apos;t hear the spoken clip,
                  espeak-ng may not be installed — run{" "}
                  <code>sudo apt install espeak-ng</code>.
                </li>
                <li>
                  <strong>PULSE_SOURCE:</strong> if the test still times out, run{" "}
                  <code>export PULSE_SOURCE=&quot;$(pactl get-default-sink).monitor&quot;</code>{" "}
                  in the terminal before launching Flint.
                </li>
              </ul>
              <p>
                <strong>Phone interview on mobile?</strong> Flint cannot capture a call taken on
                your phone. Use a softphone (e.g. Google Meet, Zoom, or a browser-based VoIP) on
                your computer so the audio routes through system output.
              </p>
            </div>
          </details>
        </>
      ) : (
        <>
          {!micReady ? (
            <>
              <p>Read the following paragraph aloud when you click <strong>Start mic test</strong>. Read it at a normal pace — you have 45 seconds.</p>
              <blockquote className="mic-calibration-paragraph">
                At SecureAuth, I led the design of an adaptive authentication system using ML-based
                risk scoring. The platform supported OAuth 2.0 and OIDC federation across multi-tenant
                SaaS customers. I integrated step-up MFA triggers with identity-aware policy
                enforcement — including Kerberos and LDAP for enterprise directories. My most recent
                work at IdMe24 focused on agentic AI identity: autonomous agents requiring just-in-time
                credential provisioning with zero-standing privilege.
              </blockquote>
              <p style={{ fontSize: "0.875rem", color: "#6b7280" }}>
                Read it once through — Flint will start listening as soon as you click the button below.
              </p>
            </>
          ) : (
            <>
              <p style={{ fontWeight: 600 }}>Recording — read the paragraph below now:</p>
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
        </>
      )}
      {systemResult && (
        <p className="mic-calibration-wer">
          System WER: {(systemResult.wer * 100).toFixed(0)}% —{" "}
          {systemResult.passed ? "passed" : "needs attention"}
        </p>
      )}
      {error && <p className="mic-calibration-error">{error}</p>}
      <div className="mic-calibration-actions">
        <button
          type="button"
          disabled={running}
          onClick={() => {
            if (isSystem) {
              void runSystemPhase();
            } else if (!micReady) {
              setMicReady(true);
              void runMicPhase();
            }
          }}
        >
          {running
            ? "Running…"
            : isSystem
              ? "Run system audio test"
              : "Start mic test — I'm ready to read"}
        </button>
        {isSystem && !running && (
          <button
            type="button"
            className="mic-calibration-skip-btn"
            data-testid="mic-calibration-skip-system"
            onClick={() => void finishCalibration(true)}
          >
            Skip — my audio route is non-standard
          </button>
        )}
      </div>
    </section>
  );
}
