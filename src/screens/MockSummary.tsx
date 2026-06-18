import { useEffect, useRef, useState } from "react";

import { getMockTurns, readMockAudioDataUrl, type CoachFeedback, type MockTurn } from "../commands";
import "./MockSummary.css";

interface Props {
  onContinue: () => void;
}

function parseCoach(raw: string): CoachFeedback | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as CoachFeedback;
  } catch {
    return null;
  }
}

function isSkippedTurn(turn: MockTurn): boolean {
  return !turn.user_text.trim() && !turn.audio_path;
}

const scoreLabel = (score: number): string => {
  if (score >= 80) return "Strong";
  if (score >= 60) return "Good";
  if (score >= 40) return "Needs work";
  return "Weak";
};

const scoreClass = (score: number): string => {
  if (score >= 80) return "ms-score--high";
  if (score >= 60) return "ms-score--mid";
  return "ms-score--low";
};

function TurnCard({ turn, index }: { turn: MockTurn; index: number }) {
  const [expanded, setExpanded] = useState(index === 0);
  const [audioError, setAudioError] = useState(false);
  const [audioSrc, setAudioSrc] = useState<string | null>(null);
  const [audioLoading, setAudioLoading] = useState(false);
  const coach = parseCoach(turn.coach_json);
  const skipped = isSkippedTurn(turn);

  useEffect(() => {
    if (!turn.audio_path || skipped) {
      setAudioSrc(null);
      setAudioError(false);
      setAudioLoading(false);
      return;
    }

    let cancelled = false;
    setAudioLoading(true);
    setAudioError(false);
    void readMockAudioDataUrl(turn.audio_path)
      .then((url) => {
        if (!cancelled) {
          setAudioSrc(url);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setAudioError(true);
          setAudioSrc(null);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setAudioLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [turn.audio_path, skipped]);

  return (
    <div className="ms-turn-card" data-expanded={expanded}>
      <button
        className="ms-turn-header"
        onClick={() => setExpanded((p) => !p)}
        aria-expanded={expanded}
      >
        <span className="ms-turn-num">Q{turn.turn_n}</span>
        <span className="ms-turn-question">{turn.question}</span>
        {skipped && <span className="ms-skipped-badge">Skipped</span>}
        {!skipped && turn.score > 0 && (
          <span className={`ms-score ${scoreClass(turn.score)}`}>
            {turn.score} · {scoreLabel(turn.score)}
          </span>
        )}
        <span className="ms-turn-chevron">{expanded ? "▲" : "▼"}</span>
      </button>

      {expanded && (
        <div className="ms-turn-body">
          {skipped && (
            <p className="ms-skipped-note">You skipped this question — it is excluded from your average score.</p>
          )}

          {turn.user_text && (
            <div className="ms-section">
              <div className="ms-section-label">YOUR ANSWER</div>
              <p className="ms-body-text">{turn.user_text}</p>
            </div>
          )}

          {audioSrc && (
            <div className="ms-section">
              <div className="ms-section-label">RECORDING</div>
              {audioError ? (
                <p className="ms-audio-error">Could not play recording. The file may have been moved or removed.</p>
              ) : (
                <audio
                  className="ms-audio"
                  controls
                  src={audioSrc}
                  preload="metadata"
                  aria-label={`Recording for question ${turn.turn_n}`}
                  onError={() => setAudioError(true)}
                />
              )}
            </div>
          )}

          {!audioSrc && turn.audio_path && !skipped && audioLoading && (
            <div className="ms-section">
              <div className="ms-section-label">RECORDING</div>
              <p className="ms-body-text">Loading recording…</p>
            </div>
          )}

          {!audioSrc && turn.audio_path && !skipped && audioError && (
            <div className="ms-section">
              <div className="ms-section-label">RECORDING</div>
              <p className="ms-audio-error">Could not play recording. The file may have been moved or removed.</p>
            </div>
          )}

          {coach && !skipped && (
            <>
              {coach.tone?.suggestion && (
                <div className="ms-section">
                  <div className="ms-section-label">TONE</div>
                  <p className="ms-body-text">
                    <strong className="ms-tone-badge">{coach.tone.assessment}</strong>
                    {" — "}
                    {coach.tone.suggestion}
                  </p>
                </div>
              )}

              {coach.context_gaps?.length > 0 && (
                <div className="ms-section">
                  <div className="ms-section-label">GAPS</div>
                  <ul className="ms-list">
                    {coach.context_gaps.map((gap, i) => (
                      <li key={i}>{gap}</li>
                    ))}
                  </ul>
                </div>
              )}

              {coach.corrected_answer && (
                <div className="ms-section">
                  <div className="ms-section-label">POLISHED ANSWER</div>
                  <p className="ms-polished">{coach.corrected_answer}</p>
                </div>
              )}
            </>
          )}

          {turn.suggested && !skipped && !coach?.corrected_answer && (
            <div className="ms-section">
              <div className="ms-section-label">SUGGESTED ANSWER</div>
              <p className="ms-suggested">{turn.suggested}</p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function MockSummary({ onContinue }: Props) {
  const [turns, setTurns] = useState<MockTurn[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const fetchedRef = useRef(false);

  useEffect(() => {
    if (fetchedRef.current) return;
    fetchedRef.current = true;

    getMockTurns()
      .then((data) => {
        setTurns(data);
        setLoading(false);
      })
      .catch((err: unknown) => {
        setError(String(err));
        setLoading(false);
      });
  }, []);

  const skippedTurns = turns.filter(isSkippedTurn);
  const scoredTurns = turns.filter((t) => t.score > 0);
  const avgScore =
    scoredTurns.length > 0
      ? Math.round(scoredTurns.reduce((s, t) => s + t.score, 0) / scoredTurns.length)
      : 0;

  if (loading) {
    return (
      <main className="mock-summary" data-testid="mock-summary-loading">
        <p className="ms-loading">Loading interview results…</p>
      </main>
    );
  }

  if (error || turns.length === 0) {
    return (
      <main className="mock-summary" data-testid="mock-summary-error">
        <p className="ms-error">{error ?? "No turns recorded for this session."}</p>
        <button className="ms-continue-btn" onClick={onContinue}>
          Continue
        </button>
      </main>
    );
  }

  return (
    <main className="mock-summary" data-testid="mock-summary">
      <header className="ms-header">
        <h1 className="ms-title">Mock Interview Complete</h1>
        <div className="ms-stats">
          <span className="ms-stat">
            <span className="ms-stat-value">{turns.length}</span>
            <span className="ms-stat-label">questions</span>
          </span>
          {skippedTurns.length > 0 && (
            <span className="ms-stat">
              <span className="ms-stat-value">{skippedTurns.length}</span>
              <span className="ms-stat-label">skipped</span>
            </span>
          )}
          {avgScore > 0 && (
            <span className={`ms-stat ms-stat--score ${scoreClass(avgScore)}`}>
              <span className="ms-stat-value">{avgScore}</span>
              <span className="ms-stat-label">avg score</span>
            </span>
          )}
        </div>
      </header>

      <section className="ms-turns">
        {turns.map((turn, i) => (
          <TurnCard key={`${turn.turn_n}-${turn.id}`} turn={turn} index={i} />
        ))}
      </section>

      <footer className="ms-footer">
        <button className="ms-continue-btn" onClick={onContinue}>
          Back to Rehearsal
        </button>
      </footer>
    </main>
  );
}

export default MockSummary;
