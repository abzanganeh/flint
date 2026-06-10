import { useCallback, useEffect, useRef, useState } from "react";

import {
  addToQuestionBank,
  getQuestionBank,
  removeFromQuestionBank,
  runRehearsalTurn,
} from "../commands";

interface QuestionBankProps {
  sessionId: string;
  onAskQuestion?: (question: string) => void;
  asking?: boolean;
}

export default function QuestionBank({ sessionId, onAskQuestion, asking = false }: QuestionBankProps) {
  const [questions, setQuestions] = useState<string[]>([]);
  const [newQuestion, setNewQuestion] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const loadBank = useCallback(async () => {
    try {
      setLoading(true);
      const bank = await getQuestionBank(sessionId);
      setQuestions(bank);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  useEffect(() => {
    void loadBank();
  }, [loadBank]);

  const handleAdd = async () => {
    const trimmed = newQuestion.trim();
    if (!trimmed) return;
    try {
      const updated = await addToQuestionBank(sessionId, trimmed);
      setQuestions(updated);
      setNewQuestion("");
      setError(null);
      inputRef.current?.focus();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleRemove = async (question: string) => {
    try {
      const updated = await removeFromQuestionBank(sessionId, question);
      setQuestions(updated);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleAsk = (question: string) => {
    if (onAskQuestion) {
      onAskQuestion(question);
    } else {
      void runRehearsalTurn(sessionId, question);
    }
  };

  if (loading) return <div className="question-bank-loading">Loading questions…</div>;

  return (
    <div className="question-bank">
      <div className="question-bank__header">
        <span className="question-bank__title">Question Bank</span>
        <span className="question-bank__count">{questions.length}</span>
      </div>

      {error && <div className="question-bank__error">{error}</div>}

      <ul className="question-bank__list">
        {questions.map((q) => (
          <li key={q} className="question-bank__item">
            <button
              className="question-bank__ask-btn"
              onClick={() => handleAsk(q)}
              disabled={asking}
              title="Run this as a rehearsal turn"
            >
              ▶
            </button>
            <span className="question-bank__text">{q}</span>
            <button
              className="question-bank__remove-btn"
              onClick={() => void handleRemove(q)}
              title="Remove from bank"
            >
              ×
            </button>
          </li>
        ))}
        {questions.length === 0 && (
          <li className="question-bank__empty">No questions yet. Add one below.</li>
        )}
      </ul>

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
    </div>
  );
}
