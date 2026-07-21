import { useCallback, useEffect, useRef, useState } from "react";

import {
  getFocusTagCatalog,
  getSessionFocus,
  inferRoundTypeFromBrief,
  ingestRoundDebrief,
  saveSessionFocus,
  type FocusTagCatalogEntry,
  type SessionFocusDto,
} from "../commands";

interface Props {
  sessionId: string;
  onComplete: () => void;
}

/** Mirrors ROUND_TYPES in src-tauri/src/session/round_questions.rs */
const ROUND_TYPES = [
  { value: "recruiter_screen", label: "Recruiter / HR screen" },
  { value: "technical", label: "Technical interview" },
  { value: "hiring_manager", label: "Hiring manager" },
  { value: "onsite_panel", label: "Onsite / panel loop" },
  { value: "final", label: "Final / executive round" },
] as const;

const ROUND_ORDER = ROUND_TYPES.map((t) => t.value);

function suggestNextRound(current: string): string {
  const idx = ROUND_ORDER.indexOf(current);
  if (idx >= 0 && idx < ROUND_ORDER.length - 1) {
    return ROUND_ORDER[idx + 1] ?? "";
  }
  return "";
}

const emptyFocus = (): SessionFocusDto => ({
  focusName: "",
  focusTags: [],
  recruiterBrief: "",
  focusNotes: "",
  focusConfirmedAt: null,
  needsFocusRefresh: false,
  roundType: "",
});

export default function SessionFocusGate({ sessionId, onComplete }: Props) {
  const [focus, setFocus] = useState<SessionFocusDto>(emptyFocus);
  const [debriefNotes, setDebriefNotes] = useState("");
  const [catalog, setCatalog] = useState<FocusTagCatalogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const roundTypeTouchedRef = useRef(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [focusData, tagCatalog] = await Promise.all([
        getSessionFocus(sessionId),
        getFocusTagCatalog(sessionId),
      ]);
      let loadedFocus = focusData;
      if (focusData.needsFocusRefresh && focusData.roundType) {
        const nextRound = suggestNextRound(focusData.roundType);
        if (nextRound) {
          loadedFocus = { ...focusData, roundType: nextRound };
        }
      }
      setFocus(loadedFocus);
      setCatalog(tagCatalog);
      setDebriefNotes("");
      roundTypeTouchedRef.current = false;
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggleTag = (tag: string) => {
    setFocus((prev) => {
      const selected = prev.focusTags.includes(tag)
        ? prev.focusTags.filter((t) => t !== tag)
        : [...prev.focusTags, tag];
      return { ...prev, focusTags: selected };
    });
  };

  const handleBriefBlur = async () => {
    if (roundTypeTouchedRef.current) return;
    const brief = focus.recruiterBrief.trim();
    if (!brief) return;
    try {
      const inferred = await inferRoundTypeFromBrief(brief);
      if (inferred && !roundTypeTouchedRef.current) {
        setFocus((prev) => ({ ...prev, roundType: inferred }));
      }
    } catch {
      // Non-fatal — suggestion only.
    }
  };

  const handleContinue = async () => {
    if (focus.focusTags.length === 0) {
      setError("Select at least one focus tag to filter rehearsal and mock questions.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const debrief = debriefNotes.trim();
      if (debrief.length > 0) {
        await ingestRoundDebrief(sessionId, focus.roundType, debrief);
      }
      await saveSessionFocus(sessionId, {
        ...focus,
        focusConfirmedAt: Math.floor(Date.now() / 1000),
        needsFocusRefresh: false,
      });
      onComplete();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const nextRoundPrep = focus.needsFocusRefresh;

  if (loading) {
    return (
      <main style={{ padding: 24, color: "#94a3b8" }} data-testid="session-focus-loading">
        Loading session focus…
      </main>
    );
  }

  return (
    <main
      style={{
        maxWidth: 640,
        margin: "0 auto",
        padding: "32px 24px",
        color: "#e2e8f0",
        fontFamily: "system-ui, sans-serif",
      }}
      data-testid="session-focus-gate"
    >
      <h1 style={{ fontSize: 22, marginBottom: 8 }}>
        {nextRoundPrep ? "Prepare for your next round" : "Session focus"}
      </h1>
      <p style={{ color: "#94a3b8", fontSize: 14, lineHeight: 1.6, marginBottom: 24 }}>
        {nextRoundPrep
          ? "Update the interview round, paste what you learned from the last call, and narrow rehearsal questions for the next conversation."
          : "Narrow rehearsal and mock questions to what this interview round covers. Live sessions always use the full bank — no surprises during the real call."}
      </p>

      {nextRoundPrep && (
        <div
          data-testid="session-focus-next-round-banner"
          style={{
            background: "#312e8122",
            border: "1px solid #4338ca",
            borderRadius: 6,
            padding: "10px 12px",
            color: "#c4b5fd",
            fontSize: 13,
            marginBottom: 16,
          }}
        >
          Your last live session finished. Confirm the next round type and optional debrief
          notes — debrief text is added to session RAG for rehearsal and live answers.
        </div>
      )}

      {error && (
        <div
          style={{
            background: "#7f1d1d22",
            border: "1px solid #7f1d1d",
            borderRadius: 6,
            padding: "10px 12px",
            color: "#fca5a5",
            fontSize: 13,
            marginBottom: 16,
          }}
        >
          {error}
        </div>
      )}

      <label style={{ display: "block", marginBottom: 16 }}>
        <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 6 }}>
          Focus name (optional)
        </span>
        <input
          type="text"
          value={focus.focusName}
          onChange={(e) => setFocus((p) => ({ ...p, focusName: e.target.value }))}
          placeholder="e.g. HR competency screen"
          style={inputStyle}
        />
      </label>

      <label style={{ display: "block", marginBottom: 16 }}>
        <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 6 }}>
          Interview round
        </span>
        <select
          data-testid="session-focus-round-type"
          value={focus.roundType}
          onChange={(e) => {
            roundTypeTouchedRef.current = true;
            setFocus((p) => ({ ...p, roundType: e.target.value }));
          }}
          style={inputStyle}
        >
          <option value="">Not specified</option>
          {ROUND_TYPES.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
        <span style={{ display: "block", fontSize: 11, color: "#52525b", marginTop: 6 }}>
          Suggested from the recruiter brief when you leave that field — you can always override.
        </span>
      </label>

      <label style={{ display: "block", marginBottom: 16 }}>
        <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 6 }}>
          Recruiter brief (paste email)
        </span>
        <textarea
          data-testid="session-focus-recruiter-brief"
          value={focus.recruiterBrief}
          onChange={(e) => setFocus((p) => ({ ...p, recruiterBrief: e.target.value }))}
          onBlur={() => void handleBriefBlur()}
          rows={4}
          placeholder="Paste the recruiter email or agenda…"
          style={{ ...inputStyle, resize: "vertical" }}
        />
      </label>

      <div style={{ marginBottom: 16 }}>
        <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 8 }}>
          Focus tags — select all that apply
        </span>
        <p style={{ color: "#64748b", fontSize: 12, marginBottom: 8, marginTop: 0 }}>
          Tags showing 0 have no matching bank questions yet — you can still select them.
        </p>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
          {catalog.map((tag) => {
            const selected = focus.focusTags.includes(tag.id);
            const isEmpty = tag.questionCount === 0;
            return (
              <button
                key={tag.id}
                type="button"
                data-testid={`focus-tag-chip-${tag.id}`}
                title={tag.description}
                onClick={() => toggleTag(tag.id)}
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: 6,
                  padding: "6px 12px",
                  borderRadius: 999,
                  border: selected ? "1px solid #7c3aed" : "1px solid #374151",
                  background: selected ? "#7c3aed33" : "transparent",
                  color: selected ? "#c4b5fd" : isEmpty ? "#52525b" : "#94a3b8",
                  fontSize: 12,
                  cursor: "pointer",
                  opacity: isEmpty ? 0.55 : 1,
                }}
              >
                {tag.label}
                <span
                  data-testid={`focus-tag-count-${tag.id}`}
                  style={{
                    fontSize: 10,
                    minWidth: 14,
                    textAlign: "center",
                    padding: "0 4px",
                    borderRadius: 8,
                    background: isEmpty ? "#27272a" : "#1e2028",
                    color: isEmpty ? "#71717a" : "#a78bfa",
                  }}
                >
                  {tag.questionCount}
                </span>
              </button>
            );
          })}
        </div>
      </div>

      <label style={{ display: "block", marginBottom: 16 }}>
        <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 6 }}>
          Notes (optional)
        </span>
        <textarea
          value={focus.focusNotes}
          onChange={(e) => setFocus((p) => ({ ...p, focusNotes: e.target.value }))}
          rows={2}
          style={{ ...inputStyle, resize: "vertical" }}
        />
      </label>

      {nextRoundPrep && (
        <label style={{ display: "block", marginBottom: 24 }}>
          <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 6 }}>
            Round debrief (optional)
          </span>
          <textarea
            data-testid="session-focus-debrief"
            value={debriefNotes}
            onChange={(e) => setDebriefNotes(e.target.value)}
            rows={4}
            placeholder="What happened in the last round? Topics asked, interviewer names, take-home details, areas to prep…"
            style={{ ...inputStyle, resize: "vertical" }}
          />
        </label>
      )}

      <button
        type="button"
        data-testid="session-focus-continue"
        onClick={() => void handleContinue()}
        disabled={saving || focus.focusTags.length === 0}
        style={{
          padding: "10px 20px",
          background: "#7c3aed",
          color: "#fff",
          border: "none",
          borderRadius: 6,
          fontSize: 14,
          fontWeight: 600,
          cursor: saving ? "wait" : "pointer",
          opacity: focus.focusTags.length === 0 ? 0.5 : 1,
        }}
      >
        {saving ? "Saving…" : "Continue to rehearsal"}
      </button>
    </main>
  );
}

const inputStyle: React.CSSProperties = {
  width: "100%",
  boxSizing: "border-box",
  padding: "10px 12px",
  background: "#111827",
  border: "1px solid #374151",
  borderRadius: 6,
  color: "#e2e8f0",
  fontSize: 14,
};
