import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

import { onDirectionalToken, onConfidenceScore } from "../events";
import { useUIStore } from "../store/ui";
import type { ConfidenceLevel } from "../types";

// ── Confidence colour map ─────────────────────────────────────────────────────

const CONFIDENCE_BORDER: Record<ConfidenceLevel, string> = {
  green: "#22c55e",
  blue: "#3b82f6",
  amber: "#f59e0b",
  amber_low: "#f59e0b",
  grey: "#6b7280",
  red: "#ef4444",
};

const CONFIDENCE_LABEL: Record<ConfidenceLevel, string> = {
  green: "✓ Grounded",
  blue: "~ Partial",
  amber: "? Uncertain",
  amber_low: "? Limited",
  grey: "→ Clarify",
  red: "⚡ Local",
};

// ── Component ────────────────────────────────────────────────────────────────

export interface DirectionalPanelProps {}

const DirectionalPanel = (_props: DirectionalPanelProps) => {
  const {
    streamingBuffers,
    confidenceLevel,
    appendDirectionalToken,
    setConfidenceLevel,
    answerNowMode,
  } = useUIStore();

  const text = streamingBuffers.directional;
  const borderColor =
    confidenceLevel != null
      ? CONFIDENCE_BORDER[confidenceLevel]
      : "transparent";
  const confidenceLabel =
    confidenceLevel != null ? CONFIDENCE_LABEL[confidenceLevel] : null;

  useEffect(() => {
    let cancelled = false;
    let unlistenToken: (() => void) | null = null;
    let unlistenConf: (() => void) | null = null;

    const setup = async () => {
      const fnToken = await onDirectionalToken(({ token }) => {
        appendDirectionalToken(token);
      });
      const fnConf = await onConfidenceScore(({ level }) => {
        setConfidenceLevel(level);
      });
      if (cancelled) {
        fnToken();
        fnConf();
      } else {
        unlistenToken = fnToken;
        unlistenConf = fnConf;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlistenToken?.();
      unlistenConf?.();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div
      data-testid="directional-panel"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
        backgroundColor: "#0f1117",
        fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
        fontSize: "13px",
        borderLeft: `4px solid ${borderColor}`,
        // In answer-now mode the panel expands to fill full width; the layout
        // parent handles the sizing, we just increase font here.
        ...(answerNowMode ? { fontSize: "16px" } : {}),
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
          Directional
        </span>
        {confidenceLabel && (
          <span
            style={{
              fontSize: "10px",
              color: borderColor,
              fontWeight: 600,
            }}
          >
            {confidenceLabel}
          </span>
        )}
      </div>

      {/* Streaming text */}
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
            Waiting for response…
          </span>
        ) : (
          text
        )}
      </div>

      {/* Action buttons */}
      {text.length > 0 && (
        <div
          style={{
            display: "flex",
            gap: 6,
            padding: "6px 12px",
            borderTop: "1px solid #1e2028",
            flexShrink: 0,
          }}
        >
          <ActionButton
            label="Answer This"
            onClick={() => void invoke("trigger_response")}
          />
          <ActionButton
            label="Rephrase"
            onClick={() => void invoke("trigger_response")}
            secondary
          />
        </div>
      )}
    </div>
  );
};

// ── Action button ─────────────────────────────────────────────────────────────

interface ActionButtonProps {
  label: string;
  onClick: () => void;
  secondary?: boolean;
}

const ActionButton = ({ label, onClick, secondary = false }: ActionButtonProps) => (
  <button
    onClick={onClick}
    style={{
      padding: "4px 10px",
      fontSize: "11px",
      fontWeight: 600,
      borderRadius: 4,
      border: secondary ? "1px solid #374151" : "none",
      backgroundColor: secondary ? "transparent" : "#3b82f6",
      color: secondary ? "#9ca3af" : "#fff",
      cursor: "pointer",
      letterSpacing: "0.02em",
    }}
  >
    {label}
  </button>
);

export default DirectionalPanel;
