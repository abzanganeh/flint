import { assignSpeaker } from "../commands";

export interface SpeakerPickerProps {
  segments: Array<{ speakerId: number; sampleText: string }>;
  onAssigned: () => void;
}

export function SpeakerPicker({ segments, onAssigned }: SpeakerPickerProps) {
  const handlePick = (speakerId: number) => {
    void assignSpeaker(speakerId)
      .then(() => onAssigned())
      .catch(() => undefined);
  };

  return (
    <div className="speaker-picker" data-testid="speaker-picker">
      <p className="speaker-picker-title">We detected two speakers. Who is the interviewer?</p>
      <ul className="speaker-picker-list">
        {segments.map((seg) => (
          <li key={seg.speakerId}>
            <button
              type="button"
              className="speaker-picker-btn"
              data-testid={`speaker-pick-${seg.speakerId}`}
              onClick={() => handlePick(seg.speakerId)}
            >
              Speaker {seg.speakerId + 1}: {seg.sampleText.slice(0, 80)}
              {seg.sampleText.length > 80 ? "…" : ""}
            </button>
          </li>
        ))}
      </ul>
      <p className="speaker-picker-hint">Or press Ctrl+Q when the interviewer finishes a question.</p>
    </div>
  );
}

export default SpeakerPicker;
