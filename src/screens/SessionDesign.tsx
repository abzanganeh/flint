import { useEffect, useRef, useState } from "react";

import {
  createSession,
  getSessionSnapshot,
  ingestContext,
  type SessionConfigDto,
} from "../commands";
import { onSessionStateChange } from "../events";
import SmartResumeLinkImport from "../components/SmartResumeLinkImport";
import { buildContextText, hasCompanyIntel, loadPendingCompanyIntel, SMART_RESUME_SESSION_ID_KEY } from "../lib/smartResumeImport";
import type { CompanyIntelDto } from "../commands";
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

const PROFILE_PLACEHOLDER = `Paste your resume, LinkedIn summary, or a quick bio here…

This is stored locally and re-used across sessions. Smart Resume integration will auto-fill this field.`;

const MIN_CONTEXT_CHARS = 50;
const CHAR_WARN_THRESHOLD = 3_000;
const PROFILE_STORAGE_KEY = "flint.userProfile";

// ──────────────────────────────────────────────────────────────────────────────
// Component
// ──────────────────────────────────────────────────────────────────────────────

export interface SessionPreFill {
  name: string;
  sessionType: string;
  domain: string;
  /** Reconstructed from the session's digest — pre-fills the context textarea. */
  contextText?: string;
  /** Tailored resume summary from Smart Resume handoff. */
  profileText?: string;
  /** Source Smart Resume session id (Phase 3 digest sync). */
  smartResumeSessionId?: string;
  /** Company culture block from Smart Resume (shown separately from JD). */
  companyIntel?: CompanyIntelDto;
}

export interface SessionDesignProps {
  /** Called with the new session UUID once the digest is ready. */
  onComplete: (sessionId: string) => void;
  /** Navigate to the past sessions list. */
  onViewSessions?: () => void;
  /** Pre-populate form fields (e.g. when cloning a past session). */
  preFill?: SessionPreFill;
  /** Paste-import handoff when deep links do not launch Flint (common in dev). */
  onImportFromSmartResume?: (token: string) => void;
  importLoading?: boolean;
}

export default function SessionDesign({
  onComplete,
  onViewSessions,
  preFill,
  onImportFromSmartResume,
  importLoading = false,
}: SessionDesignProps) {
  const [name, setName] = useState(preFill?.name ?? "");
  const [sessionType, setSessionType] = useState(preFill?.sessionType ?? "interview");
  const [domain, setDomain] = useState(preFill?.domain ?? "software engineering");
  const [contextText, setContextText] = useState(preFill?.contextText ?? "");
  const [companyIntel, setCompanyIntel] = useState<CompanyIntelDto | undefined>(
    () => preFill?.companyIntel ?? loadPendingCompanyIntel(),
  );
  const [profileText, setProfileText] = useState<string>(
    () => preFill?.profileText ?? localStorage.getItem(PROFILE_STORAGE_KEY) ?? "",
  );
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Keep sessionId in a ref so the event callback always sees the latest value.
  const sessionIdRef = useRef<string | null>(null);
  // Stable ref to onComplete so we don't need it in the listener effect deps.
  const onCompleteRef = useRef(onComplete);
  const preFillRef = useRef(preFill);
  useEffect(() => {
    preFillRef.current = preFill;
  }, [preFill]);
  useEffect(() => {
    onCompleteRef.current = onComplete;
  }, [onComplete]);

  // Smart Resume import owns the form — do not let SQLite draft restore overwrite it.
  useEffect(() => {
    if (!preFill) return;
    if (preFill.smartResumeSessionId) {
      sessionIdRef.current = null;
    }
    if (preFill.name) setName(preFill.name);
    if (preFill.sessionType) setSessionType(preFill.sessionType);
    if (preFill.domain) setDomain(preFill.domain);
    if (preFill.contextText !== undefined) setContextText(preFill.contextText);
    if (preFill.profileText !== undefined) setProfileText(preFill.profileText);
    const intel = preFill.companyIntel ?? loadPendingCompanyIntel();
    if (intel) setCompanyIntel(intel);
  }, [preFill]);

  // Re-anchor session id and form fields when resuming a draft from SQLite.
  useEffect(() => {
    void (async () => {
      const snapshot = await getSessionSnapshot();
      if (preFillRef.current?.smartResumeSessionId) return;
      if (!snapshot.sessionId) return;
      sessionIdRef.current = snapshot.sessionId;
      if (snapshot.state === SessionState.CONFIGURING) {
        if (snapshot.name) setName(snapshot.name);
        if (snapshot.sessionType) setSessionType(snapshot.sessionType);
        if (snapshot.domain) setDomain(snapshot.domain);
        if (snapshot.contextText) setContextText(snapshot.contextText);
      }
    })();
  }, []);

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
    const trimmedProfile = profileText.trim();
    const contextWithIntel = buildContextText(trimmedContext, companyIntel);

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
    setIsLoading(true);

    // Combine session context and user profile into a single text block that
    // the Rust digest extractor and RAG pipeline will embed together.
    const parts: string[] = [`[SESSION CONTEXT]\n${contextWithIntel}`];
    if (trimmedProfile.length > 0) {
      parts.push(`[YOUR PROFILE]\n${trimmedProfile}`);
    }
    const combinedText = parts.join("\n\n");

    try {
      const config: SessionConfigDto = {
        name: name.trim(),
        sessionType,
        domain: domain.trim() || "general",
      };

      let sid = sessionIdRef.current;
      if (sid) {
        const snapshot = await getSessionSnapshot();
        if (
          snapshot.state === SessionState.CONFIGURING &&
          snapshot.sessionId === sid
        ) {
          await ingestContext(sid, combinedText);
          return;
        }
      }

      // create_session transitions Idle → Configuring and returns the UUID.
      sid = await createSession(config);
      sessionIdRef.current = sid;
      await ingestContext(sid, combinedText);
    } catch (err: unknown) {
      setError(String(err));
      setIsLoading(false);
    }
  };

  const handleProfileChange = (value: string) => {
    setProfileText(value);
    localStorage.setItem(PROFILE_STORAGE_KEY, value);
  };

  useEffect(() => {
    if (preFill?.smartResumeSessionId) {
      localStorage.setItem(SMART_RESUME_SESSION_ID_KEY, preFill.smartResumeSessionId);
    }
  }, [preFill?.smartResumeSessionId]);

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

        {onImportFromSmartResume && (
          <SmartResumeLinkImport
            disabled={importLoading || isLoading}
            onImport={onImportFromSmartResume}
          />
        )}

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

        {/* User profile / resume */}
        <div className="sd-field">
          <div className="sd-context-label">
            <label htmlFor="sd-profile">Your profile / resume</label>
            <span className="sd-char-count sd-hint">Saved locally — reused across sessions</span>
          </div>
          <textarea
            id="sd-profile"
            className="sd-textarea sd-textarea--profile"
            placeholder={PROFILE_PLACEHOLDER}
            value={profileText}
            onChange={(e) => handleProfileChange(e.target.value)}
            disabled={isLoading}
            rows={5}
          />
        </div>

        {/* Session context (JD / brief) */}
        {companyIntel && hasCompanyIntel(companyIntel) && (
          <div className="sd-company-intel" aria-label="Company culture from Smart Resume">
            <div className="sd-context-label">
              <label>Company culture</label>
              <span className="sd-char-count sd-hint">From Smart Resume</span>
            </div>
            {companyIntel.mission && (
              <p><strong>Mission:</strong> {companyIntel.mission}</p>
            )}
            {companyIntel.values.length > 0 && (
              <p><strong>Values:</strong> {companyIntel.values.join(", ")}</p>
            )}
            {companyIntel.cultureNotes && (
              <p><strong>Culture:</strong> {companyIntel.cultureNotes}</p>
            )}
          </div>
        )}

        <div className="sd-field">
          <div className="sd-context-label">
            <label htmlFor="sd-context">Session context</label>
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
            rows={9}
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
          {onViewSessions && (
            <button
              type="button"
              className="sd-btn-secondary"
              onClick={onViewSessions}
              disabled={isLoading}
            >
              Past Sessions
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
