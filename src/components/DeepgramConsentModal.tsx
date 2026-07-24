import { useState } from "react";

export interface DeepgramConsentModalProps {
  onConfirm: () => void;
  onCancel: () => void;
}

export default function DeepgramConsentModal({
  onConfirm,
  onCancel,
}: DeepgramConsentModalProps) {
  const [checked, setChecked] = useState(false);

  return (
    <div className="first-run-modal__backdrop" role="dialog" aria-modal="true">
      <div className="first-run-modal" data-testid="deepgram-consent-modal">
        <h2 className="first-run-modal__title">Enable cloud transcription (Deepgram)</h2>
        <p className="first-run-modal__body">
          Flint's default transcription runs entirely on your device using Whisper.
          Switching to Deepgram sends live conversation audio to Deepgram's cloud
          servers for transcription. That audio includes the other participant's
          voice, not just yours.
        </p>
        <p className="first-run-modal__body">
          By enabling Deepgram, you confirm that you have any consent legally
          required to send this conversation to a third-party transcription
          service, in addition to Flint's own recording consent. You are
          responsible for compliance with your jurisdiction's laws.
        </p>
        <p className="first-run-modal__body">
          If Deepgram is unreachable during a session, Flint automatically falls
          back to local Whisper — you never lose transcription. You can switch
          back to Whisper-only in Settings at any time.
        </p>
        <label className="first-run-modal__dont-show">
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => setChecked(e.target.checked)}
            data-testid="deepgram-consent-checkbox"
          />
          <span>
            I understand audio will be sent to Deepgram, and I have any consent
            required to do so.
          </span>
        </label>
        <div className="first-run-modal__footer">
          <button
            type="button"
            className="first-run-modal__dismiss-btn"
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            type="button"
            className="rehearsal-footer__complete-btn rehearsal-footer__complete-btn--ready"
            disabled={!checked}
            data-testid="deepgram-consent-confirm-btn"
            onClick={onConfirm}
          >
            Enable Deepgram
          </button>
        </div>
      </div>
    </div>
  );
}
