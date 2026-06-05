import { useEffect, useState } from "react";

import OverlayLayout from "../components/OverlayLayout";
import TokenBudgetIndicator from "../components/TokenBudgetIndicator";
import WaylandCaptureHint from "../components/WaylandCaptureHint";
import { startSession, stopSession } from "../commands";
import { onSessionStateChange } from "../events";
import { useCostCap } from "../hooks/useCostCap";
import { useHotkeys } from "../hooks/useHotkeys";
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
}

const LiveOverlay = ({ sessionId, onEnded }: LiveOverlayProps) => {
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(true);
  const lastManualQuestion = useUIStore((s) => s.lastManualQuestion);

  useTokenUsage();
  useCostCap();
  useHotkeys(sessionId, lastManualQuestion, !starting);

  useEffect(() => {
    let active = true;

    void startSession(sessionId)
      .then(() => {
        if (active) setStarting(false);
      })
      .catch((e: unknown) => {
        if (active) {
          setError(String(e));
          setStarting(false);
        }
      });

    return () => {
      active = false;
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

  const handleStop = () => {
    void stopSession().catch((e: unknown) => setError(String(e)));
  };

  if (starting) {
    return (
      <main className="app-loading" data-testid="live-overlay-loading">
        <p>Starting live session…</p>
      </main>
    );
  }

  return (
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
        }}
      >
        <span
          style={{
            color: "#22c55e",
            fontSize: "11px",
            fontWeight: 700,
            letterSpacing: "0.08em",
            textTransform: "uppercase",
          }}
        >
          Live
        </span>
        <button
          data-testid="stop-session-button"
          onClick={handleStop}
          style={{
            padding: "4px 12px",
            fontSize: "11px",
            fontWeight: 600,
            borderRadius: 4,
            border: "1px solid #374151",
            backgroundColor: "transparent",
            color: "#9ca3af",
            cursor: "pointer",
          }}
        >
          End Session
        </button>
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
    </div>
  );
};

export default LiveOverlay;
