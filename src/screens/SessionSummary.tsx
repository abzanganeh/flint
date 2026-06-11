import { useEffect, useRef, useState } from "react";
import { generateSessionSummary } from "../commands";

interface SessionEssence {
  date: string;
  domain: string;
  role: string;
  company: string;
  questions_count: number;
  topics_covered: string[];
  confidence_distribution: { high: number; medium: number; low: number };
  key_moments: string[];
  follow_up_actions: string[];
  one_line_summary: string;
}

interface Props {
  onDone: () => void;
}

function parseEssence(raw: string): SessionEssence | null {
  try {
    const data = JSON.parse(raw) as Partial<SessionEssence>;
    return {
      date: data.date ?? "",
      domain: data.domain ?? "",
      role: data.role ?? "",
      company: data.company ?? "",
      questions_count: data.questions_count ?? 0,
      topics_covered: data.topics_covered ?? [],
      confidence_distribution: data.confidence_distribution ?? {
        high: 0,
        medium: 0,
        low: 0,
      },
      key_moments: data.key_moments ?? [],
      follow_up_actions: data.follow_up_actions ?? [],
      one_line_summary: data.one_line_summary ?? "",
    };
  } catch {
    return null;
  }
}

export function SessionSummary({ onDone }: Props) {
  const [essence, setEssence] = useState<SessionEssence | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const fetchedRef = useRef(false);

  useEffect(() => {
    if (fetchedRef.current) return;
    fetchedRef.current = true;

    generateSessionSummary()
      .then((raw) => {
        setEssence(parseEssence(raw));
        setLoading(false);
      })
      .catch((err: unknown) => {
        setError(String(err));
        setLoading(false);
      });
  }, []);

  if (loading) {
    return (
      <main className="session-summary" data-testid="session-summary-loading">
        <p className="ss-loading">Generating session summary…</p>
      </main>
    );
  }

  if (error || !essence) {
    return (
      <main className="session-summary" data-testid="session-summary-error">
        <p className="ss-error">
          {error ?? "Summary unavailable for this session."}
        </p>
        <button className="ss-done-btn" onClick={onDone}>
          Back to sessions
        </button>
      </main>
    );
  }

  const { high, medium, low } = essence.confidence_distribution;
  const total = high + medium + low || 1;

  return (
    <main className="session-summary" data-testid="session-summary">
      <header className="ss-header">
        <h1 className="ss-title">Session complete</h1>
        {essence.one_line_summary && (
          <p className="ss-tagline">{essence.one_line_summary}</p>
        )}
      </header>

      <section className="ss-meta">
        {essence.role && <span className="ss-chip">{essence.role}</span>}
        {essence.company && <span className="ss-chip">{essence.company}</span>}
        {essence.domain && <span className="ss-chip">{essence.domain}</span>}
        <span className="ss-chip">{essence.questions_count} questions</span>
      </section>

      <section className="ss-confidence">
        <h2 className="ss-section-heading">Confidence breakdown</h2>
        <div className="ss-conf-bar" role="group" aria-label="Confidence breakdown">
          <div
            className="ss-conf-segment ss-conf-high"
            style={{ width: `${(high / total) * 100}%` }}
            title={`High: ${high}`}
          />
          <div
            className="ss-conf-segment ss-conf-medium"
            style={{ width: `${(medium / total) * 100}%` }}
            title={`Medium: ${medium}`}
          />
          <div
            className="ss-conf-segment ss-conf-low"
            style={{ width: `${(low / total) * 100}%` }}
            title={`Low: ${low}`}
          />
        </div>
        <div className="ss-conf-legend">
          <span>High {high}</span>
          <span>Medium {medium}</span>
          <span>Low {low}</span>
        </div>
      </section>

      {essence.topics_covered.length > 0 && (
        <section className="ss-topics">
          <h2 className="ss-section-heading">Topics covered</h2>
          <ul className="ss-list">
            {essence.topics_covered.map((t) => (
              <li key={t}>{t}</li>
            ))}
          </ul>
        </section>
      )}

      {essence.key_moments.length > 0 && (
        <section className="ss-moments">
          <h2 className="ss-section-heading">Key moments</h2>
          <ul className="ss-list">
            {essence.key_moments.map((m, i) => (
              <li key={i}>{m}</li>
            ))}
          </ul>
        </section>
      )}

      {essence.follow_up_actions.length > 0 && (
        <section className="ss-actions">
          <h2 className="ss-section-heading">Follow-up actions</h2>
          <ul className="ss-list ss-actions-list">
            {essence.follow_up_actions.map((a, i) => (
              <li key={i}>
                <label className="ss-action-item">
                  <input type="checkbox" /> {a}
                </label>
              </li>
            ))}
          </ul>
        </section>
      )}

      <footer className="ss-footer">
        <button className="ss-done-btn" onClick={onDone}>
          Back to sessions
        </button>
      </footer>
    </main>
  );
}
