import { useCallback, useEffect, useRef, useState } from "react";

import PreferredAnswerPanel from "../components/PreferredAnswerPanel";
import MicQualityBadge from "../components/MicQualityBadge";
import FirstRunRehearsalModal, {
  isFirstRunModalDismissed,
} from "../components/FirstRunRehearsalModal";
import AddContextPanel from "../components/AddContextPanel";
import StoryEditor from "../components/StoryEditor";
import OverlayLayout from "../components/OverlayLayout";
import PanicRestoreShell from "../components/PanicRestoreShell";
import PrepChecklist from "../components/PrepChecklist";
import QuestionBank from "../components/QuestionBank";
import ResearchChat from "../components/ResearchChat";
import TokenBudgetIndicator from "../components/TokenBudgetIndicator";
import SessionContextBadges from "../components/SessionContextBadges";
import UsageWidget from "../components/UsageWidget";
import {
  completeRehearsal,
  getCostStatus,
  getSessionContextFields,
  runRehearsalTurn,
  type SessionContextFields,
} from "../commands";
import { useCostCap } from "../hooks/useCostCap";
import { useHotkeys } from "../hooks/useHotkeys";
import { useOrchestratorStreams } from "../hooks/useOrchestratorStreams";
import { useRagChunks } from "../hooks/useRagChunks";
import { useTokenUsage } from "../hooks/useTokenUsage";
import { needsUserContext } from "../lib/contextQuality";
import DirectionalPanel from "../panels/DirectionalPanel";
import DepthPanel from "../panels/DepthPanel";
import ClarifyingPanel from "../panels/ClarifyingPanel";
import ContextPanel from "../panels/ContextPanel";
import TranscriptPanel from "../panels/TranscriptPanel";
import { useUIStore } from "../store/ui";

export interface RehearsalProps {
  sessionId: string;
  /** Clear directional/depth/clarifying panels (e.g. after re-ingest). */
  resetPanelsOnEntry?: boolean;
  onResetPanelsHandled?: () => void;
  onComplete: () => void;
  onReturnToSetup?: () => void;
  onOpenSettings?: () => void;
  onStartMock?: () => void;
}

type SideTab = "checklist" | "questions" | "research" | "stories";

const emptyFields: SessionContextFields = {
  jobDescription: "",
  profile: "",
  companyOverview: "",
  leadershipPrinciples: "",
  roleExpectations: "",
  technicalPrep: "",
  strategyNotes: "",
  speakingStyle: "polished",
  sessionVocabulary: "",
};

const Rehearsal = ({
  sessionId,
  resetPanelsOnEntry = false,
  onResetPanelsHandled,
  onComplete,
  onReturnToSetup,
  onOpenSettings,
  onStartMock,
}: RehearsalProps) => {
  const [question, setQuestion] = useState("");
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [contextFields, setContextFields] = useState<SessionContextFields>(emptyFields);
  const [showFirstRunModal, setShowFirstRunModal] = useState(
    () => !isFirstRunModalDismissed(),
  );
  const [sideTab, setSideTab] = useState<SideTab>("checklist");
  const [sideOpen, setSideOpen] = useState(true);

  const {
    streamingBuffers,
    clearStreamingBuffers,
    clearClarifyingQuestions,
    resetOrchestratorPanels,
    setLastManualQuestion,
    setConfidenceLevel,
    ragChunks,
    confidenceLevel,
    clarifyingQuestions,
    lastManualQuestion,
  } = useUIStore();

  const prevSessionIdRef = useRef<string | null>(null);

  const [lastAskedQuestion, setLastAskedQuestion] = useState("");
  const [bankRefreshKey, setBankRefreshKey] = useState(0);
  const [weakContext, setWeakContext] = useState(false);
  const [costBlocked, setCostBlocked] = useState<string | null>(null);

  const costCap = useUIStore((s) => s.costCap);

  useTokenUsage();
  useCostCap();
  useHotkeys(sessionId, lastManualQuestion || question, !asking);
  useRagChunks(sessionId);
  useOrchestratorStreams();

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

  // Fresh panel state after re-setup (same session) or when switching sessions.
  useEffect(() => {
    const sessionChanged =
      prevSessionIdRef.current !== null && prevSessionIdRef.current !== sessionId;
    if (resetPanelsOnEntry || sessionChanged) {
      resetOrchestratorPanels();
      setQuestion("");
      setLastAskedQuestion("");
      setError(null);
      if (resetPanelsOnEntry) {
        onResetPanelsHandled?.();
      }
    }
    prevSessionIdRef.current = sessionId;
  }, [
    sessionId,
    resetPanelsOnEntry,
    resetOrchestratorPanels,
    onResetPanelsHandled,
  ]);

  useEffect(() => {
    void getCostStatus()
      .then((s) => {
        if (s.suspended) {
          setCostBlocked(
            `Usage limit reached (${s.totalTokens.toLocaleString()} tokens). ` +
              "Open Settings → Usage limits → Reset counters or raise the cap.",
          );
        } else {
          setCostBlocked(null);
        }
      })
      .catch(() => {
        // Non-fatal — rehearsal still works without cap snapshot.
      });
  }, [costCap.suspended]);

  // After a turn completes, use orchestrator confidence (not RAG score alone).
  useEffect(() => {
    if (asking) return;
    setWeakContext(
      needsUserContext(
        confidenceLevel,
        ragChunks,
        streamingBuffers.directional,
      ),
    );
  }, [
    asking,
    confidenceLevel,
    ragChunks,
    clarifyingQuestions.length,
    streamingBuffers.directional,
  ]);

  const fireQuestion = useCallback(
    async (q: string) => {
      setError(null);
      try {
        const cap = await getCostStatus();
        if (cap.suspended) {
          setCostBlocked(
            `Usage limit reached (${cap.totalTokens.toLocaleString()} tokens). ` +
              "Open Settings → Usage limits → Reset counters or raise the cap.",
          );
          return;
        }
        setCostBlocked(null);
      } catch {
        // Proceed — backend will enforce the cap if needed.
      }
      clearStreamingBuffers();
      clearClarifyingQuestions();
      setConfidenceLevel(null);
      setLastManualQuestion(q);
      setLastAskedQuestion(q);
      setWeakContext(false);
      setAsking(true);
      try {
        await runRehearsalTurn(sessionId, q);
      } catch (e) {
        setError(String(e));
      } finally {
        setAsking(false);
        setBankRefreshKey((k) => k + 1);
      }
    },
    [
      sessionId,
      clearStreamingBuffers,
      clearClarifyingQuestions,
      setConfidenceLevel,
      setLastManualQuestion,
    ],
  );

  const hasResponse =
    streamingBuffers.directional.length > 0 ||
    streamingBuffers.depth.length > 0;

  const isReaskingSameQuestion =
    hasResponse &&
    lastAskedQuestion.trim() !== "" &&
    question.trim() === lastAskedQuestion.trim();

  const handleSubmit = async () => {
    if (!question.trim() || asking) return;
    await fireQuestion(question.trim());
  };

  const handleBankAsk = useCallback(
    (q: string) => {
      setQuestion(q);
      void fireQuestion(q);
    },
    [fireQuestion],
  );

  const handleComplete = async () => {
    if (!hasResponse) {
      const proceed = window.confirm(
        "You have not asked a practice question this session.\n\n" +
          "Rehearsal helps you tailor answers before Live. Going live without " +
          "practicing or saving preferred answers may produce generic responses.\n\n" +
          "Go live anyway?",
      );
      if (!proceed) return;
    }
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
    <PanicRestoreShell>
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
            flexWrap: "wrap",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 12, minWidth: 0 }}>
            <span
              style={{
                color: "#a78bfa",
                fontSize: "11px",
                fontWeight: 700,
                letterSpacing: "0.1em",
                textTransform: "uppercase",
                flexShrink: 0,
              }}
            >
              Rehearsal Mode
            </span>
            <SessionContextBadges
              sessionId={sessionId}
              onOpenSettings={onOpenSettings}
            />
          </div>
          <span style={{ color: "#6b7280", fontSize: "11px" }}>
            — Ask → tailor your answer → save for Live. Ctrl+Enter to ask.
          </span>

          <div style={{ marginLeft: "auto", display: "flex", gap: 8, alignItems: "center" }}>
            <UsageWidget />
            {onOpenSettings && (
              <button
                type="button"
                onClick={onOpenSettings}
                title="Open API Keys settings"
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
                API Keys
              </button>
            )}
            {onReturnToSetup && (
              <button
                type="button"
                data-testid="rehearsal-back-to-setup-button"
                onClick={() => {
                  if (
                    window.confirm(
                      "Return to Session Design?\n\nThis will pause your rehearsal and take you back to re-ingest context. Your question bank and pasted text will be preserved, but you will need to run Extract & Continue again before returning to Rehearsal.",
                    )
                  ) {
                    onReturnToSetup();
                  }
                }}
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

        <div className="rehearsal-workflow-banner">
          <strong>How rehearsal feeds Live:</strong> Flint drafts from your prep context.
          Edit each answer into your own words, then <strong>Save as preferred answer</strong>.
          Saved scripts appear instantly when the same question comes up in your live interview.
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
            {asking ? "Asking…" : isReaskingSameQuestion ? "Ask again" : "Ask"}
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

        {costBlocked && !error && (
          <div
            style={{
              padding: "8px 16px",
              color: "#f59e0b",
              fontSize: "12px",
              borderBottom: "1px solid #1e2028",
              flexShrink: 0,
            }}
          >
            {costBlocked}
          </div>
        )}

        {/* Weak-context warning — shown after a turn with no/weak RAG hits */}
        {!asking && weakContext && hasResponse && lastAskedQuestion && (
          <div style={{ padding: "8px 16px", flexShrink: 0 }}>
            <AddContextPanel
              sessionId={sessionId}
              question={lastAskedQuestion}
              onSaved={(chunksAdded, reask) => {
                void loadFields();
                if (reask && chunksAdded > 0) {
                  void fireQuestion(lastAskedQuestion);
                } else if (chunksAdded > 0) {
                  setWeakContext(false);
                }
              }}
            />
          </div>
        )}

        {!asking && hasResponse && lastAskedQuestion && (
          <div style={{ padding: "8px 16px", flexShrink: 0 }}>
            <PreferredAnswerPanel
              sessionId={sessionId}
              question={lastAskedQuestion}
              suggestedAnswer={streamingBuffers.directional}
              onSaved={() => setBankRefreshKey((k) => k + 1)}
            />
          </div>
        )}

        {/* Main content: panels + sidebar */}
        <div style={{ flex: 1, overflow: "hidden", minHeight: 0, display: "flex" }}>
          {/* Overlay panels */}
          <div style={{ flex: 1, overflow: "hidden" }}>
            <OverlayLayout
              transcript={<TranscriptPanel />}
              directional={
                <DirectionalPanel sessionId={sessionId} isGenerating={asking} />
              }
              depth={<DepthPanel isGenerating={asking} />}
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
                  {(["checklist", "questions", "research", "stories"] as SideTab[]).map((t) => (
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
                      {t === "checklist"
                        ? "Prep"
                        : t === "questions"
                          ? "Qs"
                          : t === "research"
                            ? "Chat"
                            : "Stories"}
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
                      refreshKey={bankRefreshKey}
                    />
                  )}
                  {sideTab === "research" && (
                    <ResearchChat sessionId={sessionId} />
                  )}
                  {sideTab === "stories" && (
                    <StoryEditor
                      key={lastAskedQuestion}
                      sessionId={sessionId}
                      defaultQuestion={lastAskedQuestion || question}
                      onSaved={(chunksAdded, reask) => {
                        void loadFields();
                        if (reask && chunksAdded > 0 && lastAskedQuestion) {
                          void fireQuestion(lastAskedQuestion);
                        }
                      }}
                    />
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
          <span
            style={{
              color: hasResponse ? "#52525b" : "#f59e0b",
              fontSize: "11px",
              lineHeight: 1.5,
              maxWidth: 420,
            }}
          >
            {hasResponse
              ? "Review the panels above. Mock Interview trains delivery — go live when you feel ready."
              : "Ask at least one practice question and try Mock Interview before going live — better prep means sharper answers in the real session."}
          </span>
          {onStartMock && (
            <button
              data-testid="start-mock-button"
              onClick={onStartMock}
              title="Strongly recommended — practice with AI interviewer before going live"
              style={{
                padding: "8px 20px",
                backgroundColor: "#7c3aed",
                color: "#fff",
                border: "none",
                borderRadius: 6,
                fontSize: "13px",
                fontWeight: 600,
                cursor: "pointer",
                flexShrink: 0,
              }}
            >
              Mock Interview
            </button>
          )}
          <button
            data-testid="rehearsal-complete-button"
            onClick={() => void handleComplete()}
            title={
              hasResponse
                ? "Continue to live session"
                : "Not recommended — practice first for better live answers"
            }
            style={{
              padding: "8px 20px",
              backgroundColor: hasResponse ? "#22c55e" : "transparent",
              color: hasResponse ? "#fff" : "#94a3b8",
              border: hasResponse ? "none" : "1px solid #374151",
              borderRadius: 6,
              fontSize: "13px",
              fontWeight: 600,
              cursor: "pointer",
              flexShrink: 0,
            }}
          >
            {hasResponse ? "Complete Rehearsal →" : "Go live without rehearsing"}
          </button>
        </div>
      </div>
      <MicQualityBadge />
      </>
    </PanicRestoreShell>
  );
};

export default Rehearsal;
