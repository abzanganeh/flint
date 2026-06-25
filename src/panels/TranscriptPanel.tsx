import { useCallback, useEffect, useRef, useState } from "react";

import { useTranscriptionStream } from "../hooks/useTranscriptionStream";
import type { Speaker } from "../types";

// ── Constants ────────────────────────────────────────────────────────────────

const MAX_LINES = 200;
const AUDIO_GAP_PREFIX = "[audio gap";

/// Consecutive chunks from the same speaker that arrive within this wall-clock
/// window are merged into a single utterance bubble. Whisper emits one chunk
/// per VAD segment (often 2-6 words), so without this the panel floods with
/// dozens of tiny fragments per sentence. The backend `timestamp` is a
/// relative elapsed value and unreliable for gap detection, so we use arrival
/// time on the UI side.
const UTTERANCE_MERGE_WINDOW_MS = 4000;

// ── Types ────────────────────────────────────────────────────────────────────

export interface TranscriptLine {
  id: number;
  text: string;
  speaker: Speaker;
  timestamp: number;
  /** Wall-clock arrival of the most recent fragment merged into this line. */
  lastArrivalMs: number;
  /** True when any merged fragment was auto-corrected by the heuristic. */
  corrected: boolean;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function isAudioGap(text: string): boolean {
  return text.startsWith(AUDIO_GAP_PREFIX);
}

function joinFragments(existing: string, addition: string): string {
  const trimmed = addition.trim();
  if (trimmed.length === 0) return existing;
  const needsSpace = existing.length > 0 && !existing.endsWith(" ");
  return needsSpace ? `${existing} ${trimmed}` : `${existing}${trimmed}`;
}

export function appendLine(
  prev: TranscriptLine[],
  text: string,
  speaker: Speaker,
  timestamp: number,
  labelSource: string | undefined,
  nextId: () => number,
): TranscriptLine[] {
  const arrival = Date.now();
  const corrected = labelSource === "heuristic";
  const last = prev[prev.length - 1];

  // Merge into the previous bubble when it is the same speaker, neither side is
  // an audio-gap marker, and the fragment arrived within the merge window.
  const canMerge =
    last !== undefined &&
    last.speaker === speaker &&
    !isAudioGap(last.text) &&
    !isAudioGap(text) &&
    arrival - last.lastArrivalMs <= UTTERANCE_MERGE_WINDOW_MS;

  if (canMerge) {
    const merged: TranscriptLine = {
      ...last,
      text: joinFragments(last.text, text),
      lastArrivalMs: arrival,
      corrected: last.corrected || corrected,
    };
    return [...prev.slice(0, -1), merged];
  }

  const next = [
    ...prev,
    {
      id: nextId(),
      text: text.trim(),
      speaker,
      timestamp,
      lastArrivalMs: arrival,
      corrected,
    },
  ];
  // Drop oldest lines when cap is reached.
  return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
}

// ── Component ────────────────────────────────────────────────────────────────

export interface TranscriptPanelProps {}

const TranscriptPanel = (_props: TranscriptPanelProps) => {
  const [lines, setLines] = useState<TranscriptLine[]>([]);
  const bottomRef = useRef<HTMLDivElement>(null);
  // Per-instance counter — avoids shared module-level mutable state.
  const lineIdRef = useRef(0);
  const nextId = () => ++lineIdRef.current;

  const onChunk = useCallback(
    (line: {
      text: string;
      speaker: Speaker;
      timestamp: number;
      labelSource?: string;
    }) => {
      setLines((prev) =>
        appendLine(
          prev,
          line.text,
          line.speaker,
          line.timestamp,
          line.labelSource,
          nextId,
        ),
      );
    },
    [],
  );

  useTranscriptionStream(onChunk);

  // Snap to bottom on every update. Using "instant" instead of "smooth"
  // because live transcripts receive bursts of chunks — smooth animations
  // compete with each other and produce visible stutter.
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "instant" });
  }, [lines]);

  return (
    <div
      data-testid="transcript-panel"
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
        backgroundColor: "#0f1117",
        fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
        fontSize: "13px",
      }}
    >
      {/* Header */}
      <div
        style={{
          padding: "6px 12px",
          borderBottom: "1px solid #1e2028",
          color: "#6b7280",
          fontSize: "11px",
          letterSpacing: "0.08em",
          textTransform: "uppercase",
          flexShrink: 0,
        }}
      >
        Transcript
      </div>

      {/* Scrollable line list */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "8px 0",
          display: "flex",
          flexDirection: "column",
          gap: "2px",
        }}
      >
        {lines.length === 0 && (
          <div
            style={{
              color: "#4b5563",
              padding: "16px 12px",
              fontStyle: "italic",
              fontSize: "12px",
            }}
          >
            Waiting for audio…
          </div>
        )}
        {lines.map((line) => (
          <TranscriptLineRow key={line.id} line={line} />
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  );
};

// ── Line row ─────────────────────────────────────────────────────────────────

interface TranscriptLineRowProps {
  line: TranscriptLine;
}

const TranscriptLineRow = ({ line }: TranscriptLineRowProps) => {
  if (isAudioGap(line.text)) {
    return <AudioGapRow text={line.text} />;
  }

  const isSystem = line.speaker === "System";

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: isSystem ? "flex-end" : "flex-start",
        padding: "2px 12px",
      }}
    >
      <span
        style={{
          fontSize: "10px",
          fontWeight: 600,
          letterSpacing: "0.06em",
          textTransform: "uppercase",
          color: isSystem ? "#3b82f6" : "#22c55e",
          marginBottom: "1px",
        }}
      >
        {isSystem ? "Interviewer" : "You"}
        {line.corrected && (
          <span
            title="Speaker auto-corrected from the capture channel"
            style={{
              marginLeft: 6,
              color: "#f59e0b",
              fontWeight: 500,
              textTransform: "none",
              letterSpacing: 0,
            }}
          >
            (auto)
          </span>
        )}
      </span>
      <span
        style={{
          color: "#e5e7eb",
          lineHeight: "1.5",
          maxWidth: "85%",
          textAlign: isSystem ? "right" : "left",
          wordBreak: "break-word",
        }}
      >
        {line.text}
      </span>
    </div>
  );
};

// ── Audio gap row ─────────────────────────────────────────────────────────────

const AudioGapRow = ({ text }: { text: string }) => (
  <div
    style={{
      display: "flex",
      justifyContent: "center",
      padding: "4px 12px",
    }}
  >
    <span
      style={{
        color: "#f59e0b",
        fontStyle: "italic",
        fontSize: "12px",
      }}
    >
      {text}
    </span>
  </div>
);

export default TranscriptPanel;
