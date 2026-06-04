import { rephraseResponse, triggerResponse } from "../commands";
import { useDirectionalStream } from "../hooks/useDirectionalStream";
import { useUIStore } from "../store/ui";
import type { ConfidenceLevel } from "../types";

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

export interface DirectionalPanelProps {
  sessionId: string;
}

const DirectionalPanel = ({ sessionId }: DirectionalPanelProps) => {
  useDirectionalStream();

  const {
    streamingBuffers,
    confidenceLevel,
    answerNowMode,
    lastManualQuestion,
  } = useUIStore();

  const text = streamingBuffers.directional;
  const borderColor =
    confidenceLevel != null
      ? CONFIDENCE_BORDER[confidenceLevel]
      : "transparent";
  const confidenceLabel =
    confidenceLevel != null ? CONFIDENCE_LABEL[confidenceLevel] : null;

  const handleTrigger = () => {
    if (!lastManualQuestion.trim()) return;
    void triggerResponse(lastManualQuestion, sessionId);
  };

  const handleRephrase = () => {
    if (!lastManualQuestion.trim()) return;
    void rephraseResponse(lastManualQuestion, sessionId);
  };

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
        ...(answerNowMode ? { fontSize: "16px" } : {}),
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
          <ActionButton label="Answer This" onClick={handleTrigger} />
          <ActionButton
            label="Rephrase"
            onClick={handleRephrase}
            secondary
          />
        </div>
      )}
    </div>
  );
};

interface ActionButtonProps {
  label: string;
  onClick: () => void;
  secondary?: boolean;
}

const ActionButton = ({
  label,
  onClick,
  secondary = false,
}: ActionButtonProps) => (
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
