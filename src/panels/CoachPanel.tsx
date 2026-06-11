import type { CoachFeedback } from "../commands";

interface Props {
  feedback: CoachFeedback | null;
  isLoading: boolean;
  score: number;
}

const scoreColor = (score: number): string => {
  if (score >= 80) return "#22c55e";
  if (score >= 60) return "#f59e0b";
  return "#ef4444";
};

const CoachPanel = ({ feedback, isLoading, score }: Props) => {
  if (isLoading) {
    return (
      <div style={containerStyle}>
        <Header />
        <span style={{ color: "#52525b", fontSize: "12px" }}>Analyzing your answer…</span>
      </div>
    );
  }

  if (!feedback) {
    return (
      <div style={containerStyle}>
        <Header />
        <span style={{ color: "#52525b", fontSize: "12px" }}>Answer a question to get feedback.</span>
      </div>
    );
  }

  return (
    <div style={containerStyle}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <Header />
        <span
          style={{
            color: scoreColor(score),
            fontSize: "20px",
            fontWeight: 700,
          }}
        >
          {score}
        </span>
      </div>

      {feedback.tone.suggestion && (
        <Section label="TONE">
          <p style={bodyText}>
            <strong style={{ color: "#f59e0b" }}>{feedback.tone.assessment}</strong>
            {" — "}
            {feedback.tone.suggestion}
          </p>
        </Section>
      )}

      {feedback.context_gaps.length > 0 && (
        <Section label="GAPS">
          <ul style={{ margin: 0, paddingLeft: 16, ...bodyText }}>
            {feedback.context_gaps.map((gap, i) => (
              <li key={i} style={{ marginBottom: 4 }}>
                {gap}
              </li>
            ))}
          </ul>
        </Section>
      )}

      {feedback.grammar_issues.length > 0 && (
        <Section label="GRAMMAR">
          {feedback.grammar_issues.map((issue, i) => (
            <div key={i} style={{ marginBottom: 8 }}>
              <span style={{ color: "#ef4444", fontSize: "12px", textDecoration: "line-through" }}>
                {issue.original}
              </span>
              <span style={{ color: "#94a3b8", fontSize: "11px" }}> → </span>
              <span style={{ color: "#22c55e", fontSize: "12px" }}>{issue.fix}</span>
              {issue.why && (
                <p style={{ ...bodyText, color: "#64748b", marginTop: 2, marginBottom: 0 }}>
                  {issue.why}
                </p>
              )}
            </div>
          ))}
        </Section>
      )}

      {feedback.corrected_answer && (
        <Section label="POLISHED ANSWER">
          <p style={{ ...bodyText, color: "#a78bfa" }}>{feedback.corrected_answer}</p>
        </Section>
      )}
    </div>
  );
};

const Header = () => (
  <span style={{ color: "#f59e0b", fontSize: "11px", fontWeight: 600, letterSpacing: "0.06em" }}>
    COACH FEEDBACK
  </span>
);

const Section = ({ label, children }: { label: string; children: React.ReactNode }) => (
  <div style={{ marginTop: 10 }}>
    <div style={{ color: "#475569", fontSize: "10px", fontWeight: 600, marginBottom: 4, letterSpacing: "0.05em" }}>
      {label}
    </div>
    {children}
  </div>
);

const containerStyle: React.CSSProperties = {
  background: "#0f1117",
  border: "1px solid #92400e44",
  borderRadius: 8,
  padding: "12px 14px",
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const bodyText: React.CSSProperties = {
  margin: 0,
  color: "#cbd5e1",
  fontSize: "12px",
  lineHeight: 1.6,
};

export default CoachPanel;
