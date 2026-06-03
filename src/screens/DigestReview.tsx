import { useEffect, useRef, useState } from "react";

import { confirmDigest, getDigest, type DigestDto } from "../commands";
import { onSessionStateChange } from "../events";
import { SessionState } from "../types";
import "./DigestReview.css";

// ──────────────────────────────────────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────────────────────────────────────

type PreWarmPhase = "idle" | "warming" | "done";

// ──────────────────────────────────────────────────────────────────────────────
// Sub-components
// ──────────────────────────────────────────────────────────────────────────────

function ArrayField({
  label,
  values,
  onChange,
  addLabel,
  fullWidth = false,
  disabled = false,
}: {
  label: string;
  values: string[];
  onChange: (next: string[]) => void;
  addLabel: string;
  fullWidth?: boolean;
  disabled?: boolean;
}) {
  const handleChange = (idx: number, val: string) => {
    const next = [...values];
    next[idx] = val;
    onChange(next);
  };

  const handleRemove = (idx: number) => {
    onChange(values.filter((_, i) => i !== idx));
  };

  const handleAdd = () => {
    onChange([...values, ""]);
  };

  return (
    <div className={`dr-array-field${fullWidth ? " full-width" : ""}`}>
      <span className="dr-field-label">{label}</span>
      <div className="dr-array-items">
        {values.map((v, idx) => (
          <div className="dr-array-item" key={idx}>
            <input
              type="text"
              value={v}
              onChange={(e) => handleChange(idx, e.target.value)}
              disabled={disabled}
              aria-label={`${label} item ${idx + 1}`}
            />
            <button
              className="dr-item-remove"
              onClick={() => handleRemove(idx)}
              disabled={disabled}
              aria-label={`Remove ${label} item ${idx + 1}`}
            >
              ×
            </button>
          </div>
        ))}
      </div>
      <button className="dr-add-item" onClick={handleAdd} disabled={disabled}>
        + {addLabel}
      </button>
    </div>
  );
}

function PreWarmProgress({
  questions,
  phase,
}: {
  questions: string[];
  phase: PreWarmPhase;
}) {
  if (phase === "idle") return null;

  return (
    <div className="dr-prewarm" role="status" aria-live="polite">
      <div className="dr-prewarm-title">
        {phase === "warming" && (
          <>
            <div className="dr-prewarm-spinner" aria-hidden="true" />
            Pre-warming responses…
          </>
        )}
        {phase === "done" && "Pre-warm complete"}
      </div>
      <ul className="dr-prewarm-questions">
        {questions.slice(0, 5).map((q, idx) => (
          <li className="dr-prewarm-question" key={idx}>
            <span className="dr-q-icon" aria-hidden="true">
              {phase === "done" ? (
                <span className="dr-q-check">✓</span>
              ) : (
                <div className="dr-q-spinner" />
              )}
            </span>
            <span>{q}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ──────────────────────────────────────────────────────────────────────────────
// Main component
// ──────────────────────────────────────────────────────────────────────────────

export interface DigestReviewProps {
  sessionId: string;
  onComplete: () => void;
  onStartOver: () => void;
}

export default function DigestReview({
  sessionId,
  onComplete,
  onStartOver,
}: DigestReviewProps) {
  const [digest, setDigest] = useState<DigestDto | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isConfirming, setIsConfirming] = useState(false);
  const [preWarmPhase, setPreWarmPhase] = useState<PreWarmPhase>("idle");
  const [error, setError] = useState<string | null>(null);

  const onCompleteRef = useRef(onComplete);
  useEffect(() => {
    onCompleteRef.current = onComplete;
  }, [onComplete]);

  // ── Fetch digest on mount ─────────────────────────────────────────────────
  useEffect(() => {
    setIsLoading(true);
    getDigest(sessionId)
      .then((d) => {
        setDigest(d);
        setIsLoading(false);
      })
      .catch((err: unknown) => {
        setError(String(err));
        setIsLoading(false);
      });
  }, [sessionId]);

  // ── Listen to state-change events ────────────────────────────────────────
  useEffect(() => {
    let active = true;

    const unlistenPromise = onSessionStateChange(({ state }) => {
      if (!active) return;

      if (state === SessionState.PRE_WARMING) {
        setPreWarmPhase("warming");
        setIsConfirming(true);
      }

      if (state === SessionState.REHEARSING) {
        setPreWarmPhase("done");
        // Brief pause so the user sees the "done" state before navigation.
        setTimeout(() => {
          if (active) onCompleteRef.current();
        }, 800);
      }
    });

    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // ── Confirm handler ───────────────────────────────────────────────────────
  const handleConfirm = () => {
    if (!digest) return;
    setError(null);

    // Fire confirm_digest — navigation is driven by the REHEARSING event.
    confirmDigest(sessionId, digest).catch((err: unknown) => {
      setIsConfirming(false);
      setPreWarmPhase("idle");
      setError(String(err));
    });
  };

  // ── Digest field helpers ──────────────────────────────────────────────────
  const updateField = <K extends keyof DigestDto>(key: K, value: DigestDto[K]) => {
    setDigest((prev) => (prev ? { ...prev, [key]: value } : prev));
  };

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <div className="digest-review">
      <div className="digest-review-card">
        {/* Header */}
        <div className="dr-header">
          <div>
            <h1>Digest Review</h1>
            <p>Edit any field before confirming. Flint will pre-warm the top 5 questions.</p>
          </div>
          <button
            className="dr-btn-ghost"
            onClick={onStartOver}
            disabled={isConfirming}
          >
            Start Over
          </button>
        </div>

        {/* Loading */}
        {isLoading && (
          <div className="dr-loading">
            <div className="dr-spinner" aria-hidden="true" />
            <span>Loading digest…</span>
          </div>
        )}

        {/* Digest fields */}
        {!isLoading && digest && (
          <>
            <div className="dr-fields-grid">
              {/* Role */}
              <div className="dr-field">
                <label className="dr-field-label" htmlFor="dr-role">
                  Role
                </label>
                <input
                  id="dr-role"
                  className="dr-field-input"
                  type="text"
                  value={digest.role}
                  onChange={(e) => updateField("role", e.target.value)}
                  disabled={isConfirming}
                />
              </div>

              {/* Company */}
              <div className="dr-field">
                <label className="dr-field-label" htmlFor="dr-company">
                  Company
                </label>
                <input
                  id="dr-company"
                  className="dr-field-input"
                  type="text"
                  value={digest.company}
                  onChange={(e) => updateField("company", e.target.value)}
                  disabled={isConfirming}
                />
              </div>

              {/* Domain */}
              <div className="dr-field">
                <label className="dr-field-label" htmlFor="dr-domain">
                  Domain
                </label>
                <input
                  id="dr-domain"
                  className="dr-field-input"
                  type="text"
                  value={digest.domain}
                  onChange={(e) => updateField("domain", e.target.value)}
                  disabled={isConfirming}
                />
              </div>

              {/* Seniority */}
              <div className="dr-field">
                <label className="dr-field-label" htmlFor="dr-seniority">
                  Seniority
                </label>
                <input
                  id="dr-seniority"
                  className="dr-field-input"
                  type="text"
                  value={digest.seniority}
                  onChange={(e) => updateField("seniority", e.target.value)}
                  disabled={isConfirming}
                />
              </div>

              {/* Key skills */}
              <ArrayField
                label="Key Skills"
                values={digest.keySkills}
                onChange={(v) => updateField("keySkills", v)}
                addLabel="Add skill"
                fullWidth
                disabled={isConfirming}
              />

              {/* Likely questions */}
              <ArrayField
                label="Likely Questions"
                values={digest.likelyQuestions}
                onChange={(v) => updateField("likelyQuestions", v)}
                addLabel="Add question"
                fullWidth
                disabled={isConfirming}
              />

              {/* Topics to avoid */}
              <ArrayField
                label="Topics to Avoid"
                values={digest.topicsToAvoid}
                onChange={(v) => updateField("topicsToAvoid", v)}
                addLabel="Add topic"
                fullWidth
                disabled={isConfirming}
              />
            </div>

            {/* Pre-warm progress */}
            <PreWarmProgress
              questions={digest.likelyQuestions}
              phase={preWarmPhase}
            />
          </>
        )}

        {/* Error */}
        {error && (
          <div className="dr-error" role="alert">
            {error}
          </div>
        )}

        {/* Actions */}
        {!isLoading && digest && (
          <div className="dr-actions">
            <button
              className="dr-btn-primary"
              onClick={handleConfirm}
              disabled={isConfirming}
            >
              {isConfirming ? "Pre-warming…" : "Confirm and Pre-warm"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
