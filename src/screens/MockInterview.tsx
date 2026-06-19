import { useCallback, useEffect, useRef, useState } from "react";
import { type UnlistenFn } from "@tauri-apps/api/event";

import {
  advanceMockTurn,
  askMockQuestion,
  endMockTurn,
  regradeMockTurn,
  retryMockTurn,
  skipMockTurn,
  startMock,
  startMockTurn,
  stopMock,
  type CoachFeedback,
  type MockStudyMode,
} from "../commands";
import {
  onMockCoachFeedback,
  onMockEnded,
  onMockQuestionStarted,
  onMockQuestionSpoken,
  onMockSuggestedToken,
  onMockUserTranscribed,
} from "../events";
import CoachPanel from "../panels/CoachPanel";
import SuggestedAnswerPanel from "../panels/SuggestedAnswerPanel";
import { readShuffleQuestionsPreference, writeShuffleQuestionsPreference } from "../lib/shufflePreference";

export interface MockInterviewProps {
  sessionId: string;
  onComplete: () => void;
  onAbort: () => void;
}

type MockPace = "guided" | "continuous";

/** idle = pick mode; ready = guided, waiting for Ask question; waiting = expecting question event */
type TurnPhase = "idle" | "ready" | "waiting" | "speaking" | "question" | "answering" | "reviewing";

interface TurnState {
  turnN: number;
  totalQuestions: number;
  question: string;
  preferredHit: boolean;
  userTranscript: string;
  editTranscript: string;
  suggestedText: string;
  suggestedStreaming: boolean;
  coachFeedback: CoachFeedback | null;
  coachLoading: boolean;
  score: number;
}

const emptyTurn = (): TurnState => ({
  turnN: 0,
  totalQuestions: 0,
  question: "",
  preferredHit: false,
  userTranscript: "",
  editTranscript: "",
  suggestedText: "",
  suggestedStreaming: false,
  coachFeedback: null,
  coachLoading: false,
  score: 0,
});

const MockInterview = ({ sessionId: _sessionId, onComplete, onAbort }: MockInterviewProps) => {
  const [phase, setPhase] = useState<TurnPhase>("idle");
  const [pace, setPace] = useState<MockPace>("guided");
  const [studyMode, setStudyMode] = useState<MockStudyMode>("practice");
  const [shuffleQuestions, setShuffleQuestions] = useState(readShuffleQuestionsPreference);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [turn, setTurn] = useState<TurnState>(emptyTurn());
  const [recording, setRecording] = useState(false);
  const unlisteners = useRef<UnlistenFn[]>([]);
  const paceRef = useRef<MockPace>(pace);
  const studyModeRef = useRef<MockStudyMode>(studyMode);
  const beginAnsweringRef = useRef<(() => Promise<void>) | null>(null);
  const autoAdvanceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    paceRef.current = pace;
  }, [pace]);

  useEffect(() => {
    studyModeRef.current = studyMode;
  }, [studyMode]);

  const unlistenAll = useCallback(() => {
    unlisteners.current.forEach((fn) => fn());
    unlisteners.current = [];
  }, []);

  const beginAnswering = useCallback(async () => {
    setRecording(true);
    setTurn((t) => ({
      ...t,
      userTranscript: "",
      editTranscript: "",
      coachFeedback: null,
      coachLoading: false,
    }));
    setPhase("answering");
    try {
      await startMockTurn();
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    beginAnsweringRef.current = beginAnswering;
  }, [beginAnswering]);

  useEffect(() => {
    // Track whether the cleanup ran before the async setup resolved.
    // In React StrictMode, effects mount → unmount → remount synchronously.
    // Because setup() is async, the first-mount cleanup fires while the
    // Promise.all is still pending, leaving unlisteners.current empty and the
    // first set of listeners alive. The second mount then registers a second
    // set, so every event fires twice. The cancelled flag ensures any listeners
    // that resolve after cleanup are torn down immediately.
    let cancelled = false;

    const setup = async () => {
      const unlisten = await Promise.all([
        onMockQuestionStarted((p) => {
          setStudyMode(p.mode);
          setTurn(() => ({
            ...emptyTurn(),
            turnN: p.turn_n,
            totalQuestions: p.total_questions,
            question: p.question,
            preferredHit: Boolean(p.preferred_hit),
            suggestedStreaming: p.mode === "study" || Boolean(p.preferred_hit),
          }));
          setRecording(false);
          setPhase("speaking");
        }),
        onMockQuestionSpoken((p) => {
          setTurn((t) => (p.turn_n === t.turnN ? t : t));
          if (paceRef.current === "continuous") {
            setPhase("answering");
            void beginAnsweringRef.current?.();
          } else {
            setPhase("question");
          }
        }),
        onMockUserTranscribed((p) => {
          setTurn((t) => ({
            ...t,
            userTranscript: t.userTranscript + (t.userTranscript ? " " : "") + p.text,
          }));
        }),
        onMockSuggestedToken((p) => {
          setTurn((t) => ({
            ...t,
            suggestedText: t.suggestedText + p.token,
            suggestedStreaming: studyModeRef.current === "study" && t.suggestedStreaming,
          }));
        }),
        onMockCoachFeedback((p) => {
          setTurn((t) => {
            if (p.turn_n !== t.turnN) {
              return t;
            }
            try {
              const fb = JSON.parse(p.coach_json) as CoachFeedback;
              return {
                ...t,
                coachFeedback: fb,
                coachLoading: false,
                score: p.score,
                editTranscript: t.userTranscript,
              };
            } catch {
              return { ...t, coachLoading: false, editTranscript: t.userTranscript };
            }
          });
          setPhase("reviewing");
        }),
        onMockEnded(() => {
          void stopMock(false)
            .catch(() => undefined)
            .finally(() => {
              onComplete();
            });
        }),
      ]);

      if (cancelled) {
        unlisten.forEach((fn) => fn());
        return;
      }
      unlisteners.current = unlisten;
    };

    void setup();
    return () => {
      cancelled = true;
      if (autoAdvanceRef.current) clearTimeout(autoAdvanceRef.current);
      unlistenAll();
    };
  }, [unlistenAll, onComplete]);

  useEffect(() => {
    if (autoAdvanceRef.current) {
      clearTimeout(autoAdvanceRef.current);
      autoAdvanceRef.current = null;
    }
    if (phase !== "reviewing" || pace !== "continuous") return;
    if (turn.coachLoading || !turn.coachFeedback) return;
    autoAdvanceRef.current = setTimeout(() => {
      void advanceMockTurn().catch((e) => setError(String(e)));
    }, 2500);
    return () => {
      if (autoAdvanceRef.current) clearTimeout(autoAdvanceRef.current);
    };
  }, [phase, pace, turn.coachLoading, turn.coachFeedback]);

  const handleStartSession = async () => {
    setError(null);
    setStarting(true);
    try {
      await startMock(pace === "guided", studyMode, shuffleQuestions);
      setPhase(pace === "guided" ? "ready" : "waiting");
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  };

  const handleAskQuestion = async () => {
    setError(null);
    setPhase("waiting");
    try {
      await askMockQuestion();
    } catch (e) {
      setError(String(e));
      setPhase("ready");
    }
  };

  const handleAdvance = async () => {
    setError(null);
    try {
      await advanceMockTurn();
      if (pace === "guided") {
        setPhase("ready");
        setTurn(emptyTurn());
      } else {
        setPhase("waiting");
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const handleRetry = async () => {
    setError(null);
    try {
      await retryMockTurn();
      setTurn((t) => ({
        ...t,
        userTranscript: "",
        editTranscript: "",
        coachFeedback: null,
        coachLoading: false,
        suggestedText: "",
        suggestedStreaming: studyMode === "study" || t.preferredHit,
      }));
      setRecording(false);
      setPhase("speaking");
    } catch (e) {
      setError(String(e));
    }
  };

  const handleRegrade = async () => {
    setError(null);
    setTurn((t) => ({ ...t, coachLoading: true }));
    try {
      await regradeMockTurn(turn.editTranscript);
    } catch (e) {
      setError(String(e));
      setTurn((t) => ({ ...t, coachLoading: false }));
    }
  };

  const handleStopAnswering = async () => {
    setRecording(false);
    setTurn((t) => ({ ...t, coachLoading: true, suggestedStreaming: false }));
    try {
      await endMockTurn();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSkip = async () => {
    setRecording(false);
    try {
      await skipMockTurn();
      setPhase(pace === "guided" ? "ready" : "waiting");
    } catch (e) {
      setError(String(e));
    }
  };

  const resetToPicker = useCallback(() => {
    setPhase("idle");
    setTurn(emptyTurn());
    setRecording(false);
    setError(null);
    setStarting(false);
  }, []);

  const handleFinishEarly = async () => {
    setError(null);
    try {
      if (phase === "reviewing") {
        await advanceMockTurn();
      } else if (phase === "answering" && recording) {
        setRecording(false);
        setTurn((t) => ({ ...t, coachLoading: true }));
        await endMockTurn();
        return;
      }
      await stopMock(true);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleCancel = async () => {
    setError(null);
    try {
      await stopMock(false);
    } catch {
      // best effort
    }
    resetToPicker();
  };

  const showSuggested =
    studyMode === "study" ||
    (turn.suggestedText.length > 0 &&
      (turn.coachFeedback !== null || phase === "reviewing"));

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        background: "#080a0f",
        color: "#e2e8f0",
        fontFamily: "system-ui, sans-serif",
        overflow: "hidden",
      }}
    >
      {/* Header */}
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          padding: "12px 16px",
          borderBottom: "1px solid #1e2028",
          flexShrink: 0,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <span
            style={{
              width: 8,
              height: 8,
              borderRadius: "50%",
              background: "#7c3aed",
              display: "inline-block",
            }}
          />
          <span style={{ fontSize: "13px", fontWeight: 600, color: "#a78bfa" }}>
            Mock Interview
          </span>
          {turn.totalQuestions > 0 && (
            <span style={{ color: "#475569", fontSize: "11px" }}>
              {turn.turnN} / {turn.totalQuestions}
            </span>
          )}
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {phase !== "idle" && (
            <>
              <button
                type="button"
                data-testid="mock-finish-early-button"
                onClick={() => void handleFinishEarly()}
                style={{
                  background: "none",
                  border: "1px solid #7c3aed",
                  color: "#c4b5fd",
                  borderRadius: 5,
                  padding: "4px 10px",
                  fontSize: "12px",
                  cursor: "pointer",
                }}
              >
                End & review
              </button>
              <button
                type="button"
                data-testid="mock-cancel-button"
                onClick={() => void handleCancel()}
                style={{
                  background: "none",
                  border: "1px solid #374151",
                  color: "#94a3b8",
                  borderRadius: 5,
                  padding: "4px 10px",
                  fontSize: "12px",
                  cursor: "pointer",
                }}
              >
                Cancel
              </button>
            </>
          )}
          {phase === "idle" && (
            <button
              type="button"
              onClick={() => onAbort()}
              style={{
                background: "none",
                border: "1px solid #374151",
                color: "#94a3b8",
                borderRadius: 5,
                padding: "4px 10px",
                fontSize: "12px",
                cursor: "pointer",
              }}
            >
              Back to rehearsal
            </button>
          )}
        </div>
      </div>

      {error && (
        <div
          style={{
            background: "#7f1d1d22",
            border: "1px solid #7f1d1d",
            borderRadius: 6,
            margin: "8px 12px",
            padding: "8px 12px",
            color: "#fca5a5",
            fontSize: "12px",
          }}
        >
          {error}
        </div>
      )}

      {/* Main content */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "12px 16px",
          display: "flex",
          flexDirection: "column",
          gap: 12,
        }}
      >
        {phase === "idle" && (
          <div
            style={{
              background: "#111827",
              border: "1px solid #1e2028",
              borderRadius: 8,
              padding: "16px",
              display: "flex",
              flexDirection: "column",
              gap: 14,
            }}
          >
            <p style={{ margin: 0, fontSize: "14px", lineHeight: 1.6, color: "#cbd5e1" }}>
              Choose how questions are delivered and whether you practice from memory or study
              a script aloud.
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <span style={{ fontSize: "11px", fontWeight: 600, color: "#64748b", letterSpacing: "0.05em" }}>
                MODE — ASKING VS PREPARING
              </span>
              <label
                style={{
                  display: "flex",
                  gap: 10,
                  alignItems: "flex-start",
                  padding: "10px 12px",
                  borderRadius: 6,
                  border: studyMode === "practice" ? "1px solid #7c3aed" : "1px solid #374151",
                  cursor: "pointer",
                }}
              >
                <input
                  type="radio"
                  name="mock-study-mode"
                  checked={studyMode === "practice"}
                  onChange={() => setStudyMode("practice")}
                  style={{ marginTop: 3 }}
                />
                <span>
                  <strong style={{ color: "#e2e8f0" }}>Practice — answer yourself</strong>
                  <br />
                  <span style={{ fontSize: "12px", color: "#94a3b8" }}>
                    Suggested answer stays hidden until you finish. Coach scores content and
                    delivery.
                  </span>
                </span>
              </label>
              <label
                style={{
                  display: "flex",
                  gap: 10,
                  alignItems: "flex-start",
                  padding: "10px 12px",
                  borderRadius: 6,
                  border: studyMode === "study" ? "1px solid #7c3aed" : "1px solid #374151",
                  cursor: "pointer",
                }}
              >
                <input
                  type="radio"
                  name="mock-study-mode"
                  checked={studyMode === "study"}
                  onChange={() => setStudyMode("study")}
                  style={{ marginTop: 3 }}
                />
                <span>
                  <strong style={{ color: "#e2e8f0" }}>Study — see suggested answers</strong>
                  <br />
                  <span style={{ fontSize: "12px", color: "#94a3b8" }}>
                    Full script streams while you answer. Coach scores delivery (pace, filler),
                    not content depth.
                  </span>
                </span>
              </label>
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <span style={{ fontSize: "11px", fontWeight: 600, color: "#64748b", letterSpacing: "0.05em" }}>
                QUESTION ORDER
              </span>
              <label
                style={{
                  display: "flex",
                  gap: 10,
                  alignItems: "flex-start",
                  padding: "10px 12px",
                  borderRadius: 6,
                  border: shuffleQuestions ? "1px solid #7c3aed" : "1px solid #374151",
                  cursor: "pointer",
                }}
              >
                <input
                  type="checkbox"
                  checked={shuffleQuestions}
                  onChange={(e) => {
                    setShuffleQuestions(e.target.checked);
                    writeShuffleQuestionsPreference(e.target.checked);
                  }}
                  style={{ marginTop: 3 }}
                  data-testid="mock-shuffle-questions"
                />
                <span>
                  <strong style={{ color: "#e2e8f0" }}>Shuffle question order</strong>
                  <br />
                  <span style={{ fontSize: "12px", color: "#94a3b8" }}>
                    Randomize digest questions each session (stable for that session). Up to
                    three AI follow-ups still run after scripted questions.
                  </span>
                </span>
              </label>
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <span style={{ fontSize: "11px", fontWeight: 600, color: "#64748b", letterSpacing: "0.05em" }}>
                PACE — ONE QUESTION AT A TIME VS CONTINUOUS
              </span>
              <label
                style={{
                  display: "flex",
                  gap: 10,
                  alignItems: "flex-start",
                  padding: "10px 12px",
                  borderRadius: 6,
                  border: pace === "guided" ? "1px solid #7c3aed" : "1px solid #374151",
                  cursor: "pointer",
                }}
              >
                <input
                  type="radio"
                  name="mock-pace"
                  checked={pace === "guided"}
                  onChange={() => setPace("guided")}
                  style={{ marginTop: 3 }}
                />
                <span>
                  <strong style={{ color: "#e2e8f0" }}>One question at a time</strong>
                  <br />
                  <span style={{ fontSize: "12px", color: "#94a3b8" }}>
                    Click &quot;Ask question&quot; for each prompt. End anytime with &quot;End &amp;
                    review&quot; or cancel to pick options again.
                  </span>
                </span>
              </label>
              <label
                style={{
                  display: "flex",
                  gap: 10,
                  alignItems: "flex-start",
                  padding: "10px 12px",
                  borderRadius: 6,
                  border: pace === "continuous" ? "1px solid #7c3aed" : "1px solid #374151",
                  cursor: "pointer",
                }}
              >
                <input
                  type="radio"
                  name="mock-pace"
                  checked={pace === "continuous"}
                  onChange={() => setPace("continuous")}
                  style={{ marginTop: 3 }}
                />
                <span>
                  <strong style={{ color: "#e2e8f0" }}>Continuous (live-like)</strong>
                  <br />
                  <span style={{ fontSize: "12px", color: "#94a3b8" }}>
                    Questions play in sequence; the mic opens automatically after each one.
                  </span>
                </span>
              </label>
            </div>
            <button
              type="button"
              data-testid="mock-start-session-button"
              onClick={() => void handleStartSession()}
              disabled={starting}
              style={{
                ...primaryBtn,
                alignSelf: "flex-start",
                opacity: starting ? 0.6 : 1,
              }}
            >
              {starting ? "Starting…" : "Start Mock Interview"}
            </button>
          </div>
        )}

        {phase === "ready" && pace === "guided" && !turn.question && (
          <p style={{ margin: 0, fontSize: "13px", color: "#94a3b8" }}>
            Session ready. Click <strong>Ask question</strong> when you want the interviewer to
            speak the next prompt.
          </p>
        )}

        {turn.question && (
          <div
            style={{
              background: "#111827",
              border: "1px solid #1e2028",
              borderLeft: "3px solid #7c3aed",
              borderRadius: 8,
              padding: "12px 14px",
            }}
          >
            <div
              style={{
                color: "#7c3aed",
                fontSize: "10px",
                fontWeight: 600,
                marginBottom: 6,
                letterSpacing: "0.06em",
              }}
            >
              INTERVIEWER
            </div>
            <p style={{ margin: 0, fontSize: "14px", lineHeight: 1.6 }}>{turn.question}</p>
            {turn.preferredHit && (
              <p
                style={{
                  margin: "8px 0 0",
                  fontSize: "11px",
                  color: "#22c55e",
                  fontWeight: 600,
                }}
              >
                Using your saved Live script
              </p>
            )}
          </div>
        )}

        {phase !== "idle" && showSuggested && (
          <SuggestedAnswerPanel
            text={turn.suggestedText}
            isStreaming={turn.suggestedStreaming}
          />
        )}

        {phase !== "idle" && studyMode === "practice" && !showSuggested && (
          <div
            style={{
              background: "#0f1117",
              border: "1px dashed #374151",
              borderRadius: 8,
              padding: "10px 14px",
              color: "#64748b",
              fontSize: "12px",
            }}
          >
            {phase === "answering" || phase === "question"
              ? "Answer in your own words — suggested answer unlocks after you finish."
              : turn.coachLoading
                ? "Analyzing your answer…"
                : "Suggested answer appears after coach feedback."}
          </div>
        )}

        {(phase === "answering" || phase === "reviewing") && (
          <div
            style={{
              background: "#111827",
              border: "1px solid #1e2028",
              borderLeft: "3px solid #22c55e",
              borderRadius: 8,
              padding: "12px 14px",
            }}
          >
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                marginBottom: 6,
              }}
            >
              <span
                style={{
                  color: "#22c55e",
                  fontSize: "10px",
                  fontWeight: 600,
                  letterSpacing: "0.06em",
                }}
              >
                YOUR ANSWER
              </span>
              {recording && (
                <span
                  style={{
                    color: "#ef4444",
                    fontSize: "10px",
                    display: "flex",
                    alignItems: "center",
                    gap: 4,
                  }}
                >
                  <span
                    style={{
                      width: 6,
                      height: 6,
                      borderRadius: "50%",
                      background: "#ef4444",
                      display: "inline-block",
                    }}
                  />
                  REC
                </span>
              )}
            </div>
            {phase === "reviewing" ? (
              <textarea
                value={turn.editTranscript}
                onChange={(e) =>
                  setTurn((t) => ({ ...t, editTranscript: e.target.value }))
                }
                rows={4}
                style={{
                  width: "100%",
                  boxSizing: "border-box",
                  background: "#0f1117",
                  border: "1px solid #374151",
                  borderRadius: 6,
                  color: "#e2e8f0",
                  fontSize: "13px",
                  lineHeight: 1.6,
                  padding: "8px 10px",
                  resize: "vertical",
                }}
              />
            ) : (
              <p
                style={{
                  margin: 0,
                  fontSize: "13px",
                  lineHeight: 1.6,
                  color: turn.userTranscript ? "#e2e8f0" : "#475569",
                }}
              >
                {turn.userTranscript || (recording ? "Listening…" : "No answer recorded.")}
              </p>
            )}
          </div>
        )}

        {phase !== "idle" && (
          <CoachPanel
            feedback={turn.coachFeedback}
            isLoading={turn.coachLoading}
            score={turn.score}
          />
        )}
      </div>

      {/* Footer controls */}
      <div
        style={{
          padding: "10px 16px",
          borderTop: "1px solid #1e2028",
          display: "flex",
          justifyContent: "flex-end",
          gap: 10,
          flexShrink: 0,
        }}
      >
        {phase === "ready" && pace === "guided" && (
          <button
            type="button"
            data-testid="mock-ask-question-button"
            onClick={() => void handleAskQuestion()}
            style={primaryBtn}
          >
            Ask question
          </button>
        )}

        {phase === "waiting" && (
          <span style={{ color: "#52525b", fontSize: "12px", alignSelf: "center" }}>
            {pace === "continuous" ? "Next question incoming…" : "Preparing question…"}
          </span>
        )}

        {phase === "speaking" && (
          <>
            <span style={{ color: "#52525b", fontSize: "12px", alignSelf: "center", flex: 1 }}>
              Speaking question…
            </span>
            <button onClick={() => void handleSkip()} style={ghostBtn}>
              Skip
            </button>
          </>
        )}

        {phase === "question" && (
          <>
            <button onClick={() => void handleSkip()} style={ghostBtn}>
              Skip
            </button>
            <button onClick={() => void beginAnswering()} style={primaryBtn}>
              Start Answering
            </button>
          </>
        )}

        {phase === "answering" && (
          <>
            <button onClick={() => void handleSkip()} style={ghostBtn}>
              Skip
            </button>
            <button
              onClick={() => void handleStopAnswering()}
              style={{ ...primaryBtn, background: "#ef4444" }}
            >
              Done Answering
            </button>
          </>
        )}

        {phase === "reviewing" && (
          <>
            <button
              type="button"
              data-testid="mock-regrade-button"
              onClick={() => void handleRegrade()}
              disabled={turn.coachLoading || !turn.editTranscript.trim()}
              style={ghostBtn}
            >
              Re-grade
            </button>
            <button
              type="button"
              data-testid="mock-retry-button"
              onClick={() => void handleRetry()}
              style={ghostBtn}
            >
              Try again
            </button>
            <button
              type="button"
              data-testid="mock-next-button"
              onClick={() => void handleAdvance()}
              style={primaryBtn}
            >
              {pace === "guided" ? "Next question" : "Continue"}
            </button>
          </>
        )}

        {phase === "reviewing" && pace === "continuous" && (
          <span style={{ color: "#52525b", fontSize: "12px", alignSelf: "center", flex: 1 }}>
            Auto-advancing in a moment…
          </span>
        )}
      </div>
    </div>
  );
};

const primaryBtn: React.CSSProperties = {
  padding: "8px 18px",
  background: "#7c3aed",
  color: "#fff",
  border: "none",
  borderRadius: 6,
  fontSize: "13px",
  fontWeight: 600,
  cursor: "pointer",
};

const ghostBtn: React.CSSProperties = {
  padding: "8px 14px",
  background: "none",
  border: "1px solid #374151",
  color: "#94a3b8",
  borderRadius: 6,
  fontSize: "13px",
  cursor: "pointer",
};

export default MockInterview;
