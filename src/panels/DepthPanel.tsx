import { useState } from "react";

import { copyTextToClipboard } from "../commands";
import { useUIStore } from "../store/ui";
import { HistoryCard, QuestionHeading } from "./TurnCards";

export interface DepthPanelProps {
  isGenerating?: boolean;
}

const DepthPanel = ({ isGenerating = false }: DepthPanelProps) => {
  const { streamingBuffers, depthPrePrepared, currentQuestion, turnHistory } =
    useUIStore();
  const pushNotification = useUIStore((s) => s.pushNotification);
  const [copied, setCopied] = useState(false);
  const text = streamingBuffers.depth;

  const sections = text
    .split(/\n(?=\d+\.|[-*]|\*\*)/)
    .map((s) => s.trim())
    .filter(Boolean);

  const handleUseAnswer = () => {
    if (text.length === 0) return;
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
        {depthPrePrepared && text.length > 0 && (
          <span
            style={{
              fontSize: "10px",
              color: "#a78bfa",
              fontWeight: 600,
              letterSpacing: "0.04em",
            }}
          >
            pre-prepared
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
            {isGenerating
              ? "Generating depth response…"
              : "Waiting for depth response…"}
          </span>
        ) : sections.length > 1 ? (
          sections.map((section, i) => (
            <p
              key={i}
              style={{
                margin: "0 0 10px",
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {section}
            </p>
          ))
        ) : (
          <p
            style={{
              margin: 0,
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {text}
          </p>
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
              card.depth.length > 0 ? (
                <HistoryCard
                  key={card.id}
                  question={card.question}
                  answer={card.depth}
                />
              ) : null,
            )}
          </div>
        )}
      </div>

      {text.length > 0 && (
        <div
          style={{
            padding: "6px 12px",
            borderTop: "1px solid #1e2028",
            flexShrink: 0,
          }}
        >
          <button
            type="button"
            onClick={handleUseAnswer}
            title="Copy the full depth answer to your clipboard"
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
            {copied ? "Copied!" : "Use This Answer"}
          </button>
        </div>
      )}
    </div>
  );
};

export default DepthPanel;
