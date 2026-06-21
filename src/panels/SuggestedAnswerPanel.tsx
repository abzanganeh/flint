interface Props {
  text: string;
  isStreaming: boolean;
}

const SuggestedAnswerPanel = ({ text, isStreaming }: Props) => {
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
        maxHeight: "40vh",
        overflowY: "auto",
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
        <p
          style={{
            margin: 0,
            color: "#e2e8f0",
            fontSize: "13px",
            lineHeight: 1.6,
            whiteSpace: "pre-wrap",
          }}
        >
          {text}
        </p>
      ) : (
        <span style={{ color: "#52525b", fontSize: "12px" }}>
          {isStreaming ? "Generating…" : "Waiting for question…"}
        </span>
      )}
    </div>
  );
};

export default SuggestedAnswerPanel;
