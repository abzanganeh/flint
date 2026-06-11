import { useCallback, useEffect, useRef, useState } from "react";
import { type UnlistenFn } from "@tauri-apps/api/event";

import {
  endMockTurn,
  skipMockTurn,
  startMock,
  startMockTurn,
  stopMock,
  type CoachFeedback,
} from "../commands";
import {
  onMockCoachFeedback,
  onMockEnded,
  onMockQuestionStarted,
  onMockSuggestedToken,
  onMockUserTranscribed,
} from "../events";
import CoachPanel from "../panels/CoachPanel";
import SuggestedAnswerPanel from "../panels/SuggestedAnswerPanel";

export interface MockInterviewProps {
  sessionId: string;
  onComplete: () => void;
  onAbort: () => void;
}

type TurnPhase = "waiting" | "question" | "answering" | "reviewing";

interface TurnState {
  turnN: number;
  totalQuestions: number;
  question: string;
  userTranscript: string;
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
  userTranscript: "",
  suggestedText: "",
  suggestedStreaming: false,
  coachFeedback: null,
  coachLoading: false,
  score: 0,
});

const MockInterview = ({ sessionId: _sessionId, onComplete, onAbort }: MockInterviewProps) => {
  const [phase, setPhase] = useState<TurnPhase>("waiting");
  const [error, setError] = useState<string | null>(null);
  const [turn, setTurn] = useState<TurnState>(emptyTurn());
  const [recording, setRecording] = useState(false);
  const unlisteners = useRef<UnlistenFn[]>([]);

  const unlistenAll = useCallback(() => {
    unlisteners.current.forEach((fn) => fn());
    unlisteners.current = [];
  }, []);

  useEffect(() => {
    const setup = async () => {
      const unlisten = await Promise.all([
        onMockQuestionStarted((p) => {
          setTurn(() => ({
            ...emptyTurn(),
            turnN: p.turn_n,
            totalQuestions: p.total_questions,
            question: p.question,
            suggestedStreaming: true,
          }));
          setPhase("question");
          setRecording(false);
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
          }));
        }),
        onMockCoachFeedback((p) => {
          try {
            const fb = JSON.parse(p.coach_json) as CoachFeedback;
            setTurn((t) => ({
              ...t,
              coachFeedback: fb,
              coachLoading: false,
              score: p.score,
            }));
          } catch {
            setTurn((t) => ({ ...t, coachLoading: false }));
          }
          setPhase("reviewing");
        }),
        onMockEnded(() => {
          // The conductor emits `mock_ended` once the question list is
          // exhausted, but the state machine and mic resources are still on
          // MOCK_INTERVIEW. Call `stopMock` to transition back to REHEARSING
          // and shut the mic capture down before navigating away — otherwise
          // any follow-up Rehearsal action errors out with an invalid
          // transition.
          void stopMock()
            .catch(() => {
              // Best-effort: backend may already have torn down on abort.
            })
            .finally(() => {
              onComplete();
            });
        }),
      ]);
      unlisteners.current = unlisten;

      // Start the mock session.
      try {
        await startMock();
        setPhase("waiting");
      } catch (e) {
        setError(String(e));
      }
    };

    void setup();
    return () => unlistenAll();
  }, [unlistenAll, onComplete]);

  const handleStartAnswering = async () => {
    setRecording(true);
    setTurn((t) => ({ ...t, userTranscript: "", coachFeedback: null, coachLoading: false }));
    setPhase("answering");
    try {
      await startMockTurn();
    } catch (e) {
      setError(String(e));
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
      setPhase("waiting");
    } catch (e) {
      setError(String(e));
    }
  };

  const handleAbort = async () => {
    unlistenAll();
    try {
      await stopMock();
    } catch {
      // best effort
    }
    onAbort();
  };

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
        <button
          onClick={() => void handleAbort()}
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
          Exit
        </button>
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
      <div style={{ flex: 1, overflowY: "auto", padding: "12px 16px", display: "flex", flexDirection: "column", gap: 12 }}>

        {/* Question bubble */}
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
            <div style={{ color: "#7c3aed", fontSize: "10px", fontWeight: 600, marginBottom: 6, letterSpacing: "0.06em" }}>
              INTERVIEWER
            </div>
            <p style={{ margin: 0, fontSize: "14px", lineHeight: 1.6 }}>{turn.question}</p>
          </div>
        )}

        {/* Suggested answer (streams while AI question is displayed) */}
        <SuggestedAnswerPanel
          text={turn.suggestedText}
          isStreaming={turn.suggestedStreaming}
        />

        {/* User transcript */}
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
              <span style={{ color: "#22c55e", fontSize: "10px", fontWeight: 600, letterSpacing: "0.06em" }}>
                YOUR ANSWER
              </span>
              {recording && (
                <span style={{ color: "#ef4444", fontSize: "10px", display: "flex", alignItems: "center", gap: 4 }}>
                  <span style={{ width: 6, height: 6, borderRadius: "50%", background: "#ef4444", display: "inline-block" }} />
                  REC
                </span>
              )}
            </div>
            <p style={{ margin: 0, fontSize: "13px", lineHeight: 1.6, color: turn.userTranscript ? "#e2e8f0" : "#475569" }}>
              {turn.userTranscript || (recording ? "Listening…" : "No answer recorded.")}
            </p>
          </div>
        )}

        {/* Coach panel */}
        <CoachPanel
          feedback={turn.coachFeedback}
          isLoading={turn.coachLoading}
          score={turn.score}
        />
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
        {phase === "waiting" && (
          <span style={{ color: "#52525b", fontSize: "12px", alignSelf: "center" }}>
            Waiting for question…
          </span>
        )}

        {phase === "question" && (
          <>
            <button
              onClick={() => void handleSkip()}
              style={ghostBtn}
            >
              Skip
            </button>
            <button
              onClick={() => void handleStartAnswering()}
              style={primaryBtn}
            >
              Start Answering
            </button>
          </>
        )}

        {phase === "answering" && (
          <button
            onClick={() => void handleStopAnswering()}
            style={{ ...primaryBtn, background: "#ef4444" }}
          >
            Done Answering
          </button>
        )}

        {phase === "reviewing" && (
          <>
            <span style={{ color: "#52525b", fontSize: "12px", alignSelf: "center", flex: 1 }}>
              Next question incoming…
            </span>
          </>
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
