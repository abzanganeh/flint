import { useState } from "react";

interface Props {
  text: string;
  isStreaming: boolean;
}

const SuggestedAnswerPanel = ({ text, isStreaming }: Props) => {
  const [expanded, setExpanded] = useState(false);

  const previewLimit = 200;
  const isLong = text.length > previewLimit;
  const displayText =
    isLong && !expanded ? text.slice(0, previewLimit) + "…" : text;

  return (
    <div
      style={{
        background: "#0f1117",
        border: "1px solid #7c3aed44",
        borderRadius: 8,
        padding: "12px 14px",
        display: "flex",
        flexDirection: "column",
        gap: 8,
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <span
          style={{ color: "#a78bfa", fontSize: "11px", fontWeight: 600, letterSpacing: "0.06em" }}
        >
          SUGGESTED ANSWER
        </span>
        {isStreaming && (
          <span style={{ color: "#7c3aed", fontSize: "10px" }}>streaming…</span>
        )}
      </div>

      {text ? (
        <>
          <p
            style={{
              margin: 0,
              color: "#e2e8f0",
              fontSize: "13px",
              lineHeight: 1.6,
              whiteSpace: "pre-wrap",
            }}
          >
            {displayText}
          </p>
          {isLong && (
            <button
              onClick={() => setExpanded((v) => !v)}
              style={{
                background: "none",
                border: "none",
                color: "#7c3aed",
                fontSize: "12px",
                cursor: "pointer",
                padding: 0,
                alignSelf: "flex-start",
              }}
            >
              {expanded ? "Show less" : "Show full answer"}
            </button>
          )}
        </>
      ) : (
        <span style={{ color: "#52525b", fontSize: "12px" }}>
          {isStreaming ? "Generating…" : "Waiting for question…"}
        </span>
      )}
    </div>
  );
};

export default SuggestedAnswerPanel;
