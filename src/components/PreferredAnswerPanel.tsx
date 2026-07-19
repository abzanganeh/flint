import { useCallback, useEffect, useState } from "react";

import { getPreferredAnswer, savePreferredAnswer } from "../commands";
import InfoPopover from "./InfoPopover";

interface PreferredAnswerPanelProps {
  sessionId: string;
  question: string;
  /** Flint's latest directional draft — pre-fill when no saved answer yet. */
  suggestedAnswer?: string;
  onSaved?: () => void;
  /** When true, body starts collapsed to save vertical space (user can expand). */
  defaultCollapsed?: boolean;
}

type SaveState = "idle" | "saving" | "saved" | "error";

/**
 * Lets the user tailor and save a preferred answer that Flint serves instantly in Live.
 */
export default function PreferredAnswerPanel({
  sessionId,
  question,
  suggestedAnswer = "",
  onSaved,
  defaultCollapsed = false,
}: PreferredAnswerPanelProps) {
  const [draft, setDraft] = useState("");
  const [loadedPreferred, setLoadedPreferred] = useState("");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(!defaultCollapsed);

  const loadPreferred = useCallback(async () => {
    if (!question.trim()) return;
    try {
      const saved = await getPreferredAnswer(sessionId, question);
      setLoadedPreferred(saved);
      setDraft(saved || suggestedAnswer);
      setErrorMsg(null);
    } catch (e) {
      setErrorMsg(String(e));
    }
  }, [sessionId, question, suggestedAnswer]);

  useEffect(() => {
    void loadPreferred();
  }, [loadPreferred]);

  const hasSaved = loadedPreferred.trim().length > 0;
  const isDirty = draft.trim() !== loadedPreferred.trim();

  const handleSave = async () => {
    const text = draft.trim();
    if (!text) return;
    setSaveState("saving");
    setErrorMsg(null);
    try {
      await savePreferredAnswer(sessionId, question, text);
      setLoadedPreferred(text);
      setSaveState("saved");
      onSaved?.();
      setTimeout(() => setSaveState("idle"), 2500);
    } catch (e) {
      setSaveState("error");
      setErrorMsg(String(e));
    }
  };

  const handleUseSuggestion = () => {
    if (suggestedAnswer.trim()) {
      setDraft(suggestedAnswer.trim());
    }
  };

  return (
    <div className="preferred-answer-panel">
      <div className="preferred-answer-panel__header">
        <button
          type="button"
          className="preferred-answer-panel__toggle"
          aria-expanded={expanded}
          onClick={() => setExpanded((v) => !v)}
        >
          <span className="preferred-answer-panel__title">Tailor for Live</span>
          <span className="preferred-answer-panel__chevron" aria-hidden>
            {expanded ? "▲" : "▼"}
          </span>
        </button>
        <div className="preferred-answer-panel__header-actions">
          <InfoPopover ariaLabel="How tailoring for Live works">
            <p>
              Flint&apos;s draft is a starting point. Edit it into words you would actually
              say — first person, natural, grounded in your real experience. Saved answers
              appear instantly during your live interview.
            </p>
          </InfoPopover>
          {hasSaved && (
            <span className="preferred-answer-panel__badge">Saved for Live</span>
          )}
        </div>
      </div>

      {expanded && (
        <>
      <label className="preferred-answer-panel__label" htmlFor="preferred-answer-body">
        Your preferred answer
      </label>
      <textarea
        id="preferred-answer-body"
        className="preferred-answer-panel__textarea"
        rows={4}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        placeholder="Write how you would say this out loud in the interview…"
      />

      {errorMsg && <p className="preferred-answer-panel__error">{errorMsg}</p>}
      {saveState === "saved" && (
        <p className="preferred-answer-panel__success">
          Saved — Flint will use this answer in Live for this question.
        </p>
      )}

      <div className="preferred-answer-panel__actions">
        {!hasSaved && suggestedAnswer.trim() && draft !== suggestedAnswer.trim() && (
          <button
            type="button"
            className="preferred-answer-panel__btn preferred-answer-panel__btn--secondary"
            onClick={handleUseSuggestion}
          >
            Use Flint draft
          </button>
        )}
        <button
          type="button"
          className="preferred-answer-panel__btn"
          disabled={!draft.trim() || saveState === "saving" || (!isDirty && hasSaved)}
          onClick={() => void handleSave()}
        >
          {saveState === "saving"
            ? "Saving…"
            : hasSaved
              ? "Update preferred answer"
              : "Save as preferred answer"}
        </button>
      </div>
        </>
      )}
    </div>
  );
}
