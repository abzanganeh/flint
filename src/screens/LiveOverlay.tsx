import { useEffect, useLayoutEffect, useState } from "react";

import SessionContextBadges from "../components/SessionContextBadges";
import LiveSessionStatusBar from "../components/LiveSessionStatusBar";
import OverlayLayout from "../components/OverlayLayout";
import MicQualityBadge from "../components/MicQualityBadge";
import PanicRestoreShell from "../components/PanicRestoreShell";
import TokenBudgetIndicator from "../components/TokenBudgetIndicator";
import WaylandCaptureHint from "../components/WaylandCaptureHint";
import {
  getSessionSnapshot,
  startSession,
  stopSession,
} from "../commands";
import { onSessionStateChange } from "../events";
import { useCostCap } from "../hooks/useCostCap";
import { useHotkeys } from "../hooks/useHotkeys";
import { useOrchestratorStreams } from "../hooks/useOrchestratorStreams";
import { useTokenUsage } from "../hooks/useTokenUsage";
import DirectionalPanel from "../panels/DirectionalPanel";
import DepthPanel from "../panels/DepthPanel";
import ClarifyingPanel from "../panels/ClarifyingPanel";
import ContextPanel from "../panels/ContextPanel";
import TranscriptPanel from "../panels/TranscriptPanel";
import { useUIStore } from "../store/ui";
import { SessionState } from "../types";

export interface LiveOverlayProps {
  sessionId: string;
  onEnded: () => void;
  onReturnToSetup: () => void;
}

const START_TIMEOUT_MS = 45_000;

const LiveOverlay = ({ sessionId, onEnded, onReturnToSetup }: LiveOverlayProps) => {
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(true);
  const [exiting, setExiting] = useState(false);
  const lastManualQuestion = useUIStore((s) => s.lastManualQuestion);

  useTokenUsage();
  useCostCap();
  useOrchestratorStreams();
  useHotkeys(sessionId, lastManualQuestion, !starting);

  useLayoutEffect(() => {
    useUIStore.getState().resetOrchestratorPanels();
  }, [sessionId]);

  useEffect(() => {
    let active = true;
    const timeoutId = window.setTimeout(() => {
      if (!active) return;
      setError(
        "Live session is taking too long to start. Check audio/stealth health, then go back to setup.",
      );
      setStarting(false);
    }, START_TIMEOUT_MS);

    void (async () => {
      try {
        const snapshot = await getSessionSnapshot().catch(() => null);
        if (!active) return;
        if (snapshot?.state === SessionState.LIVE) {
          setStarting(false);
          return;
        }
        await startSession(sessionId);
        if (active) setStarting(false);
      } catch (e: unknown) {
        if (active) {
          setError(String(e));
          setStarting(false);
        }
      } finally {
        window.clearTimeout(timeoutId);
      }
    })();

    return () => {
      active = false;
      window.clearTimeout(timeoutId);
    };
  }, [sessionId]);

  useEffect(() => {
    let active = true;

    const unlistenPromise = onSessionStateChange(({ state }) => {
      if (!active) return;
      if (state === SessionState.ENDED) {
        onEnded();
      }
    });

    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [onEnded]);

  const handleReturnToSetup = async () => {
    if (exiting) return;
    setExiting(true);
    setError(null);
    try {
      const snapshot = await getSessionSnapshot().catch(() => null);
      if (snapshot?.state === SessionState.LIVE) {
        await stopSession().catch(() => undefined);
      }
    } finally {
      setExiting(false);
    }
    onReturnToSetup();
  };

  const handleStop = () => {
    void stopSession().catch((e: unknown) => setError(String(e)));
  };

  const toolbarButtonStyle = {
    padding: "4px 12px",
    fontSize: "11px",
    fontWeight: 600,
    borderRadius: 4,
    border: "1px solid #374151",
    backgroundColor: "transparent",
    color: "#9ca3af",
    cursor: "pointer",
  } as const;

  if (starting) {
    return (
      <main className="app-loading" data-testid="live-overlay-loading">
        <p>Starting live session…</p>
        <button
          type="button"
          data-testid="live-cancel-start-button"
          disabled={exiting}
          onClick={() => void handleReturnToSetup()}
          style={{ ...toolbarButtonStyle, marginTop: 16 }}
        >
          {exiting ? "Leaving…" : "Cancel — back to setup"}
        </button>
      </main>
    );
  }

  return (
    <PanicRestoreShell>
    <div
      data-testid="live-overlay"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        backgroundColor: "#0f1117",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "6px 12px",
          borderBottom: "1px solid #1e2028",
          flexShrink: 0,
          gap: 12,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 12, minWidth: 0, flex: 1 }}>
          <span
            style={{
              color: "#22c55e",
              fontSize: "11px",
              fontWeight: 700,
              letterSpacing: "0.08em",
              textTransform: "uppercase",
              flexShrink: 0,
            }}
          >
            Live
          </span>
          <SessionContextBadges sessionId={sessionId} />
        </div>
        <div style={{ display: "flex", gap: 8, flexShrink: 0 }}>
          <button
            type="button"
            data-testid="live-back-to-setup-button"
            disabled={exiting}
            onClick={() => void handleReturnToSetup()}
            style={toolbarButtonStyle}
          >
            {exiting ? "Leaving…" : "Back to setup"}
          </button>
          <button
            data-testid="stop-session-button"
            onClick={handleStop}
            style={toolbarButtonStyle}
          >
            End Session
          </button>
        </div>
      </div>

      {error && (
        <div
          style={{
            padding: "8px 12px",
            color: "#ef4444",
            fontSize: "12px",
            borderBottom: "1px solid #1e2028",
          }}
        >
          {error}
        </div>
      )}

      <WaylandCaptureHint />

      <div style={{ flex: 1, overflow: "hidden" }}>
        <OverlayLayout
          transcript={<TranscriptPanel />}
          directional={<DirectionalPanel sessionId={sessionId} />}
          depth={<DepthPanel />}
          clarifying={<ClarifyingPanel />}
          context={<ContextPanel sessionId={sessionId} />}
        />
      </div>

      <TokenBudgetIndicator />
      <LiveSessionStatusBar sessionId={sessionId} />
      <MicQualityBadge />
    </div>
    </PanicRestoreShell>
  );
};

export default LiveOverlay;
