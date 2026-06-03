import { useEffect, useRef, useState } from "react";

import { onTranscriptionChunk } from "../events";
import type { Speaker } from "../types";

// ── Constants ────────────────────────────────────────────────────────────────

const MAX_LINES = 200;
const AUDIO_GAP_PREFIX = "[audio gap";

// ── Types ────────────────────────────────────────────────────────────────────

interface TranscriptLine {
  id: number;
  text: string;
  speaker: Speaker;
  timestamp: number;
}

let lineIdCounter = 0;

function nextId(): number {
  return ++lineIdCounter;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function isAudioGap(text: string): boolean {
  return text.startsWith(AUDIO_GAP_PREFIX);
}

function appendLine(
  prev: TranscriptLine[],
  text: string,
  speaker: Speaker,
  timestamp: number,
): TranscriptLine[] {
  const next = [
    ...prev,
    { id: nextId(), text, speaker, timestamp },
  ];
  // Drop oldest lines when cap is reached.
  return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
}

// ── Component ────────────────────────────────────────────────────────────────

export interface TranscriptPanelProps {}

const TranscriptPanel = (_props: TranscriptPanelProps) => {
  const [lines, setLines] = useState<TranscriptLine[]>([]);
  const bottomRef = useRef<HTMLDivElement>(null);

  // Subscribe to Tauri transcription_chunk events.
  useEffect(() => {
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      unlisten = await onTranscriptionChunk(({ text, speaker, timestamp }) => {
        setLines((prev) => appendLine(prev, text, speaker, timestamp));
      });
    };

    void setup();

    return () => {
      unlisten?.();
    };
  }, []);

  // Auto-scroll to the latest line on every update.
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
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
