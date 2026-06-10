import { useCallback, useEffect, useState } from "react";

import FirstRunRehearsalModal from "../components/FirstRunRehearsalModal";
import OverlayLayout from "../components/OverlayLayout";
import PrepChecklist from "../components/PrepChecklist";
import QuestionBank from "../components/QuestionBank";
import ResearchChat from "../components/ResearchChat";
import TokenBudgetIndicator from "../components/TokenBudgetIndicator";
import UsageWidget from "../components/UsageWidget";
import {
  completeRehearsal,
  getSessionContextFields,
  runRehearsalTurn,
  type SessionContextFields,
} from "../commands";
import { useCostCap } from "../hooks/useCostCap";
import { useHotkeys } from "../hooks/useHotkeys";
import { useTokenUsage } from "../hooks/useTokenUsage";
import DirectionalPanel from "../panels/DirectionalPanel";
import DepthPanel from "../panels/DepthPanel";
import ClarifyingPanel from "../panels/ClarifyingPanel";
import ContextPanel from "../panels/ContextPanel";
import TranscriptPanel from "../panels/TranscriptPanel";
import { useUIStore } from "../store/ui";

export interface RehearsalProps {
  sessionId: string;
  onComplete: () => void;
  onReturnToSetup?: () => void;
}

type SideTab = "checklist" | "questions" | "research";

const emptyFields: SessionContextFields = {
  jobDescription: "",
  profile: "",
  companyOverview: "",
  leadershipPrinciples: "",
  roleExpectations: "",
  technicalPrep: "",
  strategyNotes: "",
};

const Rehearsal = ({ sessionId, onComplete, onReturnToSetup }: RehearsalProps) => {
  const [question, setQuestion] = useState("");
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [contextFields, setContextFields] = useState<SessionContextFields>(emptyFields);
  const [showFirstRunModal, setShowFirstRunModal] = useState(true);
  const [sideTab, setSideTab] = useState<SideTab>("checklist");
  const [sideOpen, setSideOpen] = useState(true);

  const {
    streamingBuffers,
    clearStreamingBuffers,
    clearClarifyingQuestions,
    setLastManualQuestion,
  } = useUIStore();

  useTokenUsage();
  useCostCap();
  useHotkeys(sessionId, question, asking);

  const loadFields = useCallback(async () => {
    try {
      const fields = await getSessionContextFields(sessionId);
      setContextFields(fields);
    } catch {
      // Non-fatal — checklist degrades gracefully to all-empty.
    }
  }, [sessionId]);

  useEffect(() => {
    void loadFields();
  }, [loadFields]);

  const hasResponse =
    streamingBuffers.directional.length > 0 ||
    streamingBuffers.depth.length > 0;

  const handleSubmit = async () => {
    if (!question.trim() || asking) return;
    setError(null);
    clearStreamingBuffers();
    clearClarifyingQuestions();
    setLastManualQuestion(question.trim());
    setAsking(true);

    try {
      await runRehearsalTurn(sessionId, question.trim());
    } catch (e) {
      setError(String(e));
    } finally {
      setAsking(false);
    }
  };

  const handleBankAsk = useCallback(
    (q: string) => {
      setQuestion(q);
      clearStreamingBuffers();
      clearClarifyingQuestions();
      setLastManualQuestion(q);
      setAsking(true);

      void runRehearsalTurn(sessionId, q).finally(() => setAsking(false));
    },
    [sessionId, clearStreamingBuffers, clearClarifyingQuestions, setLastManualQuestion],
  );

  const handleComplete = async () => {
    try {
      await completeRehearsal(sessionId);
    } catch (e) {
      setError(String(e));
      return;
    }
    onComplete();
  };

  const handleQuestionKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void handleSubmit();
    }
  };

  return (
    <>
      {showFirstRunModal && (
        <FirstRunRehearsalModal
          fields={contextFields}
          onDismiss={() => setShowFirstRunModal(false)}
        />
      )}

      <div
        data-testid="rehearsal-screen"
        style={{
          display: "flex",
          flexDirection: "column",
          height: "calc(100vh - 36px)",
          backgroundColor: "#0f1117",
          fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
        }}
      >
        {/* Header */}
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
          <span style={{ color: "#6b7280", fontSize: "11px" }}>
            — practice before going live. Ctrl+Enter to ask; Enter for a new line.
          </span>

          <div style={{ marginLeft: "auto", display: "flex", gap: 8, alignItems: "center" }}>
            <UsageWidget />
            {onReturnToSetup && (
              <button
                type="button"
                data-testid="rehearsal-back-to-setup-button"
                onClick={onReturnToSetup}
                style={{
                  padding: "4px 10px",
                  fontSize: "11px",
                  fontWeight: 600,
                  borderRadius: 4,
                  border: "1px solid #374151",
                  backgroundColor: "transparent",
                  color: "#9ca3af",
                  cursor: "pointer",
                }}
              >
                Edit session setup
              </button>
            )}
          </div>
        </div>

        {/* Question input */}
        <div
          style={{
            padding: "12px 16px",
            borderBottom: "1px solid #1e2028",
            display: "flex",
            gap: 8,
            flexShrink: 0,
            alignItems: "flex-start",
          }}
        >
          <textarea
            data-testid="rehearsal-question-input"
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            onKeyDown={handleQuestionKeyDown}
            placeholder="Type a practice interview question… (Enter = new line, Ctrl+Enter = Ask)"
            rows={3}
            disabled={asking}
            style={{
              flex: 1,
              padding: "8px 12px",
              backgroundColor: "#1a1d26",
              border: "1px solid #2d3748",
              borderRadius: 6,
              color: "#e5e7eb",
              fontSize: "13px",
              lineHeight: 1.5,
              fontFamily: "inherit",
              resize: "vertical",
              minHeight: "4.5rem",
              maxHeight: "12rem",
              outline: "none",
            }}
          />
          <button
            data-testid="rehearsal-submit-button"
            onClick={() => void handleSubmit()}
            disabled={!question.trim() || asking}
            style={{
              padding: "8px 16px",
              backgroundColor: question.trim() && !asking ? "#7c3aed" : "#1e2028",
              color: question.trim() && !asking ? "#fff" : "#4b5563",
              border: "none",
              borderRadius: 6,
              fontSize: "13px",
              fontWeight: 600,
              cursor: question.trim() && !asking ? "pointer" : "not-allowed",
              flexShrink: 0,
              alignSelf: "flex-end",
            }}
          >
            {asking ? "Asking…" : hasResponse ? "Ask again" : "Ask"}
          </button>
        </div>

        {error && (
          <div
            style={{
              padding: "8px 16px",
              color: "#ef4444",
              fontSize: "12px",
              borderBottom: "1px solid #1e2028",
              flexShrink: 0,
            }}
          >
            {error}
          </div>
        )}

        {/* Main content: panels + sidebar */}
        <div style={{ flex: 1, overflow: "hidden", minHeight: 0, display: "flex" }}>
          {/* Overlay panels */}
          <div style={{ flex: 1, overflow: "hidden" }}>
            <OverlayLayout
              transcript={<TranscriptPanel />}
              directional={<DirectionalPanel sessionId={sessionId} />}
              depth={<DepthPanel />}
              clarifying={<ClarifyingPanel />}
              context={<ContextPanel sessionId={sessionId} />}
            />
          </div>

          {/* Sidebar */}
          <div
            style={{
              width: sideOpen ? 280 : 28,
              flexShrink: 0,
              borderLeft: "1px solid #1e2028",
              display: "flex",
              flexDirection: "column",
              overflow: "hidden",
              transition: "width 0.18s ease",
            }}
          >
            {/* Sidebar toggle */}
            <button
              onClick={() => setSideOpen((v) => !v)}
              title={sideOpen ? "Hide sidebar" : "Show sidebar"}
              style={{
                height: 28,
                background: "none",
                border: "none",
                borderBottom: "1px solid #1e2028",
                cursor: "pointer",
                color: "#52525b",
                fontSize: 10,
                fontWeight: 600,
                display: "flex",
                alignItems: "center",
                justifyContent: sideOpen ? "flex-end" : "center",
                padding: "0 8px",
                flexShrink: 0,
              }}
            >
              {sideOpen ? "▶" : "◀"}
            </button>

            {sideOpen && (
              <>
                {/* Tab bar */}
                <div
                  style={{
                    display: "flex",
                    borderBottom: "1px solid #1e2028",
                    flexShrink: 0,
                  }}
                >
                  {(["checklist", "questions", "research"] as SideTab[]).map((t) => (
                    <button
                      key={t}
                      onClick={() => setSideTab(t)}
                      style={{
                        flex: 1,
                        padding: "5px 4px",
                        background: "none",
                        border: "none",
                        borderBottom: sideTab === t ? "2px solid #7c3aed" : "2px solid transparent",
                        color: sideTab === t ? "#a78bfa" : "#52525b",
                        fontSize: 10,
                        fontWeight: 600,
                        cursor: "pointer",
                        textTransform: "capitalize",
                        letterSpacing: "0.04em",
                      }}
                    >
                      {t === "checklist" ? "Prep" : t === "questions" ? "Qs" : "Chat"}
                    </button>
                  ))}
                </div>

                {/* Tab content */}
                <div style={{ flex: 1, overflow: "auto", padding: 12 }}>
                  {sideTab === "checklist" && (
                    <PrepChecklist fields={contextFields} />
                  )}
                  {sideTab === "questions" && (
                    <QuestionBank
                      sessionId={sessionId}
                      onAskQuestion={handleBankAsk}
                      asking={asking}
                    />
                  )}
                  {sideTab === "research" && (
                    <ResearchChat sessionId={sessionId} />
                  )}
                </div>
              </>
            )}
          </div>
        </div>

        <TokenBudgetIndicator />

        {/* Footer */}
        <div
          style={{
            padding: "10px 16px",
            borderTop: "1px solid #1e2028",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            flexShrink: 0,
            gap: 12,
          }}
        >
          <span style={{ color: "#52525b", fontSize: "11px" }}>
            {hasResponse
              ? "Review the panels above, then go live when ready."
              : "Optional: ask a practice question, or go live without rehearsing."}
          </span>
          <button
            data-testid="rehearsal-complete-button"
            onClick={() => void handleComplete()}
            title="Continue to live session"
            style={{
              padding: "8px 20px",
              backgroundColor: "#22c55e",
              color: "#fff",
              border: "none",
              borderRadius: 6,
              fontSize: "13px",
              fontWeight: 600,
              cursor: "pointer",
              flexShrink: 0,
            }}
          >
            {hasResponse ? "Complete Rehearsal →" : "Skip rehearsal → Go live"}
          </button>
        </div>
      </div>
    </>
  );
};

export default Rehearsal;
