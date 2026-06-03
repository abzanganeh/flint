import { useEffect, useRef, useState } from "react";

import {
  createSession,
  ingestContext,
  type SessionConfigDto,
} from "../commands";
import { onSessionStateChange } from "../events";
import { SessionState } from "../types";
import "./SessionDesign.css";

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

const SESSION_TYPES = [
  { value: "interview", label: "Interview" },
  { value: "meeting", label: "Meeting" },
  { value: "presentation", label: "Presentation" },
  { value: "negotiation", label: "Negotiation" },
  { value: "other", label: "Other" },
];

const CONTEXT_PLACEHOLDER = `Paste your job description, meeting brief, or notes here…

Example — interview:
  Role: Senior Software Engineer
  Company: Acme Corp
  Requirements: 5+ years in distributed systems, Rust or Go, ownership mindset…
  About the team: …`;

const MIN_CONTEXT_CHARS = 50;
const CHAR_WARN_THRESHOLD = 3_000;

// ──────────────────────────────────────────────────────────────────────────────
// Component
// ──────────────────────────────────────────────────────────────────────────────

export interface SessionDesignProps {
  /** Called with the new session UUID once the digest is ready. */
  onComplete: (sessionId: string) => void;
}

export default function SessionDesign({ onComplete }: SessionDesignProps) {
  const [name, setName] = useState("");
  const [sessionType, setSessionType] = useState("interview");
  const [domain, setDomain] = useState("software engineering");
  const [contextText, setContextText] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Keep sessionId in a ref so the event callback always sees the latest value.
  const sessionIdRef = useRef<string | null>(null);
  // Stable ref to onComplete so we don't need it in the listener effect deps.
  const onCompleteRef = useRef(onComplete);
  useEffect(() => {
    onCompleteRef.current = onComplete;
  }, [onComplete]);

  // ── Listen to state-change events (state changes NEVER come from cmd results)
  useEffect(() => {
    let active = true;

    const unlistenPromise = onSessionStateChange(({ state }) => {
      if (!active) return;

      if (state === SessionState.INGESTING) {
        setIsLoading(true);
        setError(null);
      }

      if (state === SessionState.DIGEST_REVIEW) {
        setIsLoading(false);
        if (sessionIdRef.current) {
          onCompleteRef.current(sessionIdRef.current);
        }
      }

      // Reverted to CONFIGURING (e.g. empty context)
      if (state === SessionState.CONFIGURING) {
        setIsLoading(false);
      }
    });

    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // ── Extract digest handler ────────────────────────────────────────────────
  const handleExtract = async () => {
    const trimmedContext = contextText.trim();
    if (!name.trim()) {
      setError("Please enter a session name.");
      return;
    }
    if (trimmedContext.length < MIN_CONTEXT_CHARS) {
      setError(
        `Please paste at least ${MIN_CONTEXT_CHARS} characters of context.`,
      );
      return;
    }

    setError(null);

    try {
      const config: SessionConfigDto = {
        name: name.trim(),
        sessionType,
        domain: domain.trim() || "general",
      };

      // create_session transitions Idle → Configuring and returns the UUID.
      const sid = await createSession(config);
      sessionIdRef.current = sid;

      // Fire ingest_context — do not await for navigation; the DIGEST_REVIEW
      // event drives that (task rule: all state changes come from events).
      ingestContext(sid, trimmedContext).catch((err: unknown) => {
        setIsLoading(false);
        setError(String(err));
      });
    } catch (err: unknown) {
      setError(String(err));
      setIsLoading(false);
    }
  };

  const charCount = contextText.length;
  const canExtract =
    !isLoading && name.trim().length > 0 && contextText.trim().length >= MIN_CONTEXT_CHARS;

  return (
    <div className="session-design">
      <div className="session-design-card">
        {/* Header */}
        <div className="sd-header">
          <h1>New Session</h1>
          <p>Paste your context and Flint will extract a digest to prepare for your session.</p>
        </div>

        {/* Config row */}
        <div className="sd-field-row">
          <div className="sd-field">
            <label htmlFor="sd-name">Session name</label>
            <input
              id="sd-name"
              type="text"
              placeholder="e.g. Acme Corp — SWE Interview"
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={isLoading}
            />
          </div>

          <div className="sd-field">
            <label htmlFor="sd-type">Session type</label>
            <select
              id="sd-type"
              value={sessionType}
              onChange={(e) => setSessionType(e.target.value)}
              disabled={isLoading}
            >
              {SESSION_TYPES.map((t) => (
                <option key={t.value} value={t.value}>
                  {t.label}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="sd-field">
          <label htmlFor="sd-domain">Domain</label>
          <input
            id="sd-domain"
            type="text"
            placeholder="e.g. software engineering, product management…"
            value={domain}
            onChange={(e) => setDomain(e.target.value)}
            disabled={isLoading}
          />
        </div>

        {/* Context textarea */}
        <div className="sd-field">
          <div className="sd-context-label">
            <label htmlFor="sd-context">Context</label>
            <span
              className={`sd-char-count${charCount > CHAR_WARN_THRESHOLD ? " warning" : ""}`}
            >
              {charCount.toLocaleString()} chars
            </span>
          </div>
          <textarea
            id="sd-context"
            className="sd-textarea"
            placeholder={CONTEXT_PLACEHOLDER}
            value={contextText}
            onChange={(e) => setContextText(e.target.value)}
            disabled={isLoading}
            rows={10}
          />
        </div>

        {/* Status */}
        {isLoading && (
          <div className="sd-loading" role="status" aria-live="polite">
            <div className="sd-spinner" aria-hidden="true" />
            <span>Analysing your context…</span>
          </div>
        )}

        {error && (
          <div className="sd-error" role="alert">
            {error}
          </div>
        )}

        {/* Actions */}
        <div className="sd-actions">
          <button
            className="sd-btn-primary"
            onClick={() => void handleExtract()}
            disabled={!canExtract}
          >
            Extract Digest
          </button>
        </div>
      </div>
    </div>
  );
}
