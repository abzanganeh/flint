import { useRagChunks } from "../hooks/useRagChunks";
import { useUIStore } from "../store/ui";

export interface ContextPanelProps {
  sessionId: string;
}

const ContextPanel = ({ sessionId }: ContextPanelProps) => {
  useRagChunks(sessionId);
  const { ragChunks, digestSummary } = useUIStore();

  return (
    <div
      data-testid="context-panel"
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
          padding: "6px 12px",
          borderBottom: "1px solid #1e2028",
          color: "#6b7280",
          fontSize: "11px",
          letterSpacing: "0.08em",
          textTransform: "uppercase",
          flexShrink: 0,
        }}
      >
        Context
      </div>

      {digestSummary && (
        <div
          style={{
            padding: "8px 12px",
            borderBottom: "1px solid #1a1d26",
            color: "#9ca3af",
            fontSize: "11px",
            lineHeight: 1.5,
            flexShrink: 0,
          }}
        >
          {digestSummary}
        </div>
      )}

      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "8px 0",
          display: "flex",
          flexDirection: "column",
          gap: "4px",
        }}
      >
        {ragChunks.length === 0 ? (
          <div
            style={{
              color: "#4b5563",
              padding: "16px 12px",
              fontStyle: "italic",
              fontSize: "12px",
            }}
          >
            No context chunks yet.
          </div>
        ) : (
          ragChunks.map((chunk, i) => (
            <ChunkRow key={i} text={chunk.text} score={chunk.score} />
          ))
        )}
      </div>
    </div>
  );
};

interface ChunkRowProps {
  text: string;
  score: number;
}

const ChunkRow = ({ text, score }: ChunkRowProps) => {
  const pct = Math.round(score * 100);
  const barColor =
    score >= 0.75 ? "#22c55e" : score >= 0.5 ? "#3b82f6" : "#f59e0b";

  return (
    <div
      style={{
        padding: "6px 12px",
        borderBottom: "1px solid #1a1d26",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          marginBottom: 4,
        }}
      >
        <div
          style={{
            flex: 1,
            height: 3,
            borderRadius: 2,
            backgroundColor: "#1e2028",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              width: `${pct}%`,
              height: "100%",
              backgroundColor: barColor,
              borderRadius: 2,
            }}
          />
        </div>
        <span
          style={{
            color: barColor,
            fontSize: "10px",
            fontWeight: 600,
            minWidth: 28,
          }}
        >
          {pct}%
        </span>
      </div>
      <p
        style={{
          margin: 0,
          color: "#9ca3af",
          fontSize: "11px",
          lineHeight: "1.5",
          wordBreak: "break-word",
          display: "-webkit-box",
          WebkitLineClamp: 3,
          WebkitBoxOrient: "vertical",
          overflow: "hidden",
        }}
      >
        {text}
      </p>
    </div>
  );
};

export default ContextPanel;
