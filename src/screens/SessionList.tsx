import React, { useCallback, useEffect, useState } from "react";
import type { SessionSummaryDto } from "../commands";
import { deleteSession, listSessions, promoteSession } from "../commands";

interface Props {
  onBack: () => void;
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

export const SessionList: React.FC<Props> = ({ onBack }) => {
  const [sessions, setSessions] = useState<SessionSummaryDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionInFlight, setActionInFlight] = useState<string | null>(null);

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

  useEffect(() => {
    load();
  }, [load]);

  const handlePromote = useCallback(
    async (id: string) => {
      setActionInFlight(id + ":promote");
      try {
        await promoteSession(id);
        await load();
      } catch (e) {
        setError(String(e));
      } finally {
        setActionInFlight(null);
      }
    },
    [load],
  );

  const handleDelete = useCallback(
    async (id: string) => {
      setActionInFlight(id + ":delete");
      try {
        await deleteSession(id);
        setSessions((prev) => prev.filter((s) => s.id !== id));
      } catch (e) {
        setError(String(e));
      } finally {
        setActionInFlight(null);
      }
    },
    [],
  );

  return (
    <div className="flex flex-col min-h-screen bg-neutral-950 text-white">
      {/* Header */}
      <header className="border-b border-neutral-800 px-6 py-4 flex items-center gap-4">
        <button
          onClick={onBack}
          className="text-neutral-400 hover:text-white text-sm transition-colors"
        >
          ← Back
        </button>
        <h1 className="text-base font-semibold text-white">Past Sessions</h1>
        <div className="ml-auto">
          <button
            onClick={load}
            disabled={loading}
            className="text-xs text-neutral-400 hover:text-white transition-colors disabled:opacity-40"
          >
            Refresh
          </button>
        </div>
      </header>

      {/* Body */}
      <main className="flex-1 px-6 py-6">
        {error && (
          <p className="text-red-400 text-sm mb-4 bg-red-900/20 rounded-lg px-3 py-2">
            {error}
          </p>
        )}

        {loading && (
          <div className="flex items-center justify-center py-16">
            <p className="text-neutral-400 text-sm">Loading sessions…</p>
          </div>
        )}

        {!loading && sessions.length === 0 && (
          <div className="flex flex-col items-center justify-center py-16 gap-2">
            <p className="text-neutral-400 text-sm">No past sessions.</p>
            <p className="text-neutral-600 text-xs">
              Completed sessions will appear here.
            </p>
          </div>
        )}

        <ul className="space-y-3">
          {sessions.map((session) => {
            const expiryLabel = formatExpiry(
              session.expiresInSecs,
              session.promoted,
            );
            const expired = session.expiresInSecs <= 0 && !session.promoted;
            const date = new Date(session.createdAt * 1000);
            const dateLabel = date.toLocaleDateString(undefined, {
              month: "short",
              day: "numeric",
              year: "numeric",
            });

            return (
              <li
                key={session.id}
                className="bg-neutral-900 border border-neutral-800 rounded-xl px-5 py-4 flex items-center gap-4"
              >
                {/* Status dot */}
                <div
                  className={`w-2 h-2 rounded-full flex-shrink-0 ${
                    session.promoted
                      ? "bg-blue-400"
                      : expired
                        ? "bg-neutral-600"
                        : "bg-green-400"
                  }`}
                />

                {/* Info */}
                <div className="flex-1 min-w-0">
                  <p className="text-sm font-medium text-white truncate">
                    {session.name || dateLabel} &mdash; {session.domain || stateLabel(session.state)}
                  </p>
                  <p className="text-xs text-neutral-500 mt-0.5 truncate">
                    {session.name ? dateLabel : ""}{session.name && session.sessionType ? ` \u00B7 ${session.sessionType}` : ""}
                  </p>
                  <p
                    className={`text-xs mt-0.5 ${
                      expired ? "text-red-400" : session.promoted ? "text-blue-400" : "text-neutral-400"
                    }`}
                  >
                    {expiryLabel}
                  </p>
                </div>

                {/* Actions */}
                <div className="flex items-center gap-2 flex-shrink-0">
                  {!session.promoted && (
                    <button
                      onClick={() => handlePromote(session.id)}
                      disabled={actionInFlight !== null}
                      className="text-xs text-blue-400 hover:text-blue-300 disabled:opacity-40 transition-colors"
                    >
                      {actionInFlight === session.id + ":promote"
                        ? "Pinning…"
                        : "Pin"}
                    </button>
                  )}
                  <button
                    onClick={() => handleDelete(session.id)}
                    disabled={actionInFlight !== null}
                    className="text-xs text-red-400 hover:text-red-300 disabled:opacity-40 transition-colors"
                  >
                    {actionInFlight === session.id + ":delete"
                      ? "Deleting…"
                      : "Delete"}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>

        {/* Data retention note */}
        {sessions.length > 0 && (
          <p className="text-neutral-600 text-xs mt-6 text-center">
            Sessions are kept for 30 days. Pin a session to exempt it from
            automatic deletion.
          </p>
        )}
      </main>
    </div>
  );
};
