import { useCallback, useEffect, useRef, useState } from "react";

import {
  addToQuestionBank,
  getFocusTagCatalog,
  getQuestionBank,
  type FocusTagCatalogEntry,
  type QuestionBankEntry,
  removeFromQuestionBank,
  runRehearsalTurn,
} from "../commands";
import {
  readShuffleQuestionsPreference,
  writeShuffleQuestionsPreference,
} from "../lib/shufflePreference";

interface QuestionBankProps {
  sessionId: string;
  onAskQuestion?: (question: string) => void;
  asking?: boolean;
  /** Bump after a rehearsal turn completes to refresh practice status. */
  refreshKey?: number;
}

function scoreLabel(entry: QuestionBankEntry): string | null {
  if (!entry.satisfied) return null;
  if (entry.lastSource === "mock" && entry.coachScore > 0) {
    return `Coach ${entry.coachScore}`;
  }
  if (entry.confidenceScore > 0) {
    return `Conf ${Math.round(entry.confidenceScore * 100)}%`;
  }
  return "Practiced";
}

export default function QuestionBank({
  sessionId,
  onAskQuestion,
  asking = false,
  refreshKey = 0,
}: QuestionBankProps) {
  const [entries, setEntries] = useState<QuestionBankEntry[]>([]);
  const [newQuestion, setNewQuestion] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [shuffleQuestions, setShuffleQuestions] = useState(readShuffleQuestionsPreference);
  const [tagCatalog, setTagCatalog] = useState<FocusTagCatalogEntry[]>([]);
  const [tagPickerOpen, setTagPickerOpen] = useState(false);
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);

  const loadBank = useCallback(async () => {
    try {
      setLoading(true);
      const bank = await getQuestionBank(sessionId, shuffleQuestions);
      setEntries(bank);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [sessionId, shuffleQuestions, refreshKey]);

  useEffect(() => {
    void loadBank();
  }, [loadBank]);

  useEffect(() => {
    getFocusTagCatalog(sessionId)
      .then(setTagCatalog)
      .catch(() => setTagCatalog([]));
  }, [sessionId]);

  const handleShuffleToggle = (enabled: boolean) => {
    setShuffleQuestions(enabled);
    writeShuffleQuestionsPreference(enabled);
  };

  const toggleSelectedTag = (tagId: string) => {
    setSelectedTags((prev) =>
      prev.includes(tagId) ? prev.filter((t) => t !== tagId) : [...prev, tagId],
    );
  };

  const handleAdd = async () => {
    const trimmed = newQuestion.trim();
    if (!trimmed) return;
    try {
      await addToQuestionBank(sessionId, trimmed, selectedTags.length > 0 ? selectedTags : undefined);
      await loadBank();
      setNewQuestion("");
      setSelectedTags([]);
      setError(null);
      inputRef.current?.focus();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleRemove = async (question: string) => {
    try {
      await removeFromQuestionBank(sessionId, question);
      await loadBank();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleAsk = (entry: QuestionBankEntry, force = false) => {
    if (entry.satisfied && !force) {
      const ok = window.confirm(
        "You already practiced this question well. Run it again anyway?",
      );
      if (!ok) return;
    }
    if (onAskQuestion) {
      onAskQuestion(entry.question);
    } else {
      void runRehearsalTurn(sessionId, entry.question);
    }
  };

  const pending = entries.filter((e) => !e.satisfied);
  const completed = entries.filter((e) => e.satisfied);

  if (loading) return <div className="question-bank-loading">Loading questions…</div>;

  const renderItem = (entry: QuestionBankEntry, completedRow: boolean) => (
    <li
      key={entry.question}
      className={
        completedRow
          ? "question-bank__item question-bank__item--completed"
          : "question-bank__item"
      }
    >
      <button
        className="question-bank__ask-btn"
        onClick={() => handleAsk(entry, completedRow)}
        disabled={asking}
        title={
          completedRow
            ? "Practice this question again"
            : "Run this as a rehearsal turn"
        }
      >
        {completedRow ? "↻" : "▶"}
      </button>
      <span className="question-bank__text">{entry.question}</span>
      {entry.hasPreferredAnswer && (
        <span className="question-bank__preferred" title="Tailored answer saved for Live">
          Live
        </span>
      )}
      {completedRow && scoreLabel(entry) && (
        <span className="question-bank__score">{scoreLabel(entry)}</span>
      )}
      <button
        className="question-bank__remove-btn"
        onClick={() => void handleRemove(entry.question)}
        title="Remove from bank"
      >
        ×
      </button>
    </li>
  );

  return (
    <div className="question-bank">
      <div className="question-bank__header">
        <span className="question-bank__title">Question Bank</span>
        <span className="question-bank__count">{entries.length}</span>
      </div>

      <label className="question-bank__shuffle" title="Randomize pending questions (stable per session)">
        <input
          type="checkbox"
          checked={shuffleQuestions}
          onChange={(e) => handleShuffleToggle(e.target.checked)}
          data-testid="rehearsal-shuffle-questions"
        />
        <span>Shuffle order</span>
      </label>

      {error && <div className="question-bank__error">{error}</div>}

      <ul className="question-bank__list">
        {pending.map((e) => renderItem(e, false))}
        {pending.length === 0 && completed.length === 0 && (
          <li className="question-bank__empty">No questions yet. Add one below.</li>
        )}
        {pending.length === 0 && completed.length > 0 && (
          <li className="question-bank__empty">All questions practiced — review below or add more.</li>
        )}
      </ul>

      {completed.length > 0 && (
        <>
          <div className="question-bank__section-label">Practiced well</div>
          <ul className="question-bank__list question-bank__list--completed">
            {completed.map((e) => renderItem(e, true))}
          </ul>
        </>
      )}

      <div className="question-bank__add">
        <input
          ref={inputRef}
          className="question-bank__input"
          type="text"
          placeholder="Add a question…"
          value={newQuestion}
          onChange={(e) => setNewQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void handleAdd();
          }}
        />
        <button
          className="question-bank__add-btn"
          onClick={() => void handleAdd()}
          disabled={!newQuestion.trim()}
        >
          Add
        </button>
      </div>

      <button
        type="button"
        className="question-bank__tag-toggle"
        onClick={() => setTagPickerOpen((v) => !v)}
        data-testid="question-bank-tag-toggle"
      >
        {tagPickerOpen ? "− Tag (optional)" : "+ Tag (optional)"}
        {selectedTags.length > 0 ? ` (${selectedTags.length})` : ""}
      </button>

      {tagPickerOpen && (
        <div className="question-bank__tag-picker" data-testid="question-bank-tag-picker">
          {tagCatalog.map((tag) => {
            const active = selectedTags.includes(tag.id);
            return (
              <button
                key={tag.id}
                type="button"
                data-testid={`question-bank-tag-chip-${tag.id}`}
                className={`question-bank__tag-chip${active ? " question-bank__tag-chip--active" : ""}${
                  tag.questionCount === 0 ? " question-bank__tag-chip--muted" : ""
                }`}
                title={tag.description}
                onClick={() => toggleSelectedTag(tag.id)}
              >
                {tag.label}
                <span className="question-bank__tag-chip-count">{tag.questionCount}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
