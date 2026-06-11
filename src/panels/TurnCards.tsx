/**
 * Shared presentation pieces for per-question answer cards in the
 * Directional and Depth panels. The live answer renders under a question
 * heading; completed turns collapse into <details> cards so the panel stays
 * scannable mid-interview instead of growing into a wall of text.
 */

export interface QuestionHeadingProps {
  question: string;
}

export const QuestionHeading = ({ question }: QuestionHeadingProps) => (
  <div
    style={{
      color: "#93c5fd",
      fontSize: "12px",
      fontWeight: 600,
      lineHeight: "1.4",
      marginBottom: 8,
      paddingBottom: 6,
      borderBottom: "1px dashed #1e2028",
    }}
  >
    {question}
  </div>
);

export interface HistoryCardProps {
  question: string;
  answer: string;
}

export const HistoryCard = ({ question, answer }: HistoryCardProps) => (
  <details
    style={{
      marginBottom: 6,
      borderRadius: 4,
      backgroundColor: "#151823",
      border: "1px solid #1e2028",
    }}
  >
    <summary
      style={{
        padding: "5px 8px",
        cursor: "pointer",
        color: "#9ca3af",
        fontSize: "11px",
        fontWeight: 600,
        lineHeight: "1.4",
        listStylePosition: "inside",
      }}
    >
      {question.length > 0 ? question : "Earlier answer"}
    </summary>
    <div
      style={{
        padding: "6px 10px 8px",
        color: "#9ca3af",
        fontSize: "12px",
        lineHeight: "1.55",
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
        borderTop: "1px solid #1e2028",
      }}
    >
      {answer}
    </div>
  </details>
);
