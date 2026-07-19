import type { LiveReadinessReportDto } from "../commands";

export interface LiveReadinessModalProps {
  report: LiveReadinessReportDto;
  busy: boolean;
  onClose: () => void;
  onRunAudioPreview: () => void;
}

const STATUS_ICON: Record<string, string> = {
  pass: "✓",
  warn: "⚠",
  fail: "✗",
};

export default function LiveReadinessModal({
  report,
  busy,
  onClose,
  onRunAudioPreview,
}: LiveReadinessModalProps) {
  return (
    <div className="first-run-modal__backdrop" role="dialog" aria-modal="true">
      <div className="first-run-modal" data-testid="live-readiness-modal">
        <h2 className="first-run-modal__title">Live session readiness</h2>
        <p className="first-run-modal__body">
          Flint checked your device, audio routing, API keys, and Private Mode
          requirements before you go live.
        </p>

        <ul className="first-run-modal__empty-list" style={{ marginBottom: 12 }}>
          {report.checks.map((row) => (
            <li key={row.check} style={{ marginBottom: 8 }}>
              <span aria-hidden>{STATUS_ICON[row.status] ?? "•"} </span>
              <strong>{row.check.replaceAll("_", " ")}</strong>: {row.message}
              {row.fixInstruction && row.status !== "pass" ? (
                <div style={{ fontSize: 12, color: "#94a3b8", marginTop: 4, whiteSpace: "pre-line" }}>
                  {row.fixInstruction}
                </div>
              ) : null}
            </li>
          ))}
        </ul>

        {report.headphoneGate.blocked ? (
          <p className="first-run-modal__tip" role="alert">
            {report.headphoneGate.message}
            {report.headphoneGate.fixInstruction ? ` ${report.headphoneGate.fixInstruction}` : ""}
          </p>
        ) : null}

        <div className="first-run-modal__footer">
          <button type="button" className="first-run-modal__dismiss-btn" onClick={onClose}>
            Close
          </button>
          <button
            type="button"
            className="rehearsal-footer__complete-btn rehearsal-footer__complete-btn--ready"
            disabled={!report.ready || busy}
            data-testid="live-readiness-audio-preview-btn"
            onClick={onRunAudioPreview}
          >
            {busy ? "Starting preview…" : "Run 60s audio preview"}
          </button>
        </div>
      </div>
    </div>
  );
}
