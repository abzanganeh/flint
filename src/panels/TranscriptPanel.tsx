import { useCallback, useEffect, useRef, useState } from "react";

import { triggerResponse } from "../commands";
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

/// How long the "Asking…" affordance stays visible after clicking a Q chip
/// before the chip resets to idle. The user can re-click sooner than this by
/// clicking another sentence — only one "asking" target is tracked at a time.
const Q_ASKING_LATCH_MS = 8000;

/// Sentences below this character length are treated as filler/aborts (e.g.
/// "Uh.", "Hmm?") and don't get their own Q chip.
const Q_MIN_SENTENCE_CHARS = 3;

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

/**
 * Split an interviewer utterance into individual sentences for Q-per-sentence.
 *
 * The matcher captures runs of non-terminal characters followed by one or more
 * `.?!` (greedily, so trailing "?!" stays attached), and a final tail without
 * terminal punctuation as one in-progress sentence. Whitespace is trimmed and
 * empty / overly-short fragments are dropped.
 */
export function splitIntoSentences(text: string): string[] {
  const trimmed = text.trim();
  if (trimmed.length === 0) return [];
  const matches = trimmed.match(/[^.!?]+[.!?]+|[^.!?]+$/g);
  const pieces = (matches ?? [trimmed])
    .map((s) => s.trim())
    .filter((s) => s.length >= Q_MIN_SENTENCE_CHARS);
  return pieces.length === 0 ? [trimmed] : pieces;
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

// Keep async dispatch testable without awaiting an internal click handler.
export const __triggerResponseImpl = { fn: triggerResponse };

// ── Component ────────────────────────────────────────────────────────────────

export interface TranscriptPanelProps {
  /** Live session id — required for the Q-per-sentence dispatcher. */
  sessionId: string;
}

const TranscriptPanel = ({ sessionId }: TranscriptPanelProps) => {
  const [lines, setLines] = useState<TranscriptLine[]>([]);
  const [askingKey, setAskingKey] = useState<string | null>(null);
  const [askError, setAskError] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const askingTimerRef = useRef<number | null>(null);
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

  useEffect(() => {
    return () => {
      if (askingTimerRef.current !== null) {
        window.clearTimeout(askingTimerRef.current);
      }
    };
  }, []);

  const handleAsk = useCallback(
    async (key: string, sentence: string) => {
      const text = sentence.trim();
      if (text.length === 0) return;

      setAskError(null);
      setAskingKey(key);
      if (askingTimerRef.current !== null) {
        window.clearTimeout(askingTimerRef.current);
      }
      askingTimerRef.current = window.setTimeout(() => {
        setAskingKey((current) => (current === key ? null : current));
        askingTimerRef.current = null;
      }, Q_ASKING_LATCH_MS);

      try {
        await __triggerResponseImpl.fn(text, sessionId);
      } catch (e: unknown) {
        setAskError(String(e));
        setAskingKey((current) => (current === key ? null : current));
        if (askingTimerRef.current !== null) {
          window.clearTimeout(askingTimerRef.current);
          askingTimerRef.current = null;
        }
      }
    },
    [sessionId],
  );

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
      <div
        style={{
          padding: "6px 12px",
          borderBottom: "1px solid #1e2028",
          color: "#6b7280",
          fontSize: "11px",
          letterSpacing: "0.08em",
          textTransform: "uppercase",
          flexShrink: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
        }}
      >
        <span>Transcript</span>
        <span
          style={{
            color: "#4b5563",
            fontSize: "10px",
            letterSpacing: "0.04em",
            textTransform: "none",
          }}
          title="Click Q on any interviewer sentence to send only that sentence to the AI."
        >
          Click Q to answer that sentence
        </span>
      </div>

      {askError && (
        <div
          data-testid="transcript-ask-error"
          style={{
            padding: "6px 12px",
            color: "#ef4444",
            fontSize: "11px",
            borderBottom: "1px solid #1e2028",
            backgroundColor: "#1a0d0d",
          }}
        >
          {askError}
        </div>
      )}

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
          <TranscriptLineRow
            key={line.id}
            line={line}
            askingKey={askingKey}
            onAsk={handleAsk}
          />
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  );
};

// ── Line row ─────────────────────────────────────────────────────────────────

interface TranscriptLineRowProps {
  line: TranscriptLine;
  askingKey: string | null;
  onAsk: (key: string, sentence: string) => void;
}

const TranscriptLineRow = ({ line, askingKey, onAsk }: TranscriptLineRowProps) => {
  if (isAudioGap(line.text)) {
    return <AudioGapRow text={line.text} />;
  }

  const isSystem = line.speaker === "System";

  if (!isSystem) {
    return <UserBubble line={line} />;
  }

  const sentences = splitIntoSentences(line.text);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "flex-end",
        padding: "2px 12px",
        gap: 2,
      }}
    >
      <SpeakerLabel isSystem corrected={line.corrected} />
      {sentences.map((sentence, idx) => {
        const key = `${line.id}:${idx}`;
        const status = askingKey === key ? "asking" : "idle";
        return (
          <InterviewerSentence
            key={key}
            sentence={sentence}
            status={status}
            onClick={() => onAsk(key, sentence)}
          />
        );
      })}
    </div>
  );
};

interface InterviewerSentenceProps {
  sentence: string;
  status: "idle" | "asking";
  onClick: () => void;
}

const InterviewerSentence = ({ sentence, status, onClick }: InterviewerSentenceProps) => {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "row",
        alignItems: "flex-start",
        justifyContent: "flex-end",
        gap: 6,
        maxWidth: "90%",
      }}
    >
      <span
        style={{
          color: "#e5e7eb",
          lineHeight: 1.5,
          textAlign: "right",
          wordBreak: "break-word",
          flex: 1,
        }}
      >
        {sentence}
      </span>
      <button
        type="button"
        data-testid="q-chip"
        data-status={status}
        onClick={onClick}
        disabled={status === "asking"}
        title={
          status === "asking"
            ? "Asking the AI to answer this sentence…"
            : "Send only this sentence to the AI"
        }
        style={{
          flexShrink: 0,
          minWidth: 26,
          height: 22,
          padding: "0 7px",
          borderRadius: 11,
          border: `1px solid ${status === "asking" ? "#1e3a8a" : "#1f2937"}`,
          backgroundColor: status === "asking" ? "#1e3a8a" : "transparent",
          color: status === "asking" ? "#bfdbfe" : "#3b82f6",
          fontSize: 10,
          fontWeight: 700,
          letterSpacing: "0.04em",
          cursor: status === "asking" ? "default" : "pointer",
          marginTop: 2,
          lineHeight: 1,
        }}
      >
        {status === "asking" ? "Asking…" : "Q"}
      </button>
    </div>
  );
};

interface SpeakerLabelProps {
  isSystem: boolean;
  corrected: boolean;
}

const SpeakerLabel = ({ isSystem, corrected }: SpeakerLabelProps) => (
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
    {corrected && (
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
);

const UserBubble = ({ line }: { line: TranscriptLine }) => (
  <div
    style={{
      display: "flex",
      flexDirection: "column",
      alignItems: "flex-start",
      padding: "2px 12px",
    }}
  >
    <SpeakerLabel isSystem={false} corrected={line.corrected} />
    <span
      style={{
        color: "#e5e7eb",
        lineHeight: "1.5",
        maxWidth: "85%",
        textAlign: "left",
        wordBreak: "break-word",
      }}
    >
      {line.text}
    </span>
  </div>
);

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
