import { useState } from "react";

import { appendResearchToContext } from "../commands";

interface StoryEditorProps {
  sessionId: string;
  /** Pre-fill the question field (e.g. last asked question). */
  defaultQuestion?: string;
  onSaved?: (chunksAdded: number, reask: boolean) => void;
}

type SaveState = "idle" | "saving" | "saved" | "error";

/**
 * Proactive story editor — add answer material for any question without
 * leaving Rehearsal. Embeds into RAG via append_research_to_context.
 */
export default function StoryEditor({
  sessionId,
  defaultQuestion = "",
  onSaved,
}: StoryEditorProps) {
  const [question, setQuestion] = useState(defaultQuestion);
  const [story, setStory] = useState("");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const handleSave = async (reask: boolean) => {
    const q = question.trim();
    const s = story.trim();
    if (!q || !s) return;
    setSaveState("saving");
    setErrorMsg(null);
    try {
      const result = await appendResearchToContext(sessionId, q, s, []);
      setSaveState("saved");
      setStory("");
      onSaved?.(result.chunksAdded, reask);
      setTimeout(() => setSaveState("idle"), 2500);
    } catch (e) {
      setSaveState("error");
      setErrorMsg(String(e));
    }
  };

  return (
    <div className="story-editor">
      <p className="story-editor__hint">
        Link a personal story or project notes to a specific interview question.
        Flint embeds it into your context immediately.
      </p>
      <label className="story-editor__label" htmlFor="story-question">
        Interview question
      </label>
      <input
        id="story-question"
        className="story-editor__input"
        type="text"
        value={question}
        onChange={(e) => setQuestion(e.target.value)}
        placeholder="e.g. Describe a project with significant technical debt…"
      />
      <label className="story-editor__label" htmlFor="story-body">
        Your answer / story
      </label>
      <textarea
        id="story-body"
        className="story-editor__textarea"
        rows={8}
        value={story}
        onChange={(e) => setStory(e.target.value)}
        placeholder="Paste your STAR story, project details, metrics…"
      />
      {errorMsg && <p className="story-editor__error">{errorMsg}</p>}
      {saveState === "saved" && (
        <p className="story-editor__success">Saved to context.</p>
      )}
      <div className="story-editor__actions">
        <button
          type="button"
          className="story-editor__save-btn story-editor__save-btn--secondary"
          disabled={!question.trim() || !story.trim() || saveState === "saving"}
          onClick={() => void handleSave(false)}
        >
          {saveState === "saving" ? "Saving…" : "Save only"}
        </button>
        <button
          type="button"
          className="story-editor__save-btn"
          disabled={!question.trim() || !story.trim() || saveState === "saving"}
          onClick={() => void handleSave(true)}
        >
          {saveState === "saving" ? "Saving…" : "Save & Re-ask"}
        </button>
      </div>
    </div>
  );
}
