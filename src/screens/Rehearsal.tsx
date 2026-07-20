import { useCallback, useEffect, useRef, useState } from "react";

import InfoPopover from "../components/InfoPopover";
import PreferredAnswerPanel from "../components/PreferredAnswerPanel";
import LiveReadinessModal from "../components/LiveReadinessModal";
import MicQualityBadge from "../components/MicQualityBadge";
import FirstRunRehearsalModal, {
  isFirstRunModalDismissed,
} from "../components/FirstRunRehearsalModal";
import PreLiveTimingModal from "../components/PreLiveTimingModal";
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
  runLiveReadinessCheck,
  runRehearsalTurn,
  type LiveReadinessReportDto,
  type SessionContextFields,
} from "../commands";
import {
  isLiveReadinessTestCurrent,
  saveLiveReadinessTestRecord,
} from "../lib/liveReadinessStorage";
import { useCostCap } from "../hooks/useCostCap";
import { useHotkeys, isRehearsalSubmitChord } from "../hooks/useHotkeys";
import { useOrchestratorStreams } from "../hooks/useOrchestratorStreams";
import { useRagChunks } from "../hooks/useRagChunks";
import { useTokenUsage } from "../hooks/useTokenUsage";
import { needsUserContext } from "../lib/contextQuality";
import AnswerPanel from "../panels/AnswerPanel";
import VisualPanel from "../panels/VisualPanel";
import ContextPanel from "../panels/ContextPanel";
import TranscriptPanel from "../panels/TranscriptPanel";
import { useUIStore } from "../store/ui";

export interface RehearsalProps {
  sessionId: string;
  /** Clear answer/visual panels (e.g. after re-ingest). */
  resetPanelsOnEntry?: boolean;
  onResetPanelsHandled?: () => void;
  onComplete: () => void;
  onReturnToSetup?: () => void;
  onOpenSettings?: () => void;
  onStartMock?: () => void;
  onStartLivePreviewTest?: () => void;
}

type SideTab = "checklist" | "questions" | "research" | "stories";

const QUESTION_INPUT_MIN_HEIGHT = 44;
const QUESTION_INPUT_MAX_HEIGHT = 260;
const QUESTION_INPUT_DEFAULT_HEIGHT = 64;
const QUESTION_INPUT_HEIGHT_STORAGE_KEY = "flint.rehearsal.questionInputHeight";

const readPersistedQuestionInputHeight = (): number => {
  try {
    const raw = window.localStorage.getItem(QUESTION_INPUT_HEIGHT_STORAGE_KEY);
    const parsed = raw ? Number.parseInt(raw, 10) : NaN;
    if (Number.isFinite(parsed)) {
      return Math.min(QUESTION_INPUT_MAX_HEIGHT, Math.max(QUESTION_INPUT_MIN_HEIGHT, parsed));
    }
  } catch {
    // localStorage unavailable (e.g. private mode) — fall back to default.
  }
  return QUESTION_INPUT_DEFAULT_HEIGHT;
};

const persistQuestionInputHeight = (height: number): void => {
  try {
    window.localStorage.setItem(QUESTION_INPUT_HEIGHT_STORAGE_KEY, String(height));
  } catch {
    // Non-fatal — just won't persist across sessions.
  }
};

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
  onStartLivePreviewTest,
}: RehearsalProps) => {
  const [question, setQuestion] = useState("");
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [contextFields, setContextFields] = useState<SessionContextFields>(emptyFields);
  const [showTimingModal, setShowTimingModal] = useState(true);
  const [showFirstRunModal, setShowFirstRunModal] = useState(false);
  const [readinessReport, setReadinessReport] = useState<LiveReadinessReportDto | null>(
    null,
  );
  const [readinessBusy, setReadinessBusy] = useState(false);
  const [liveTestCurrent, setLiveTestCurrent] = useState(false);

  const refreshLiveTestStatus = useCallback(async () => {
    try {
      const report = await runLiveReadinessCheck(sessionId);
      setLiveTestCurrent(isLiveReadinessTestCurrent(sessionId, report.configFingerprint));
    } catch {
      setLiveTestCurrent(false);
    }
  }, [sessionId]);

  useEffect(() => {
    void refreshLiveTestStatus();
  }, [refreshLiveTestStatus]);

  useEffect(() => {
    const onFocus = () => void refreshLiveTestStatus();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshLiveTestStatus]);
  const [sideTab, setSideTab] = useState<SideTab>("checklist");
  const [sideOpen, setSideOpen] = useState(true);
  const [questionInputHeight, setQuestionInputHeight] = useState(
    readPersistedQuestionInputHeight,
  );
  const questionInputHeightRef = useRef(questionInputHeight);
  questionInputHeightRef.current = questionInputHeight;

  const {
    streamingBuffers,
    clearStreamingBuffers,
    resetOrchestratorPanels,
    setLastManualQuestion,
    setConfidenceLevel,
    ragChunks,
    confidenceLevel,
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
        streamingBuffers.answer,
      ),
    );
  }, [asking, confidenceLevel, ragChunks, streamingBuffers.answer]);

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
      setConfidenceLevel,
      setLastManualQuestion,
    ],
  );

  const hasResponse =
    streamingBuffers.answer.length > 0 || streamingBuffers.visual.length > 0;

  const isReaskingSameQuestion =
    hasResponse &&
    lastAskedQuestion.trim() !== "" &&
    question.trim() === lastAskedQuestion.trim();

  // On first response, collapse low-priority panels so Answer/Visual get space.
  const autoFocusedPanelsRef = useRef(false);
  useEffect(() => {
    if (!hasResponse || autoFocusedPanelsRef.current) return;
    autoFocusedPanelsRef.current = true;
    useUIStore.setState((s) => ({
      panelLayout: {
        ...s.panelLayout,
        collapsed: {
          ...s.panelLayout.collapsed,
          transcript: true,
          context: true,
        },
      },
    }));
  }, [hasResponse]);

  const handleSubmit = useCallback(async () => {
    if (!question.trim() || asking) return;
    await fireQuestion(question.trim());
  }, [asking, fireQuestion, question]);

  const handleBankAsk = useCallback(
    (q: string) => {
      setQuestion(q);
      void fireQuestion(q);
    },
    [fireQuestion],
  );

  const handleQuestionResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = questionInputHeightRef.current;

    const onMove = (ev: MouseEvent) => {
      const dy = ev.clientY - startY;
      const next = Math.min(
        QUESTION_INPUT_MAX_HEIGHT,
        Math.max(QUESTION_INPUT_MIN_HEIGHT, startHeight + dy),
      );
      setQuestionInputHeight(next);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      persistQuestionInputHeight(questionInputHeightRef.current);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, []);

  const handleGenerateDiagram = useCallback(
    async (q: string) => {
      setError(null);
      clearStreamingBuffers();
      setAsking(true);
      try {
        await runRehearsalTurn(sessionId, q, undefined, true);
      } catch (e) {
        setError(String(e));
      } finally {
        setAsking(false);
      }
    },
    [sessionId, clearStreamingBuffers],
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

  const handleTestLiveSession = async () => {
    setReadinessBusy(true);
    setError(null);
    try {
      const report = await runLiveReadinessCheck(sessionId);
      setReadinessReport(report);
    } catch (e) {
      setError(String(e));
    } finally {
      setReadinessBusy(false);
    }
  };

  const closeReadinessModal = () => {
    if (readinessReport?.ready) {
      saveLiveReadinessTestRecord(sessionId, readinessReport.configFingerprint);
      setLiveTestCurrent(true);
    }
    setReadinessReport(null);
  };

  const handleRunAudioPreview = () => {
    if (!readinessReport?.ready) return;
    saveLiveReadinessTestRecord(sessionId, readinessReport.configFingerprint);
    setLiveTestCurrent(true);
    setReadinessReport(null);
    onStartLivePreviewTest?.();
  };

  // Wayland/WebKit often omits modifier flags on the target element; capture at
  // document. Refs keep the listener stable (no rebind on every keystroke).
  const askingRef = useRef(asking);
  const questionRef = useRef(question);
  askingRef.current = asking;
  questionRef.current = question;
  useEffect(() => {
    const onDocKeyDown = (e: KeyboardEvent) => {
      if (askingRef.current) return;
      if (!isRehearsalSubmitChord(e)) return;
      const root = document.querySelector('[data-testid="rehearsal-screen"]');
      if (!root?.contains(e.target as Node)) return;
      const q = questionRef.current.trim();
      if (!q) return;
      e.preventDefault();
      void fireQuestion(q);
    };
    document.addEventListener("keydown", onDocKeyDown, true);
    return () => document.removeEventListener("keydown", onDocKeyDown, true);
  }, [fireQuestion]);

  return (
    <PanicRestoreShell>
      <>
      {showTimingModal && (
        <PreLiveTimingModal
          onDismiss={() => {
            setShowTimingModal(false);
            if (!isFirstRunModalDismissed()) {
              setShowFirstRunModal(true);
            }
          }}
        />
      )}
      {showFirstRunModal && (
        <FirstRunRehearsalModal
          fields={contextFields}
          onDismiss={() => setShowFirstRunModal(false)}
        />
      )}
      {readinessReport && (
        <LiveReadinessModal
          report={readinessReport}
          busy={readinessBusy}
          onClose={closeReadinessModal}
          onRunAudioPreview={handleRunAudioPreview}
        />
      )}

      <div
        data-testid="rehearsal-screen"
        className="rehearsal-screen"
      >
        {/* Compact header — workflow + shortcut hints live in [i] popovers */}
        <header className="rehearsal-header">
          <div className="rehearsal-header__primary">
            <span className="rehearsal-header__mode">Rehearsal Mode</span>
            <SessionContextBadges
              sessionId={sessionId}
              onOpenSettings={onOpenSettings}
            />
            <InfoPopover ariaLabel="How rehearsal feeds Live">
              <p>
                <strong>Ask → tailor → save for Live.</strong> Flint drafts from your prep
                context. Edit each answer into your own words, then{" "}
                <strong>Save as preferred answer</strong>. Saved scripts appear instantly
                when the same question comes up in your live interview.
              </p>
            </InfoPopover>
            <InfoPopover ariaLabel="Keyboard shortcuts">
              <ul className="rehearsal-shortcuts-list">
                <li>
                  <kbd>Ctrl+Enter</kbd> — Ask / submit question
                </li>
                <li>
                  {typeof navigator !== "undefined" && /linux/i.test(navigator.userAgent)
                    ? "Ctrl+Shift+Space or F8 — Re-ask (Ctrl+Alt+Space blocked on Wayland)"
                    : "Ctrl+Alt+Space — Re-ask last question"}
                </li>
              </ul>
            </InfoPopover>
          </div>

          <div className="rehearsal-header__actions">
            <button
              type="button"
              data-testid="test-live-session-button"
              disabled={readinessBusy || liveTestCurrent}
              title={
                liveTestCurrent
                  ? "Already tested with current settings — change phone mode, mic calibration, or audio routing to re-test"
                  : "Run installation and live-session readiness checks"
              }
              onClick={() => void handleTestLiveSession()}
              className="rehearsal-header__ghost-btn"
            >
              {readinessBusy
                ? "Testing…"
                : liveTestCurrent
                  ? "Live test complete"
                  : "Test Live Session"}
            </button>
            <UsageWidget />
            {onOpenSettings && (
              <button
                type="button"
                onClick={onOpenSettings}
                title="Open API Keys settings"
                className="rehearsal-header__ghost-btn"
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
                className="rehearsal-header__ghost-btn"
              >
                Edit session setup
              </button>
            )}
          </div>
        </header>

        {/* Scrollable workflow strip — caps height so panels always get room */}
        <div className="rehearsal-workflow-scroll">
          <div className="rehearsal-question-row">
            <div className="rehearsal-question-input-wrap">
              <textarea
                data-testid="rehearsal-question-input"
                value={question}
                onChange={(e) => setQuestion(e.target.value)}
                placeholder="Type a practice question… (Ctrl+Enter to ask)"
                disabled={asking}
                className="rehearsal-question-input"
                style={{ height: `${questionInputHeight}px` }}
              />
              <div
                className="rehearsal-question-resize-handle"
                onMouseDown={handleQuestionResizeStart}
                role="separator"
                aria-orientation="horizontal"
                aria-label="Resize question input"
                title="Drag to resize"
              >
                <span className="rehearsal-question-resize-handle__grip" />
              </div>
            </div>
            <button
              data-testid="rehearsal-submit-button"
              onClick={() => void handleSubmit()}
              disabled={!question.trim() || asking}
              className="rehearsal-ask-btn"
            >
              {asking ? "Asking…" : isReaskingSameQuestion ? "Ask again" : "Ask"}
            </button>
          </div>

          {error && <div className="rehearsal-inline-alert rehearsal-inline-alert--error">{error}</div>}
          {costBlocked && !error && (
            <div className="rehearsal-inline-alert rehearsal-inline-alert--warn">{costBlocked}</div>
          )}

          {!asking && weakContext && hasResponse && lastAskedQuestion && (
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
          )}

          {!asking && hasResponse && lastAskedQuestion && (
            <PreferredAnswerPanel
              sessionId={sessionId}
              question={lastAskedQuestion}
              suggestedAnswer={streamingBuffers.answer}
              onSaved={() => setBankRefreshKey((k) => k + 1)}
              defaultCollapsed
            />
          )}
        </div>

        {/* Main content: panels + sidebar — guaranteed min height */}
        <div className="rehearsal-main">
          <div className="rehearsal-panels">
            <OverlayLayout
              transcript={<TranscriptPanel sessionId={sessionId} />}
              answer={
                <AnswerPanel sessionId={sessionId} isGenerating={asking} />
              }
              visual={
                <VisualPanel
                  sessionId={sessionId}
                  isGenerating={asking}
                  onGenerateDiagram={handleGenerateDiagram}
                />
              }
              context={<ContextPanel sessionId={sessionId} />}
            />
          </div>

          {/* Sidebar */}
          <div className={`rehearsal-sidebar${sideOpen ? "" : " rehearsal-sidebar--collapsed"}`}>
            {/* Sidebar toggle — colored bar so collapse/expand is obvious at a glance */}
            <button
              onClick={() => setSideOpen((v) => !v)}
              aria-label={sideOpen ? "Hide prep tools sidebar" : "Show prep tools sidebar"}
              title={sideOpen ? "Hide Prep Tools sidebar" : "Show Prep Tools sidebar (Qs, Chat, Stories)"}
              className={`rehearsal-sidebar__toggle${
                sideOpen ? " rehearsal-sidebar__toggle--expanded" : " rehearsal-sidebar__toggle--collapsed"
              }`}
            >
              {sideOpen ? (
                <>
                  <span>Prep Tools</span>
                  <span className="rehearsal-sidebar__toggle-icon">▸</span>
                </>
              ) : (
                <span className="rehearsal-sidebar__toggle-vertical">◂ Prep Tools</span>
              )}
            </button>

            {sideOpen && (
              <>
                {/* Tab bar */}
                <div className="rehearsal-sidebar__tabs">
                  {(["checklist", "questions", "research", "stories"] as SideTab[]).map((t) => (
                    <button
                      key={t}
                      onClick={() => setSideTab(t)}
                      className={`rehearsal-sidebar__tab${
                        sideTab === t ? " rehearsal-sidebar__tab--active" : ""
                      }`}
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
                <div className="rehearsal-sidebar__tab-content">
                  {sideTab === "checklist" && (
                    <PrepChecklist
                      fields={contextFields}
                      sessionId={sessionId}
                      onFieldsUpdated={loadFields}
                      onOpenSessionDesign={onReturnToSetup}
                    />
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

        <footer className="rehearsal-footer">
          <span className={`rehearsal-footer__hint${hasResponse ? "" : " rehearsal-footer__hint--warn"}`}>
            {hasResponse
              ? "Review the panels above. Mock Interview trains delivery — go live when you feel ready."
              : "Ask at least one practice question and try Mock Interview before going live — better prep means sharper answers in the real session."}
          </span>
          {onStartMock && (
            <button
              data-testid="start-mock-button"
              onClick={onStartMock}
              title="Strongly recommended — practice with AI interviewer before going live"
              className="rehearsal-footer__mock-btn"
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
            className={`rehearsal-footer__complete-btn${hasResponse ? " rehearsal-footer__complete-btn--ready" : ""}`}
          >
            {hasResponse ? "Complete Rehearsal →" : "Go live without rehearsing"}
          </button>
        </footer>
      </div>
      <MicQualityBadge />
      </>
    </PanicRestoreShell>
  );
};

export default Rehearsal;
