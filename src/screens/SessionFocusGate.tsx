import { useCallback, useEffect, useState } from "react";

import {
  getSessionFocus,
  listQuestionBankTags,
  saveSessionFocus,
  type SessionFocusDto,
} from "../commands";

interface Props {
  sessionId: string;
  onComplete: () => void;
}

const emptyFocus = (): SessionFocusDto => ({
  focusName: "",
  focusTags: [],
  recruiterBrief: "",
  focusNotes: "",
  focusConfirmedAt: null,
  needsFocusRefresh: false,
});

export default function SessionFocusGate({ sessionId, onComplete }: Props) {
  const [focus, setFocus] = useState<SessionFocusDto>(emptyFocus);
  const [availableTags, setAvailableTags] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [focusData, tags] = await Promise.all([
        getSessionFocus(sessionId),
        listQuestionBankTags(sessionId),
      ]);
      setFocus(focusData);
      setAvailableTags(tags);
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

  const handleContinue = async () => {
    if (focus.focusTags.length === 0) {
      setError("Select at least one focus tag to filter rehearsal and mock questions.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
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
      <h1 style={{ fontSize: 22, marginBottom: 8 }}>Session focus</h1>
      <p style={{ color: "#94a3b8", fontSize: 14, lineHeight: 1.6, marginBottom: 24 }}>
        Narrow rehearsal and mock questions to what this interview round covers. Live sessions
        always use the full bank — no surprises during the real call.
      </p>

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
          Recruiter brief (paste email)
        </span>
        <textarea
          value={focus.recruiterBrief}
          onChange={(e) => setFocus((p) => ({ ...p, recruiterBrief: e.target.value }))}
          rows={4}
          placeholder="Paste the recruiter email or agenda…"
          style={{ ...inputStyle, resize: "vertical" }}
        />
      </label>

      <div style={{ marginBottom: 16 }}>
        <span style={{ display: "block", fontSize: 12, color: "#64748b", marginBottom: 8 }}>
          Focus tags — select all that apply
        </span>
        {availableTags.length === 0 ? (
          <p style={{ color: "#64748b", fontSize: 13 }}>
            No tags inferred yet. Confirm digest first or add questions to the bank.
          </p>
        ) : (
          <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
            {availableTags.map((tag) => {
              const selected = focus.focusTags.includes(tag);
              return (
                <button
                  key={tag}
                  type="button"
                  onClick={() => toggleTag(tag)}
                  style={{
                    padding: "6px 12px",
                    borderRadius: 999,
                    border: selected ? "1px solid #7c3aed" : "1px solid #374151",
                    background: selected ? "#7c3aed33" : "transparent",
                    color: selected ? "#c4b5fd" : "#94a3b8",
                    fontSize: 12,
                    cursor: "pointer",
                  }}
                >
                  {tag}
                </button>
              );
            })}
          </div>
        )}
      </div>

      <label style={{ display: "block", marginBottom: 24 }}>
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
