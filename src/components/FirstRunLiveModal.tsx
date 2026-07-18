import { useState } from "react";

interface FirstRunLiveModalProps {
  onDismiss: () => void;
}

const STORAGE_KEY = "flint_first_run_live_dismissed";

/** Read once: did the user dismiss the first-run live modal? */
export function isFirstRunLiveModalDismissed(): boolean {
  try {
    return (
      typeof localStorage !== "undefined" &&
      localStorage.getItem(STORAGE_KEY) === "true"
    );
  } catch {
    return false;
  }
}

export default function FirstRunLiveModal({ onDismiss }: FirstRunLiveModalProps) {
  const [dontShowAgain, setDontShowAgain] = useState(false);

  const handleDismiss = () => {
    if (dontShowAgain) {
      try {
        localStorage.setItem(STORAGE_KEY, "true");
      } catch {
        // Best-effort persistence.
      }
    }
    onDismiss();
  };

  return (
    <div className="first-run-modal__backdrop" role="dialog" aria-modal="true">
      <div className="first-run-modal" data-testid="first-run-live-modal">
        <h2 className="first-run-modal__title">Before you go live</h2>

        <p className="first-run-modal__body">
          Flint listens in the background and drafts answers in four panels:
          Transcript, Answer, Visual, and Context. Nothing is sent to the
          interviewer — only you see the overlay.
        </p>

        <ol className="first-run-modal__steps">
          <li>
            <strong>Transcript</strong> — live speech with per-line speaker labels.
            Swap any mislabeled line before asking Flint to respond.
          </li>
          <li>
            <strong>Answer</strong> — concise response draft for the current question.
          </li>
          <li>
            <strong>Visual</strong> — diagrams or code when a question needs a whiteboard
            (system design, architecture).
          </li>
          <li>
            <strong>Context</strong> — RAG hits and earlier Q&amp;A from your prep.
          </li>
        </ol>

        <p className="first-run-modal__tip">
          Press <strong>Ask now</strong> (Ctrl+Q) when the interviewer finishes their
          question — not while you are speaking. In phone mode, labels are best-effort;
          use per-line swap if needed.
        </p>

        <div className="first-run-modal__footer">
          <label className="first-run-modal__dont-show">
            <input
              type="checkbox"
              checked={dontShowAgain}
              onChange={(e) => setDontShowAgain(e.target.checked)}
              data-testid="first-run-live-dont-show"
            />
            <span>Don&apos;t show again</span>
          </label>
          <button
            className="first-run-modal__dismiss-btn"
            data-testid="first-run-live-dismiss"
            onClick={handleDismiss}
          >
            Got it — start live session
          </button>
        </div>
      </div>
    </div>
  );
}
