import React, { useCallback, useEffect, useState } from "react";
import type { DigestDto, SessionSummaryDto } from "../commands";
import {
  deleteSession,
  demoteSession,
  getDigest,
  getSessionContext,
  listSessions,
  promoteSession,
} from "../commands";
import type { SessionPreFill } from "./SessionDesign";
import "./SessionList.css";

interface Props {
  onBack: () => void;
  /** Called when the user wants to start a new session cloned from a past one. */
  onStartSimilar?: (preFill: SessionPreFill) => void;
}

function digestToContextText(digest: DigestDto): string {
  const lines: string[] = [];

  if (digest.role)     lines.push(`Role: ${digest.role}`);
  if (digest.company)  lines.push(`Company: ${digest.company}`);
  if (digest.domain)   lines.push(`Domain: ${digest.domain}`);
  if (digest.seniority) lines.push(`Seniority: ${digest.seniority}`);

  if (digest.keySkills.length > 0) {
    lines.push("", "Key Skills:");
    digest.keySkills.forEach((s) => lines.push(`- ${s}`));
  }

  if (digest.likelyQuestions.length > 0) {
    lines.push("", "Likely Questions:");
    digest.likelyQuestions.forEach((q) => lines.push(`- ${q}`));
  }

  if (digest.topicsToAvoid.length > 0) {
    lines.push("", "Topics to Avoid:");
    digest.topicsToAvoid.forEach((t) => lines.push(`- ${t}`));
  }

  return lines.join("\n");
}

function formatExpiry(expiresInSecs: number, promoted: boolean): string {
  if (promoted) return "Pinned";
  if (expiresInSecs <= 0) return "Expired";
  const days = Math.floor(expiresInSecs / 86400);
  if (days === 0) return "Expires today";
  return `${days}d remaining`;
}

function stateLabel(state: string): string {
  return state
    .replace(/_/g, " ")
    .toLowerCase()
    .replace(/^./, (c) => c.toUpperCase());
}

function expiryClass(expiresInSecs: number, promoted: boolean): string {
  if (promoted) return "sl-expiry--pinned";
  if (expiresInSecs <= 0) return "sl-expiry--expired";
  return "sl-expiry--normal";
}

export const SessionList: React.FC<Props> = ({ onBack, onStartSimilar }) => {
  const [sessions, setSessions] = useState<SessionSummaryDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionInFlight, setActionInFlight] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [cloning, setCloning] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const rows = await listSessions();
      setSessions(rows);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const handlePromote = useCallback(async (id: string) => {
    setActionInFlight(id + ":promote");
    try {
      await promoteSession(id);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setActionInFlight(null);
    }
  }, [load]);

  const handleDemote = useCallback(async (id: string) => {
    setActionInFlight(id + ":demote");
    try {
      await demoteSession(id);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setActionInFlight(null);
    }
  }, [load]);

  const handleStartSimilar = useCallback(async (session: SessionSummaryDto) => {
    if (!onStartSimilar) return;
    setCloning(session.id);
    setError(null);

    const base = {
      name: session.name,
      sessionType: session.sessionType,
      domain: session.domain,
    };

    try {
      const stored = await getSessionContext(session.id);
      if (stored.trim().length > 0) {
        onStartSimilar({ ...base, contextText: stored.trim() });
        return;
      }

      // Legacy sessions (before context_text column): reconstruct from digest.
      const digest = await getDigest(session.id);
      onStartSimilar({ ...base, contextText: digestToContextText(digest) });
    } catch {
      onStartSimilar(base);
      setError(
        "Could not load saved context for this session. Paste your JD again in Session context.",
      );
    } finally {
      setCloning(null);
    }
  }, [onStartSimilar]);

  const handleDelete = useCallback(async (id: string) => {
    setActionInFlight(id + ":delete");
    try {
      await deleteSession(id);
      setSessions((prev) => prev.filter((s) => s.id !== id));
    } catch (e) {
      setError(String(e));
    } finally {
      setActionInFlight(null);
    }
  }, []);

  return (
    <div className="sl-root">
      <header className="sl-header">
        <button className="sl-back-btn" onClick={onBack} type="button">
          ← Back
        </button>
        <h1>Past Sessions</h1>
        <button
          className="sl-refresh-btn"
          onClick={load}
          disabled={loading}
          type="button"
        >
          Refresh
        </button>
      </header>

      <main className="sl-body">
        {error && <div className="sl-error">{error}</div>}

        {loading && (
          <div className="sl-loading">
            <p>Loading sessions…</p>
          </div>
        )}

        {!loading && sessions.length === 0 && (
          <div className="sl-empty">
            <p>No past sessions.</p>
            <small>Completed sessions will appear here.</small>
          </div>
        )}

        <ul className="sl-list">
          {sessions.map((session) => {
            const expired = session.expiresInSecs <= 0 && !session.promoted;
            const date = new Date(session.createdAt * 1000);
            const dateLabel = date.toLocaleDateString(undefined, {
              month: "short",
              day: "numeric",
              year: "numeric",
            });
            const busy = actionInFlight !== null;

            const isSelected = selectedId === session.id;

            return (
              <li
                key={session.id}
                className={`sl-item${isSelected ? " sl-item--selected" : ""}`}
                onClick={() => setSelectedId(isSelected ? null : session.id)}
              >
                <div className={`sl-dot ${session.promoted ? "sl-dot--pinned" : expired ? "sl-dot--expired" : "sl-dot--active"}`} />

                <div className="sl-info">
                  <p className="sl-name">
                    {session.name || dateLabel}&thinsp;&mdash;&thinsp;{session.domain || stateLabel(session.state)}
                  </p>
                  <p className="sl-meta">
                    {session.name ? dateLabel : ""}
                    {session.name && session.sessionType ? ` · ${session.sessionType}` : ""}
                  </p>
                  <p className={`sl-expiry ${expiryClass(session.expiresInSecs, session.promoted)}`}>
                    {formatExpiry(session.expiresInSecs, session.promoted)}
                  </p>
                </div>

                <div className="sl-actions" onClick={(e) => e.stopPropagation()}>
                  {isSelected && onStartSimilar && (
                    <button
                      className="sl-action-btn sl-action-btn--clone"
                      onClick={() => void handleStartSimilar(session)}
                      disabled={busy || cloning === session.id}
                      type="button"
                      title="Open a new session pre-filled with this session's context"
                    >
                      {cloning === session.id ? "Loading…" : "Start similar"}
                    </button>
                  )}
                  {session.promoted ? (
                    <button
                      className="sl-action-btn sl-action-btn--unpin"
                      onClick={() => handleDemote(session.id)}
                      disabled={busy}
                      type="button"
                    >
                      {actionInFlight === session.id + ":demote" ? "Unpinning…" : "Unpin"}
                    </button>
                  ) : (
                    <button
                      className="sl-action-btn sl-action-btn--pin"
                      onClick={() => handlePromote(session.id)}
                      disabled={busy}
                      type="button"
                    >
                      {actionInFlight === session.id + ":promote" ? "Pinning…" : "Pin"}
                    </button>
                  )}
                  <button
                    className="sl-action-btn sl-action-btn--delete"
                    onClick={() => handleDelete(session.id)}
                    disabled={busy}
                    type="button"
                  >
                    {actionInFlight === session.id + ":delete" ? "Deleting…" : "Delete"}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>

        {sessions.length > 0 && (
          <p className="sl-footer-note">
            Sessions are kept for 30 days. Pin a session to exempt it from automatic deletion.
          </p>
        )}
      </main>
    </div>
  );
};
