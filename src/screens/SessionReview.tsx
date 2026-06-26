import { useEffect, useMemo, useState } from "react";

import { getSessionReview, type ReviewChunkDto, type SessionReviewDto } from "../commands";

interface Props {
  sessionId: string;
  onBack: () => void;
}

export interface ReviewUtterance {
  speaker: "System" | "Microphone";
  text: string;
  corrected: boolean;
}

/**
 * Merge consecutive same-speaker chunks into readable utterances. Whisper emits
 * one chunk per VAD segment, so a raw chunk list is a flood of fragments; the
 * review screen wants paragraph-level turns.
 */
export function mergeReviewChunks(chunks: ReviewChunkDto[]): ReviewUtterance[] {
  const out: ReviewUtterance[] = [];
  for (const c of chunks) {
    const corrected = c.labelSource !== "channel";
    const last = out[out.length - 1];
    if (last && last.speaker === c.speaker) {
      last.text = `${last.text} ${c.text}`.trim();
      last.corrected = last.corrected || corrected;
    } else {
      out.push({ speaker: c.speaker, text: c.text.trim(), corrected });
    }
  }
  return out;
}

function transcriptToPlainText(utterances: ReviewUtterance[]): string {
  return utterances
    .map((u) => `${u.speaker === "System" ? "INTERVIEWER" : "YOU"}: ${u.text}`)
    .join("\n\n");
}

export function SessionReview({ sessionId, onBack }: Props) {
  const [review, setReview] = useState<SessionReviewDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    getSessionReview(sessionId)
      .then((r) => {
        if (!active) return;
        setReview(r);
        setLoading(false);
      })
      .catch((e: unknown) => {
        if (!active) return;
        setError(String(e));
        setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [sessionId]);

  const utterances = useMemo(
    () => (review ? mergeReviewChunks(review.transcript) : []),
    [review],
  );

  const micTurns = utterances.filter((u) => u.speaker === "Microphone").length;
  const singleChannel =
    review !== null && review.transcript.length > 0 && micTurns === 0;

  const handleCopy = () => {
    void navigator.clipboard
      ?.writeText(transcriptToPlainText(utterances))
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      })
      .catch(() => undefined);
  };

  return (
    <main className="sr-root" data-testid="session-review" style={rootStyle}>
      <header style={headerStyle}>
        <button type="button" onClick={onBack} style={backBtnStyle}>
          ← Back
        </button>
        <h1 style={{ fontSize: 18, margin: 0 }}>Session review</h1>
        <button
          type="button"
          onClick={handleCopy}
          disabled={utterances.length === 0}
          style={copyBtnStyle}
        >
          {copied ? "Copied" : "Copy transcript"}
        </button>
      </header>

      {loading && <p style={mutedStyle}>Loading transcript…</p>}

      {error && (
        <p style={{ ...mutedStyle, color: "#ef4444" }} data-testid="session-review-error">
          {error}
        </p>
      )}

      {!loading && !error && review && (
        <>
          <section style={metaRowStyle}>
            <Chip label={`${review.questionsCount} answered`} />
            <Chip label={`${review.directionalCount} directional`} />
            <Chip label={`${review.depthCount} depth`} />
            <Chip label={`${review.clarifyingCount} clarifying`} />
            <Chip label={review.state} />
          </section>

          {singleChannel && (
            <p style={noteStyle} data-testid="session-review-single-channel-note">
              This looks like a single-channel (phone) recording: every line was
              captured on one channel, so speaker labels are best-effort. Read for
              content rather than strict attribution.
            </p>
          )}

          {utterances.length === 0 ? (
            <p style={mutedStyle} data-testid="session-review-empty">
              No transcript was recorded for this session. If you expected audio
              here, the capture device was likely mis-routed during the live
              session.
            </p>
          ) : (
            <section style={transcriptStyle}>
              {utterances.map((u, i) => (
                <UtteranceRow key={i} utterance={u} />
              ))}
            </section>
          )}
        </>
      )}
    </main>
  );
}

const Chip = ({ label }: { label: string }) => (
  <span style={chipStyle}>{label}</span>
);

const UtteranceRow = ({ utterance }: { utterance: ReviewUtterance }) => {
  const isInterviewer = utterance.speaker === "System";
  return (
    <div
      data-testid="review-utterance"
      data-speaker={utterance.speaker}
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: isInterviewer ? "flex-start" : "flex-end",
        marginBottom: 12,
      }}
    >
      <span
        style={{
          fontSize: 10,
          fontWeight: 700,
          letterSpacing: "0.06em",
          textTransform: "uppercase",
          color: isInterviewer ? "#3b82f6" : "#22c55e",
          marginBottom: 2,
        }}
      >
        {isInterviewer ? "Interviewer" : "You"}
        {utterance.corrected && (
          <span style={{ marginLeft: 6, color: "#f59e0b", fontWeight: 500 }}>
            (auto)
          </span>
        )}
      </span>
      <span
        style={{
          color: "#e5e7eb",
          lineHeight: 1.5,
          maxWidth: "85%",
          textAlign: isInterviewer ? "left" : "right",
          wordBreak: "break-word",
          backgroundColor: isInterviewer ? "#161922" : "#13201a",
          borderRadius: 8,
          padding: "8px 12px",
        }}
      >
        {utterance.text}
      </span>
    </div>
  );
};

const rootStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  height: "100%",
  overflow: "hidden",
  padding: "16px 20px",
  color: "#e5e7eb",
  fontFamily: "'Inter', system-ui, sans-serif",
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 12,
  marginBottom: 12,
  flexShrink: 0,
};

const backBtnStyle: React.CSSProperties = {
  padding: "4px 12px",
  fontSize: 12,
  borderRadius: 4,
  border: "1px solid #374151",
  background: "transparent",
  color: "#9ca3af",
  cursor: "pointer",
};

const copyBtnStyle: React.CSSProperties = {
  padding: "4px 12px",
  fontSize: 12,
  borderRadius: 4,
  border: "1px solid #374151",
  background: "transparent",
  color: "#9ca3af",
  cursor: "pointer",
};

const metaRowStyle: React.CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  gap: 8,
  marginBottom: 12,
  flexShrink: 0,
};

const chipStyle: React.CSSProperties = {
  fontSize: 11,
  padding: "3px 10px",
  borderRadius: 999,
  border: "1px solid #2a2f3a",
  color: "#9ca3af",
  textTransform: "capitalize",
};

const noteStyle: React.CSSProperties = {
  fontSize: 12,
  color: "#fbbf24",
  backgroundColor: "#1a1400",
  border: "1px solid #2a2f3a",
  borderRadius: 8,
  padding: "8px 12px",
  marginBottom: 12,
  flexShrink: 0,
};

const transcriptStyle: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  paddingRight: 4,
};

const mutedStyle: React.CSSProperties = {
  color: "#6b7280",
  fontSize: 13,
};

export default SessionReview;
