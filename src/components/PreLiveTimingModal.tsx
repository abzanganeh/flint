export interface PreLiveTimingModalProps {
  onDismiss: () => void;
}

/**
 * Shown when entering Rehearsal or Mock Interview — users must start Flint live
 * early enough to complete consent and setup popups before the real call.
 */
export default function PreLiveTimingModal({ onDismiss }: PreLiveTimingModalProps) {
  return (
    <div className="first-run-modal__backdrop" role="dialog" aria-modal="true">
      <div className="first-run-modal" data-testid="pre-live-timing-modal">
        <h2 className="first-run-modal__title">Start live early</h2>

        <p className="first-run-modal__body">
          Open your <strong>live Flint session at least 2 minutes before</strong> your
          scheduled interview time. Before audio begins you will need to:
        </p>

        <ol className="first-run-modal__steps">
          <li>
            Pass the <strong>60-second audio preview</strong> (TEST — NOT LIVE; no AI
            responses yet).
          </li>
          <li>
            Confirm <strong>recording consent</strong> when you click Go Live.
          </li>
          <li>
            Review the <strong>first-run live tips</strong> if you have not dismissed them.
          </li>
        </ol>

        <p className="first-run-modal__tip">
          Run <strong>Test Live Session</strong> from Rehearsal at least once with your
          final headphones, mic, and audio routing before the real interview. Saved
          preferred answers only apply during the actual live session — not during preview.
        </p>

        <div className="first-run-modal__footer">
          <button
            type="button"
            className="first-run-modal__dismiss-btn"
            data-testid="pre-live-timing-dismiss"
            onClick={onDismiss}
          >
            Got it
          </button>
        </div>
      </div>
    </div>
  );
}
