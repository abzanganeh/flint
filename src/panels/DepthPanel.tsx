import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

import { onDepthToken } from "../events";
import { useUIStore } from "../store/ui";

// ── Component ────────────────────────────────────────────────────────────────

export interface DepthPanelProps {
  // Exposed for testing — in production the store drives state.
  isPrePrepared?: boolean;
}

const DepthPanel = ({ isPrePrepared = false }: DepthPanelProps) => {
  const { streamingBuffers, appendDepthToken, confidenceLevel } = useUIStore();

  const text = streamingBuffers.depth;

  // A response is pre-prepared if it was served from the pre-warm cache.
  // The Rust side emits a confidence_score with the ⚡ icon signal — we check
  // the prop override first (for tests), otherwise treat red confidence as
  // local fallback (not pre-prepared) and rely on the parent passing the flag.
  const showPrePreparedBadge =
    isPrePrepared ||
    (confidenceLevel === "green" && text.length > 0 && false); // placeholder; parent drives

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const fn = await onDepthToken(({ token }) => {
        appendDepthToken(token);
      });
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div
      data-testid="depth-panel"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
        backgroundColor: "#0f1117",
        fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
        fontSize: "13px",
      }}
    >
      {/* Header */}
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
            color: "#6b7280",
            fontSize: "11px",
            letterSpacing: "0.08em",
            textTransform: "uppercase",
          }}
        >
          Depth
        </span>
        {showPrePreparedBadge && (
          <span
            style={{
              fontSize: "10px",
              color: "#a78bfa",
              fontWeight: 600,
              letterSpacing: "0.04em",
            }}
          >
            ⚡ pre-prepared
          </span>
        )}
      </div>

      {/* Response text */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "10px 12px",
          color: "#e5e7eb",
          lineHeight: "1.65",
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
        }}
      >
        {text.length === 0 ? (
          <span
            style={{ color: "#4b5563", fontStyle: "italic", fontSize: "12px" }}
          >
            Waiting for depth response…
          </span>
        ) : (
          text
        )}
      </div>

      {/* Action button */}
      {text.length > 0 && (
        <div
          style={{
            padding: "6px 12px",
            borderTop: "1px solid #1e2028",
            flexShrink: 0,
          }}
        >
          <button
            onClick={() => void invoke("trigger_response")}
            style={{
              padding: "4px 10px",
              fontSize: "11px",
              fontWeight: 600,
              borderRadius: 4,
              border: "none",
              backgroundColor: "#7c3aed",
              color: "#fff",
              cursor: "pointer",
            }}
          >
            Use This Answer
          </button>
        </div>
      )}
    </div>
  );
};

export default DepthPanel;
