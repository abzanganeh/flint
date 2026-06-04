import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import OverlayLayout from "../components/OverlayLayout";
import TokenBudgetIndicator from "../components/TokenBudgetIndicator";
import DirectionalPanel from "../panels/DirectionalPanel";
import DepthPanel from "../panels/DepthPanel";
import ClarifyingPanel from "../panels/ClarifyingPanel";
import ContextPanel from "../panels/ContextPanel";
import TranscriptPanel from "../panels/TranscriptPanel";
import { useUIStore } from "../store/ui";

// ── Types ─────────────────────────────────────────────────────────────────────

export interface RehearsalProps {
  sessionId: string;
  onComplete: () => void;
}

// ── Component ────────────────────────────────────────────────────────────────

const Rehearsal = ({ sessionId, onComplete }: RehearsalProps) => {
  const [question, setQuestion] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { streamingBuffers, clearStreamingBuffers, clearClarifyingQuestions } =
    useUIStore();

  const hasResponse =
    streamingBuffers.directional.length > 0 ||
    streamingBuffers.depth.length > 0;

  const handleSubmit = async () => {
    if (!question.trim()) return;
    setError(null);
    clearStreamingBuffers();
    clearClarifyingQuestions();

    try {
      // Use the real start_session + trigger_response path so the rehearsal
      // exercises the full orchestrator stack, not a separate mock.
      await invoke("start_session", { sessionId });
      await invoke("trigger_response");
      setSubmitted(true);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleComplete = async () => {
    try {
      await invoke("stop_session");
    } catch {
      // Session may already be stopped; proceed regardless.
    }
    onComplete();
  };

  return (
    <div
      data-testid="rehearsal-screen"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        backgroundColor: "#0f1117",
        fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
      }}
    >
      {/* Top bar */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 12,
          padding: "8px 16px",
          borderBottom: "1px solid #1e2028",
          flexShrink: 0,
        }}
      >
        <span
          style={{
            color: "#a78bfa",
            fontSize: "11px",
            fontWeight: 700,
            letterSpacing: "0.1em",
            textTransform: "uppercase",
          }}
        >
          Rehearsal Mode
        </span>
        <span style={{ color: "#374151", fontSize: "11px" }}>
          — practice before going live. This session is not saved.
        </span>
      </div>

      {/* Question input */}
      {!submitted && (
        <div
          style={{
            padding: "16px",
            borderBottom: "1px solid #1e2028",
            display: "flex",
            gap: 8,
            flexShrink: 0,
          }}
        >
          <input
            data-testid="rehearsal-question-input"
            type="text"
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void handleSubmit();
            }}
            placeholder="Type a practice interview question…"
            style={{
              flex: 1,
              padding: "8px 12px",
              backgroundColor: "#1a1d26",
              border: "1px solid #2d3748",
              borderRadius: 6,
              color: "#e5e7eb",
              fontSize: "13px",
              outline: "none",
            }}
          />
          <button
            data-testid="rehearsal-submit-button"
            onClick={() => void handleSubmit()}
            disabled={!question.trim()}
            style={{
              padding: "8px 16px",
              backgroundColor: question.trim() ? "#7c3aed" : "#1e2028",
              color: question.trim() ? "#fff" : "#4b5563",
              border: "none",
              borderRadius: 6,
              fontSize: "13px",
              fontWeight: 600,
              cursor: question.trim() ? "pointer" : "not-allowed",
            }}
          >
            Ask
          </button>
          {error && (
            <span style={{ color: "#ef4444", fontSize: "12px", alignSelf: "center" }}>
              {error}
            </span>
          )}
        </div>
      )}

      {/* Panels — same layout as live session */}
      <div style={{ flex: 1, overflow: "hidden" }}>
        <OverlayLayout
          transcript={<TranscriptPanel />}
          directional={<DirectionalPanel />}
          depth={<DepthPanel />}
          clarifying={<ClarifyingPanel />}
          context={<ContextPanel />}
        />
      </div>

      {/* Token budget */}
      <TokenBudgetIndicator />

      {/* Complete rehearsal button — only enabled once a response has arrived */}
      <div
        style={{
          padding: "10px 16px",
          borderTop: "1px solid #1e2028",
          display: "flex",
          justifyContent: "flex-end",
          flexShrink: 0,
        }}
      >
        <button
          data-testid="rehearsal-complete-button"
          onClick={() => void handleComplete()}
          disabled={!hasResponse}
          title={
            hasResponse
              ? "Complete rehearsal and go live"
              : "Wait for a response before completing rehearsal"
          }
          style={{
            padding: "8px 20px",
            backgroundColor: hasResponse ? "#22c55e" : "#1e2028",
            color: hasResponse ? "#fff" : "#4b5563",
            border: "none",
            borderRadius: 6,
            fontSize: "13px",
            fontWeight: 600,
            cursor: hasResponse ? "pointer" : "not-allowed",
          }}
        >
          Complete Rehearsal →
        </button>
      </div>
    </div>
  );
};

export default Rehearsal;
