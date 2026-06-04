import { useDepthStream } from "../hooks/useDepthStream";
import { useUIStore } from "../store/ui";

export interface DepthPanelProps {}

const DepthPanel = (_props: DepthPanelProps) => {
  useDepthStream();

  const { streamingBuffers, depthPrePrepared } = useUIStore();
  const text = streamingBuffers.depth;

  const sections = text
    .split(/\n(?=\d+\.|[-*]|\*\*)/)
    .map((s) => s.trim())
    .filter(Boolean);

  const handleCopy = () => {
    if (text.length === 0) return;
    void navigator.clipboard.writeText(text);
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
        {text.length === 0 ? (
          <span
            style={{ color: "#4b5563", fontStyle: "italic", fontSize: "12px" }}
          >
            Waiting for depth response…
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
            onClick={handleCopy}
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
