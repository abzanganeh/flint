import { useEffect, useRef, useState } from "react";

import {
  createSession,
  getSessionSnapshot,
  ingestStructuredContext,
  type SessionConfigDto,
  type SessionContextFields,
} from "../commands";
import { onSessionStateChange } from "../events";
import SmartResumeLinkImport from "../components/SmartResumeLinkImport";
import {
  buildCompanyOverviewText,
  loadPendingCompanyIntel,
  SMART_RESUME_SESSION_ID_KEY,
} from "../lib/smartResumeImport";
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

const MIN_REQUIRED_CHARS = 50;
const CHAR_WARN_THRESHOLD = 3_000;
const PROFILE_STORAGE_KEY = "flint.userProfile";

const PLACEHOLDERS: Record<keyof SessionContextFields, string> = {
  jobDescription: `Paste the full job posting here…

Example:
  Role: Senior Software Engineer
  Company: Acme Corp
  Requirements: 5+ years in distributed systems, Rust or Go, ownership mindset…`,
  profile: `Paste your resume, LinkedIn summary, or a quick bio here…

Saved locally and reused across sessions. Smart Resume integration auto-fills this field.`,
  companyOverview: `Mission, values, culture, recent news…

Example:
  Mission: Empower every person and organisation…
  Core Values: Bias for Action, Customer Obsession`,
  leadershipPrinciples: `Key leadership traits the company prizes…

Example: Think Big, Dive Deep, Deliver Results`,
  roleExpectations: `Deliverables or success criteria for this role…

Example: Own the backend platform; drive 0→1 on the new API gateway`,
  technicalPrep: `Topics or systems to brush up on…

Example: Distributed consensus, consistent hashing, Rust lifetimes`,
  strategyNotes: `Talking points, questions to ask, angles to emphasise…

Example: Lead with the Rust migration I owned at my last company`,
  speakingStyle: "",
  sessionVocabulary: `Domain terms Whisper should recognise (comma-separated)…

Example: RBAC, ABAC, OIDC, SCIM, entitlement review, Fisher`,
};

// ──────────────────────────────────────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────────────────────────────────────

export interface SessionPreFill {
  name: string;
  sessionType: string;
  domain: string;
  /** Legacy blob — used only as fallback into jobDescription. */
  contextText?: string;
  /** Source Smart Resume session id. */
  smartResumeSessionId?: string;
  /** Company intel for backward compat display. */
  companyIntel?: CompanyIntelDto;
  /** Structured fields take priority over contextText. */
  contextFields?: Partial<SessionContextFields>;
}

export interface SessionDesignProps {
  onComplete: (sessionId: string) => void;
  onViewSessions?: () => void;
  preFill?: SessionPreFill;
  onImportFromSmartResume?: (token: string) => void;
  importLoading?: boolean;
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

function emptyFields(): SessionContextFields {
  return {
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
}

function fieldsFromPreFill(
  preFill: SessionPreFill | undefined,
  pendingIntel: CompanyIntelDto | undefined,
): SessionContextFields {
  const base = emptyFields();
  if (!preFill) return base;

  if (preFill.contextFields) {
    Object.assign(base, preFill.contextFields);
  }

  // Legacy blob fallback: put it in jobDescription only when no structured JD present.
  if (!base.jobDescription && preFill.contextText) {
    base.jobDescription = preFill.contextText;
  }

  // Company intel → companyOverview if the field is still empty.
  const intel = preFill.companyIntel ?? pendingIntel;
  if (intel && !base.companyOverview) {
    base.companyOverview = buildCompanyOverviewText(intel);
  }

  return base;
}

// ──────────────────────────────────────────────────────────────────────────────
// FieldBlock — labelled textarea
// ──────────────────────────────────────────────────────────────────────────────

interface FieldBlockProps {
  id: keyof SessionContextFields;
  label: string;
  badge?: string;
  hint?: string;
  value: string;
  onChange: (v: string) => void;
  disabled: boolean;
  rows?: number;
  required?: boolean;
}

function FieldBlock({
  id,
  label,
  badge,
  hint,
  value,
  onChange,
  disabled,
  rows = 4,
  required = false,
}: FieldBlockProps) {
  const overThreshold = value.length > CHAR_WARN_THRESHOLD;

  return (
    <div className="sd-field">
      <div className="sd-context-label">
        <label htmlFor={`sd-${id}`}>
          {label}
          {required && <span className="sd-required" aria-label="required"> *</span>}
        </label>
        <span className="sd-field-meta">
          {badge && <span className="sd-badge">{badge}</span>}
          {hint && <span className="sd-hint">{hint}</span>}
          <span className={`sd-char-count${overThreshold ? " warning" : ""}`}>
            {value.length.toLocaleString()} chars
          </span>
        </span>
      </div>
      <textarea
        id={`sd-${id}`}
        className={`sd-textarea${required ? " sd-textarea--required" : ""}`}
        placeholder={PLACEHOLDERS[id]}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        rows={rows}
      />
    </div>
  );
}

// ──────────────────────────────────────────────────────────────────────────────
// SessionDesign
// ──────────────────────────────────────────────────────────────────────────────

export default function SessionDesign({
  onComplete,
  onViewSessions,
  preFill,
  onImportFromSmartResume,
  importLoading = false,
}: SessionDesignProps) {
  const pendingIntel = loadPendingCompanyIntel();

  const [name, setName] = useState(preFill?.name ?? "");
  const [sessionType, setSessionType] = useState(preFill?.sessionType ?? "interview");
  const [domain, setDomain] = useState(preFill?.domain ?? "software engineering");
  const [fields, setFields] = useState<SessionContextFields>(() =>
    fieldsFromPreFill(preFill, pendingIntel),
  );
  const [showRecommended, setShowRecommended] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Profile also persists locally so it survives new-session flows.
  useEffect(() => {
    setFields((prev) => {
      if (prev.profile) return prev;
      const stored = localStorage.getItem(PROFILE_STORAGE_KEY) ?? "";
      return stored ? { ...prev, profile: stored } : prev;
    });
  }, []);

  const sessionIdRef = useRef<string | null>(null);
  const onCompleteRef = useRef(onComplete);
  const preFillRef = useRef(preFill);
  useEffect(() => { preFillRef.current = preFill; }, [preFill]);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);

  // Apply preFill updates (Smart Resume import resolves asynchronously).
  useEffect(() => {
    if (!preFill) return;
    if (preFill.smartResumeSessionId) sessionIdRef.current = null;
    if (preFill.name) setName(preFill.name);
    if (preFill.sessionType) setSessionType(preFill.sessionType);
    if (preFill.domain) setDomain(preFill.domain);
    setFields(fieldsFromPreFill(preFill, pendingIntel));
    // Expand recommended section when import provides company overview.
    if (preFill.contextFields?.companyOverview || buildCompanyOverviewText(preFill.companyIntel)) {
      setShowRecommended(true);
    }
  }, [preFill]); // eslint-disable-line react-hooks/exhaustive-deps

  // Draft restore from SQLite (on mount).
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

        if (snapshot.contextFields) {
          // v6 session — restore all structured fields.
          setFields((prev) => ({ ...prev, ...snapshot.contextFields }));
          const hasRecommended =
            snapshot.contextFields.companyOverview ||
            snapshot.contextFields.leadershipPrinciples ||
            snapshot.contextFields.roleExpectations ||
            snapshot.contextFields.technicalPrep ||
            snapshot.contextFields.strategyNotes;
          if (hasRecommended) setShowRecommended(true);
        } else if (snapshot.contextText) {
          // Legacy session — put assembled blob in jobDescription.
          setFields((prev) => ({ ...prev, jobDescription: snapshot.contextText! }));
        }
      }
    })();
  }, []);

  // State-change events — transitions always come from Rust.
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
        if (sessionIdRef.current) onCompleteRef.current(sessionIdRef.current);
      }
      if (state === SessionState.CONFIGURING) {
        setIsLoading(false);
      }
    });
    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // ── Field helpers ─────────────────────────────────────────────────────────

  const setField = (key: keyof SessionContextFields) => (value: string) => {
    setFields((prev) => ({ ...prev, [key]: value }));
    if (key === "profile") {
      localStorage.setItem(PROFILE_STORAGE_KEY, value);
    }
  };

  // ── Smart Resume session id persistence ───────────────────────────────────

  useEffect(() => {
    if (preFill?.smartResumeSessionId) {
      localStorage.setItem(SMART_RESUME_SESSION_ID_KEY, preFill.smartResumeSessionId);
    }
  }, [preFill?.smartResumeSessionId]);

  // ── Submit ────────────────────────────────────────────────────────────────

  const handleExtract = async () => {
    if (!name.trim()) {
      setError("Please enter a session name.");
      return;
    }
    if (fields.jobDescription.trim().length < MIN_REQUIRED_CHARS) {
      setError(`Job description must be at least ${MIN_REQUIRED_CHARS} characters.`);
      return;
    }
    if (fields.profile.trim().length < MIN_REQUIRED_CHARS) {
      setError(`Profile must be at least ${MIN_REQUIRED_CHARS} characters.`);
      return;
    }

    setError(null);
    setIsLoading(true);

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
          await ingestStructuredContext(sid, fields);
          return;
        }
      }

      sid = await createSession(config);
      sessionIdRef.current = sid;
      await ingestStructuredContext(sid, fields);
    } catch (err: unknown) {
      setError(String(err));
      setIsLoading(false);
    }
  };

  const canExtract =
    !isLoading &&
    name.trim().length > 0 &&
    fields.jobDescription.trim().length >= MIN_REQUIRED_CHARS &&
    fields.profile.trim().length >= MIN_REQUIRED_CHARS;

  return (
    <div className="session-design">
      <div className="session-design-card">
        {/* Header */}
        <div className="sd-header">
          <h1>New Session</h1>
          <p>Fill in your context — Flint will build a digest to prepare your session.</p>
        </div>

        {onImportFromSmartResume && (
          <SmartResumeLinkImport
            disabled={importLoading || isLoading}
            onImport={onImportFromSmartResume}
          />
        )}

        {/* Session metadata */}
        <div className="sd-field-row">
          <div className="sd-field">
            <label htmlFor="sd-name">
              Session name <span className="sd-required" aria-label="required">*</span>
            </label>
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
                <option key={t.value} value={t.value}>{t.label}</option>
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

        {/* ── Required fields ── */}
        <div className="sd-section-label">Required</div>

        <FieldBlock
          id="jobDescription"
          label="Job description"
          badge="Required"
          value={fields.jobDescription}
          onChange={setField("jobDescription")}
          disabled={isLoading}
          rows={9}
          required
        />

        <FieldBlock
          id="profile"
          label="Your profile / resume"
          badge="Required"
          hint="Saved locally — reused across sessions"
          value={fields.profile}
          onChange={setField("profile")}
          disabled={isLoading}
          rows={5}
          required
        />

        {/* ── Recommended fields (toggle) ── */}
        <button
          type="button"
          className="sd-toggle-recommended"
          onClick={() => setShowRecommended((v) => !v)}
          disabled={isLoading}
        >
          {showRecommended ? "Hide" : "Add"} recommended context
          <span className="sd-toggle-recommended__hint">
            {" "}(company overview, leadership principles, role expectations, tech prep, strategy)
          </span>
        </button>

        {showRecommended && (
          <div className="sd-recommended-fields">
            <div className="sd-section-label">Recommended</div>

            <FieldBlock
              id="companyOverview"
              label="Company overview"
              hint="Mission, values, culture"
              value={fields.companyOverview}
              onChange={setField("companyOverview")}
              disabled={isLoading}
              rows={4}
            />

            <FieldBlock
              id="leadershipPrinciples"
              label="Leadership principles"
              hint="Values the company prizes"
              value={fields.leadershipPrinciples}
              onChange={setField("leadershipPrinciples")}
              disabled={isLoading}
              rows={3}
            />

            <FieldBlock
              id="roleExpectations"
              label="Role expectations"
              hint="Deliverables and success criteria"
              value={fields.roleExpectations}
              onChange={setField("roleExpectations")}
              disabled={isLoading}
              rows={3}
            />

            <FieldBlock
              id="technicalPrep"
              label="Technical preparation"
              hint="Topics to brush up on"
              value={fields.technicalPrep}
              onChange={setField("technicalPrep")}
              disabled={isLoading}
              rows={3}
            />

            <FieldBlock
              id="strategyNotes"
              label="Strategy notes"
              hint="Talking points and angles"
              value={fields.strategyNotes}
              onChange={setField("strategyNotes")}
              disabled={isLoading}
              rows={3}
            />

            <div className="sd-field">
              <div className="sd-context-label">
                <label htmlFor="sd-speakingStyle">Speaking style</label>
                <span className="sd-hint">How mock coach judges your delivery</span>
              </div>
              <select
                id="sd-speakingStyle"
                className="sd-input"
                value={fields.speakingStyle || "polished"}
                onChange={(e) => setField("speakingStyle")(e.target.value)}
                disabled={isLoading}
              >
                <option value="polished">Polished — executive, structured</option>
                <option value="natural">Natural — conversational, authentic</option>
              </select>
            </div>

            <FieldBlock
              id="sessionVocabulary"
              label="Session vocabulary"
              hint="Domain acronyms for speech recognition"
              value={fields.sessionVocabulary}
              onChange={setField("sessionVocabulary")}
              disabled={isLoading}
              rows={2}
            />
          </div>
        )}

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
