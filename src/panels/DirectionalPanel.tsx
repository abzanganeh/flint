import { useState } from "react";

import { copyTextToClipboard, rephraseResponse } from "../commands";
import { useUIStore } from "../store/ui";
import type { ConfidenceLevel } from "../types";
import { HistoryCard, QuestionHeading } from "./TurnCards";

const clearBuffersForNewTurn = (): void => {
  const store = useUIStore.getState();
  store.clearStreamingBuffers();
  store.setAnswerNowMode(false);
  store.clearClarifyingQuestions();
  store.setConfidenceLevel(null);
};

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
  isGenerating?: boolean;
}

const DirectionalPanel = ({ sessionId, isGenerating = false }: DirectionalPanelProps) => {
  const {
    streamingBuffers,
    confidenceLevel,
    answerNowMode,
    lastManualQuestion,
    currentQuestion,
    turnHistory,
  } = useUIStore();

  const text = streamingBuffers.directional;
  const borderColor =
    confidenceLevel != null
      ? CONFIDENCE_BORDER[confidenceLevel]
      : "transparent";
  const confidenceLabel =
    confidenceLevel != null ? CONFIDENCE_LABEL[confidenceLevel] : null;

  const pushNotification = useUIStore((s) => s.pushNotification);
  const setAnswerNowMode = useUIStore((s) => s.setAnswerNowMode);
  const [copied, setCopied] = useState(false);

  const handleAnswerThis = () => {
    if (text.length === 0) return;
    setAnswerNowMode(true);
    void copyTextToClipboard(text)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      })
      .catch((err: unknown) => {
        pushNotification({
          id: crypto.randomUUID(),
          message: `Copy failed: ${String(err)}`,
          level: "error",
        });
      });
  };

  const handleRephrase = () => {
    if (!lastManualQuestion.trim() || isGenerating) return;
    clearBuffersForNewTurn();
    void rephraseResponse(lastManualQuestion, sessionId).catch((err: unknown) => {
      pushNotification({
        id: crypto.randomUUID(),
        message: String(err),
        level: "error",
      });
    });
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
        {answerNowMode && (
          <span
            style={{
              fontSize: "10px",
              color: "#22c55e",
              fontWeight: 600,
              marginLeft: 8,
            }}
          >
            Answer Now
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
        }}
      >
        {currentQuestion.length > 0 && (
          <QuestionHeading question={currentQuestion} />
        )}
        {text.length === 0 ? (
          <span
            style={{ color: "#4b5563", fontStyle: "italic", fontSize: "12px" }}
          >
            {isGenerating ? "Generating response…" : "Waiting for response…"}
          </span>
        ) : (
          <div style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
            {text}
          </div>
        )}

        {turnHistory.length > 0 && (
          <div style={{ marginTop: 16 }}>
            <div
              style={{
                color: "#4b5563",
                fontSize: "10px",
                letterSpacing: "0.08em",
                textTransform: "uppercase",
                borderTop: "1px solid #1e2028",
                paddingTop: 8,
                marginBottom: 6,
              }}
            >
              Earlier questions
            </div>
            {turnHistory.map((card) =>
              card.directional.length > 0 ? (
                <HistoryCard
                  key={card.id}
                  question={card.question}
                  answer={card.directional}
                />
              ) : null,
            )}
          </div>
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
          <ActionButton
            label={copied ? "Copied!" : "Answer This"}
            onClick={handleAnswerThis}
            title="Enlarge for reading and copy to clipboard (Ctrl+Alt+Space hold in live mode)"
          />
          <ActionButton
            label="Rephrase"
            onClick={handleRephrase}
            secondary
            disabled={isGenerating || !lastManualQuestion.trim()}
            title="Generate a new phrasing for the same question"
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
  disabled?: boolean;
  title?: string;
}

const ActionButton = ({
  label,
  onClick,
  secondary = false,
  disabled = false,
  title,
}: ActionButtonProps) => (
  <button
    type="button"
    onClick={onClick}
    disabled={disabled}
    title={title}
    style={{
      padding: "4px 10px",
      fontSize: "11px",
      fontWeight: 600,
      borderRadius: 4,
      border: secondary ? "1px solid #374151" : "none",
      backgroundColor: secondary ? "transparent" : disabled ? "#1e293b" : "#3b82f6",
      color: secondary ? (disabled ? "#4b5563" : "#9ca3af") : disabled ? "#6b7280" : "#fff",
      cursor: disabled ? "not-allowed" : "pointer",
      letterSpacing: "0.02em",
      opacity: disabled ? 0.6 : 1,
    }}
  >
    {label}
  </button>
);

export default DirectionalPanel;
