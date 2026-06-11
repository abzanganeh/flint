import { useState } from "react";

import { appendResearchToContext } from "../commands";

interface AddContextPanelProps {
  sessionId: string;
  question: string;
  /** Called after context is saved. Second arg true when user chose Save & Re-ask. */
  onSaved: (chunksAdded: number, reask: boolean) => void;
}

type SaveState = "idle" | "saving" | "saved" | "error";

export default function AddContextPanel({
  sessionId,
  question,
  onSaved,
}: AddContextPanelProps) {
  const [expanded, setExpanded] = useState(false);
  const [text, setText] = useState("");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [chunksAdded, setChunksAdded] = useState(0);

  const handleSave = async (reask: boolean) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    setSaveState("saving");
    setErrorMsg(null);
    try {
      const result = await appendResearchToContext(sessionId, question, trimmed, []);
      setChunksAdded(result.chunksAdded);
      setSaveState("saved");
      onSaved(result.chunksAdded, reask);
      if (reask) {
        // Parent will re-ask; reset panel so it can be re-used.
        setTimeout(() => {
          setText("");
          setSaveState("idle");
          setExpanded(false);
        }, 800);
      }
    } catch (e) {
      setSaveState("error");
      setErrorMsg(String(e));
    }
  };

  return (
    <div
      style={{
        marginTop: 10,
        border: "1px solid #3b2f1e",
        borderRadius: 6,
        backgroundColor: "#1a1510",
        fontSize: "12px",
      }}
    >
      {/* Warning header — always visible */}
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          width: "100%",
          padding: "8px 12px",
          background: "none",
          border: "none",
          cursor: "pointer",
          textAlign: "left",
          color: "#f59e0b",
          fontWeight: 600,
          fontSize: "12px",
        }}
      >
        <span>⚠</span>
        <span style={{ flex: 1 }}>
          No personal story found for this question — Flint used general knowledge.
          Add your own experience below.
        </span>
        <span style={{ color: "#6b7280", fontSize: "10px" }}>
          {expanded ? "▲ Hide" : "▼ Add your story"}
        </span>
      </button>

      {expanded && (
        <div style={{ padding: "0 12px 12px" }}>
          {saveState === "saved" ? (
            <p style={{ color: "#22c55e", margin: "0 0 8px" }}>
              Saved — {chunksAdded} chunk{chunksAdded !== 1 ? "s" : ""} added to
              your context.
            </p>
          ) : (
            <>
              <p style={{ color: "#9ca3af", margin: "0 0 8px", lineHeight: 1.5 }}>
                Paste your story, project details, or talking points for this
                question. Flint will embed and index it immediately — no need to
                leave Rehearsal.
              </p>
              <textarea
                value={text}
                onChange={(e) => setText(e.target.value)}
                placeholder={`e.g. "On Project X, I faced [problem]… I solved it by…"`}
                rows={6}
                style={{
                  width: "100%",
                  padding: "8px 10px",
                  backgroundColor: "#0f1117",
                  border: "1px solid #374151",
                  borderRadius: 4,
                  color: "#e5e7eb",
                  fontSize: "12px",
                  fontFamily: "inherit",
                  lineHeight: 1.5,
                  resize: "vertical",
                  outline: "none",
                  boxSizing: "border-box",
                }}
                autoFocus
              />
              {errorMsg && (
                <p style={{ color: "#ef4444", margin: "4px 0 0", fontSize: "11px" }}>
                  {errorMsg}
                </p>
              )}
              <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
                <button
                  type="button"
                  disabled={!text.trim() || saveState === "saving"}
                  onClick={() => void handleSave(true)}
                  style={{
                    padding: "5px 12px",
                    backgroundColor:
                      text.trim() && saveState !== "saving" ? "#7c3aed" : "#1e2028",
                    color:
                      text.trim() && saveState !== "saving" ? "#fff" : "#4b5563",
                    border: "none",
                    borderRadius: 4,
                    fontSize: "11px",
                    fontWeight: 600,
                    cursor:
                      text.trim() && saveState !== "saving"
                        ? "pointer"
                        : "not-allowed",
                  }}
                >
                  {saveState === "saving" ? "Saving…" : "Save & Re-ask"}
                </button>
                <button
                  type="button"
                  disabled={!text.trim() || saveState === "saving"}
                  onClick={() => void handleSave(false)}
                  style={{
                    padding: "5px 12px",
                    backgroundColor: "transparent",
                    color:
                      text.trim() && saveState !== "saving" ? "#9ca3af" : "#4b5563",
                    border: "1px solid #374151",
                    borderRadius: 4,
                    fontSize: "11px",
                    fontWeight: 600,
                    cursor:
                      text.trim() && saveState !== "saving"
                        ? "pointer"
                        : "not-allowed",
                  }}
                >
                  Save only
                </button>
                <button
                  type="button"
                  onClick={() => setExpanded(false)}
                  style={{
                    padding: "5px 10px",
                    backgroundColor: "transparent",
                    color: "#52525b",
                    border: "none",
                    borderRadius: 4,
                    fontSize: "11px",
                    cursor: "pointer",
                  }}
                >
                  Dismiss
                </button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
