import { useState } from "react";

export interface RecordingConsentModalProps {
  onConfirm: () => void;
  onCancel: () => void;
}

export default function RecordingConsentModal({
  onConfirm,
  onCancel,
}: RecordingConsentModalProps) {
  const [checked, setChecked] = useState(false);

  return (
    <div className="first-run-modal__backdrop" role="dialog" aria-modal="true">
      <div className="first-run-modal" data-testid="recording-consent-modal">
        <h2 className="first-run-modal__title">Recording consent required</h2>
        <p className="first-run-modal__body">
          Flint transcribes conversation audio locally to assist you during a live
          session. Laws vary by location: some jurisdictions require every participant
          to consent before a conversation is recorded or transcribed.
        </p>
        <p className="first-run-modal__body">
          By continuing, you confirm that you have the legal right to capture and
          transcribe this specific conversation, and that you will inform other
          participants and obtain any required consent before the live session starts.
        </p>
        <label className="first-run-modal__dont-show">
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => setChecked(e.target.checked)}
            data-testid="recording-consent-checkbox"
          />
          <span>I confirm I have the legal right to record/transcribe this conversation</span>
        </label>
        <div className="first-run-modal__footer">
          <button type="button" className="first-run-modal__dismiss-btn" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="rehearsal-footer__complete-btn rehearsal-footer__complete-btn--ready"
            disabled={!checked}
            data-testid="recording-consent-confirm-btn"
            onClick={onConfirm}
          >
            Continue to Live
          </button>
        </div>
      </div>
    </div>
  );
}
