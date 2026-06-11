import { useMemo } from "react";

import { useUIStore } from "../store/ui";

// ── Component ────────────────────────────────────────────────────────────────

export interface ClarifyingPanelProps {}

const ClarifyingPanel = (_props: ClarifyingPanelProps) => {
  const clarifyingQuestions = useUIStore((s) => s.clarifyingQuestions);

  // Dedupe for display (guards against stale HMR listeners or pre-fix state).
  const visibleQuestions = useMemo(() => {
    const seen = new Set<string>();
    return clarifyingQuestions.filter((q) => {
      const key = q.question.trim().toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }, [clarifyingQuestions]);

  return (
    <div
      data-testid="clarifying-panel"
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
          padding: "6px 12px",
          borderBottom: "1px solid #1e2028",
          color: "#6b7280",
          fontSize: "11px",
          letterSpacing: "0.08em",
          textTransform: "uppercase",
          flexShrink: 0,
        }}
      >
        Clarify
      </div>

      {/* Questions list */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "8px 0",
          display: "flex",
          flexDirection: "column",
          gap: "2px",
        }}
      >
        {visibleQuestions.length === 0 ? (
          <div
            style={{
              color: "#4b5563",
              padding: "16px 12px",
              fontStyle: "italic",
              fontSize: "12px",
            }}
          >
            No clarifying questions yet.
          </div>
        ) : (
          visibleQuestions.map((q) => (
            <ClarifyingRow
              key={q.id || `${q.rank}-${q.question}`}
              question={q.question}
              rank={q.rank}
            />
          ))
        )}
      </div>
    </div>
  );
};

// ── Row ───────────────────────────────────────────────────────────────────────

interface ClarifyingRowProps {
  question: string;
  rank: number;
}

const ClarifyingRow = ({ question, rank }: ClarifyingRowProps) => (
  <div
    style={{
      display: "flex",
      alignItems: "flex-start",
      gap: 8,
      padding: "6px 12px",
      borderBottom: "1px solid #1a1d26",
    }}
  >
    <span
      style={{
        color: "#6b7280",
        fontSize: "10px",
        fontWeight: 700,
        minWidth: 14,
        paddingTop: 2,
      }}
    >
      {rank}
    </span>
    <span
      style={{
        color: "#d1d5db",
        lineHeight: "1.55",
        fontSize: "12px",
        wordBreak: "break-word",
      }}
    >
      {question}
    </span>
  </div>
);

export default ClarifyingPanel;
