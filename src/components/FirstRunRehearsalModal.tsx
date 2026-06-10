import { useState } from "react";

import type { SessionContextFields } from "../commands";

interface FirstRunRehearsalModalProps {
  fields: SessionContextFields;
  onDismiss: () => void;
}

const EMPTY_FIELD_LABELS: Record<keyof SessionContextFields, string> = {
  jobDescription: "Job Description",
  profile: "Your Profile",
  companyOverview: "Company Overview",
  leadershipPrinciples: "Leadership Principles",
  roleExpectations: "Role Expectations",
  technicalPrep: "Technical Prep",
  strategyNotes: "Strategy Notes",
};

const STORAGE_KEY = "flint_first_run_modal_dismissed";

/** Read once: did the user already dismiss the modal with "don't show again"? */
export function isFirstRunModalDismissed(): boolean {
  try {
    return typeof localStorage !== "undefined"
      && localStorage.getItem(STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

export default function FirstRunRehearsalModal({
  fields,
  onDismiss,
}: FirstRunRehearsalModalProps) {
  const [dontShowAgain, setDontShowAgain] = useState(false);

  const emptyFields = (Object.keys(fields) as Array<keyof SessionContextFields>).filter(
    (k) => !fields[k]?.trim(),
  );

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
      <div className="first-run-modal">
        <h2 className="first-run-modal__title">Before you rehearse</h2>

        <p className="first-run-modal__body">
          Flint answers questions using <strong>only the context you paste</strong> — not the
          live web. The more you fill in, the more grounded and specific the answers will be.
        </p>

        {emptyFields.length > 0 && (
          <div className="first-run-modal__empty-section">
            <p className="first-run-modal__empty-label">Fields still empty:</p>
            <ul className="first-run-modal__empty-list">
              {emptyFields.map((k) => (
                <li key={k} className="first-run-modal__empty-item">
                  {EMPTY_FIELD_LABELS[k]}
                </li>
              ))}
            </ul>
          </div>
        )}

        <p className="first-run-modal__tip">
          Use the <strong>Prep Checklist</strong> sidebar to fill missing fields without
          leaving this screen.
        </p>

        <div className="first-run-modal__footer">
          <label className="first-run-modal__dont-show">
            <input
              type="checkbox"
              checked={dontShowAgain}
              onChange={(e) => setDontShowAgain(e.target.checked)}
            />
            <span>Don&apos;t show again for this session</span>
          </label>
          <button className="first-run-modal__dismiss-btn" onClick={handleDismiss}>
            Got it, start rehearsing
          </button>
        </div>
      </div>
    </div>
  );
}
