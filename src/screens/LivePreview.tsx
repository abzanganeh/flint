import { useCallback, useEffect, useState } from "react";

import RecordingConsentModal from "../components/RecordingConsentModal";
import {
  acceptRecordingConsent,
  cancelLivePreview,
  commitLivePreview,
  getRecordingConsentStatus,
  getSessionSnapshot,
} from "../commands";
import { onSessionStateChange } from "../events";
import TranscriptPanel from "../panels/TranscriptPanel";
import { SessionState } from "../types";

const PREVIEW_SECONDS = 60;

export interface LivePreviewProps {
  sessionId: string;
  onGoLive: () => void;
  onBack: () => void;
}

const LivePreview = ({ sessionId, onGoLive, onBack }: LivePreviewProps) => {
  const [secondsLeft, setSecondsLeft] = useState(PREVIEW_SECONDS);
  const [busy, setBusy] = useState<"commit" | "cancel" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [phoneCallMode, setPhoneCallMode] = useState(false);
  const [consentAccepted, setConsentAccepted] = useState<boolean | null>(null);
  const [showConsentModal, setShowConsentModal] = useState(false);

  useEffect(() => {
    void getSessionSnapshot()
      .then((snapshot) => setPhoneCallMode(snapshot.phoneCallMode ?? false))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    void getRecordingConsentStatus(sessionId)
      .then((status) => {
        setConsentAccepted(status.accepted);
      })
      .catch(() => setConsentAccepted(false));
  }, [sessionId]);

  useEffect(() => {
    const id = window.setInterval(() => {
      setSecondsLeft((prev) => (prev > 0 ? prev - 1 : 0));
    }, 1000);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    let active = true;
    const unlistenPromise = onSessionStateChange(({ state }) => {
      if (!active) return;
      if (state === SessionState.READY || state === SessionState.REHEARSING) {
        onBack();
      }
      if (state === SessionState.LIVE) {
        onGoLive();
      }
    });
    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [onBack, onGoLive]);

  const commitLive = useCallback(async () => {
    setError(null);
    setBusy("commit");
    try {
      await commitLivePreview(sessionId);
      onGoLive();
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [onGoLive, sessionId]);

  const handleGoLive = useCallback(() => {
    if (consentAccepted === null) {
      return;
    }
    if (consentAccepted) {
      void commitLive();
      return;
    }
    setShowConsentModal(true);
  }, [commitLive, consentAccepted]);

  const handleConsentConfirm = useCallback(async () => {
    setBusy("commit");
    setError(null);
    try {
      await acceptRecordingConsent(sessionId);
      setConsentAccepted(true);
      setShowConsentModal(false);
      await commitLivePreview(sessionId);
      onGoLive();
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [onGoLive, sessionId]);

  const handleConsentCancel = useCallback(() => {
    setShowConsentModal(false);
    window.alert(
      "You must confirm recording consent before starting a live session. Flint cannot go live without it.",
    );
    void (async () => {
      setBusy("cancel");
      try {
        await cancelLivePreview(sessionId);
        onBack();
      } catch (e: unknown) {
        setError(String(e));
      } finally {
        setBusy(null);
      }
    })();
  }, [onBack, sessionId]);

  const handleBack = useCallback(async () => {
    setError(null);
    setBusy("cancel");
    try {
      await cancelLivePreview(sessionId);
      onBack();
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [onBack, sessionId]);

  return (
    <div
      data-testid="live-preview-screen"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "calc(100vh - 36px)",
        backgroundColor: "#1a1418",
        fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
        position: "relative",
        overflow: "hidden",
      }}
    >
      <div
        aria-hidden
        data-testid="live-preview-watermark"
        style={{
          position: "absolute",
          inset: 0,
          pointerEvents: "none",
          zIndex: 0,
          backgroundImage:
            "repeating-linear-gradient(-35deg, transparent 0, transparent 72px, rgba(248, 113, 113, 0.06) 72px, rgba(248, 113, 113, 0.06) 144px)",
        }}
      >
        <div
          style={{
            position: "absolute",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%) rotate(-18deg)",
            fontSize: "clamp(48px, 12vw, 96px)",
            fontWeight: 800,
            letterSpacing: "0.12em",
            color: "rgba(248, 113, 113, 0.14)",
            whiteSpace: "nowrap",
            userSelect: "none",
          }}
        >
          TEST — NOT LIVE
        </div>
      </div>

      {showConsentModal && (
        <RecordingConsentModal
          onConfirm={() => void handleConsentConfirm()}
          onCancel={handleConsentCancel}
        />
      )}

      <div
        style={{
          position: "relative",
          zIndex: 1,
          padding: "10px 16px",
          borderBottom: "1px solid #3f1d2b",
          display: "flex",
          flexDirection: "column",
          gap: 8,
          flexShrink: 0,
          backgroundColor: "rgba(26, 20, 24, 0.92)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <span
            data-testid="live-preview-mode-badge"
            style={{
              color: "#fb7185",
              fontSize: "11px",
              fontWeight: 700,
              letterSpacing: "0.08em",
              textTransform: "uppercase",
              border: "1px solid rgba(251, 113, 133, 0.45)",
              borderRadius: 4,
              padding: "2px 8px",
            }}
          >
            Test — Not Live
          </span>
          <span style={{ color: "#94a3b8", fontSize: "12px", flex: 1 }}>
            Hear your setup for up to {PREVIEW_SECONDS}s — no AI responses yet.
            {phoneCallMode
              ? " Phone mode: confirm speaker labels, then go live."
              : " Confirm audio routing, then go live."}
          </span>
          <span
            data-testid="live-preview-countdown"
            style={{
              color: secondsLeft <= 10 ? "#f59e0b" : "#e2e8f0",
              fontSize: "13px",
              fontWeight: 700,
              fontVariantNumeric: "tabular-nums",
            }}
          >
            {secondsLeft}s
          </span>
        </div>
        <p
          data-testid="live-preview-warning"
          style={{
            margin: 0,
            color: "#fda4af",
            fontSize: "12px",
            lineHeight: 1.45,
          }}
        >
          Start your live session at least <strong>2 minutes before</strong> your interview.
          After Go Live you will confirm recording consent and may see first-run tips — plan
          extra time. Use headphones unless you enabled phone-interview mode. Saved preferred
          answers apply only after you are fully live, not during this preview.
        </p>
      </div>

      {error && (
        <div
          data-testid="live-preview-error"
          style={{
            position: "relative",
            zIndex: 1,
            padding: "8px 16px",
            color: "#ef4444",
            fontSize: "12px",
            borderBottom: "1px solid #1e2028",
          }}
        >
          {error}
        </div>
      )}

      <div style={{ position: "relative", zIndex: 1, flex: 1, overflow: "hidden", minHeight: 0 }}>
        <TranscriptPanel sessionId={sessionId} />
      </div>

      <div
        style={{
          position: "relative",
          zIndex: 1,
          padding: "12px 16px",
          borderTop: "1px solid #3f1d2b",
          display: "flex",
          justifyContent: "space-between",
          gap: 12,
          flexShrink: 0,
          backgroundColor: "rgba(26, 20, 24, 0.92)",
        }}
      >
        <button
          type="button"
          data-testid="live-preview-back-button"
          disabled={busy !== null}
          onClick={() => void handleBack()}
          style={{
            padding: "8px 16px",
            backgroundColor: "transparent",
            color: "#94a3b8",
            border: "1px solid #374151",
            borderRadius: 6,
            fontSize: "13px",
            fontWeight: 600,
            cursor: busy ? "default" : "pointer",
          }}
        >
          {busy === "cancel" ? "Cancelling…" : "Back to Rehearsal"}
        </button>
        <button
          type="button"
          data-testid="live-preview-go-live-button"
          disabled={busy !== null || consentAccepted === null}
          onClick={() => void handleGoLive()}
          style={{
            padding: "8px 20px",
            backgroundColor: "#22c55e",
            color: "#fff",
            border: "none",
            borderRadius: 6,
            fontSize: "13px",
            fontWeight: 600,
            cursor: busy ? "default" : "pointer",
          }}
        >
          {busy === "commit" ? "Starting…" : "Go Live"}
        </button>
      </div>
    </div>
  );
};

export default LivePreview;
