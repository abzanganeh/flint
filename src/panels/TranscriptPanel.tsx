import { useCallback, useEffect, useRef, useState } from "react";

import { copyTextToClipboard, relabelTranscriptChunk, triggerResponse } from "../commands";
import { onSpeakerRefined, onTranscriptChunkRelabeled } from "../events";
import { useTranscriptionStream } from "../hooks/useTranscriptionStream";
import { PANEL_ACCENTS } from "../lib/panelColors";
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
const UTTERANCE_MERGE_WINDOW_MS = 8000;

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
  /** Persisted chunk ids merged into this bubble (for per-line relabel). */
  chunkIds: string[];
  /** Most recent label provenance across merged fragments. */
  labelSource?: string;
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

export function splitIntoSentences(text: string): string[] {
  const trimmed = text.trim();
  if (trimmed.length === 0) return [];
  const matches = trimmed.match(/[^.!?]+[.!?]+|[^.!?]+$/g);
  const pieces = (matches ?? [trimmed])
    .map((s) => s.trim())
    .filter((s) => s.length >= Q_MIN_SENTENCE_CHARS);
  return pieces.length === 0 ? [trimmed] : pieces;
}

export function linesToPlainText(lines: TranscriptLine[]): string {
  return lines
    .filter((line) => !isAudioGap(line.text))
    .map((line) => {
      const label = line.speaker === "System" ? "INTERVIEWER" : "YOU";
      return `${label}: ${line.text.trim()}`;
    })
    .join("\n\n");
}

export function appendLine(
  prev: TranscriptLine[],
  text: string,
  speaker: Speaker,
  timestamp: number,
  labelSource: string | undefined,
  chunkId: string | undefined,
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
    const mergedIds = chunkId ? [...last.chunkIds, chunkId] : last.chunkIds;
    const merged: TranscriptLine = {
      ...last,
      text: joinFragments(last.text, text),
      lastArrivalMs: arrival,
      corrected: last.corrected || corrected,
      chunkIds: mergedIds,
      labelSource: labelSource ?? last.labelSource,
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
      chunkIds: chunkId ? [chunkId] : [],
      labelSource,
    },
  ];
  // Drop oldest lines when cap is reached.
  return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
}

/** Apply a backend relabel to every line that contains `chunkId`. */
export function applyChunkRelabel(
  lines: TranscriptLine[],
  chunkId: string,
  speaker: Speaker,
  labelSource: string,
): TranscriptLine[] {
  return lines.map((line) => {
    if (!line.chunkIds.includes(chunkId)) return line;
    return {
      ...line,
      speaker,
      corrected: labelSource === "heuristic",
      labelSource,
    };
  });
}

// Keep async dispatch testable without awaiting an internal click handler.
export const __triggerResponseImpl = { fn: triggerResponse };
export const __relabelImpl = { fn: relabelTranscriptChunk };

// ── Component ────────────────────────────────────────────────────────────────

export interface TranscriptPanelProps {
  /** Live session id — required for the Q-per-sentence dispatcher. */
  sessionId: string;
}

const TranscriptPanel = ({ sessionId }: TranscriptPanelProps) => {
  const [lines, setLines] = useState<TranscriptLine[]>([]);
  const [askingKey, setAskingKey] = useState<string | null>(null);
  const [askError, setAskError] = useState<string | null>(null);
  const [copyError, setCopyError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [swapError, setSwapError] = useState<string | null>(null);
  const [swappingLineId, setSwappingLineId] = useState<number | null>(null);
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
      chunkId?: string;
    }) => {
      setLines((prev) =>
        appendLine(
          prev,
          line.text,
          line.speaker,
          line.timestamp,
          line.labelSource,
          line.chunkId,
          nextId,
        ),
      );
    },
    [],
  );

  useTranscriptionStream(onChunk);

  useEffect(() => {
    let cancelled = false;
    const cleanups: Array<() => void> = [];

    void Promise.all([
      onTranscriptChunkRelabeled(({ chunk_id, speaker, label_source }) => {
        if (cancelled) return;
        setLines((prev) =>
          applyChunkRelabel(prev, chunk_id, speaker, label_source),
        );
      }),
      onSpeakerRefined(({ chunk_id, speaker, source }) => {
        if (cancelled) return;
        setLines((prev) => applyChunkRelabel(prev, chunk_id, speaker, source));
      }),
    ]).then((unsubs) => {
      if (cancelled) {
        unsubs.forEach((fn) => fn());
      } else {
        cleanups.push(...unsubs);
      }
    });

    return () => {
      cancelled = true;
      cleanups.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => {
    return () => {
      if (askingTimerRef.current !== null) {
        window.clearTimeout(askingTimerRef.current);
      }
    };
  }, []);

  const handleAsk = useCallback(
    async (key: string, utterance: string) => {
      const text = utterance.trim();
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

  const handleSwapSpeaker = useCallback(async (line: TranscriptLine) => {
    if (line.chunkIds.length === 0) return;
    const newSpeaker: Speaker = line.speaker === "System" ? "Microphone" : "System";
    setSwapError(null);
    setSwappingLineId(line.id);
    try {
      await Promise.all(
        line.chunkIds.map((chunkId) => __relabelImpl.fn(chunkId, newSpeaker)),
      );
      setLines((prev) =>
        prev.map((entry) =>
          entry.id === line.id
            ? {
                ...entry,
                speaker: newSpeaker,
                corrected: false,
                labelSource: "user",
              }
            : entry,
        ),
      );
    } catch (e: unknown) {
      setSwapError(String(e));
    } finally {
      setSwappingLineId(null);
    }
  }, []);

  const handleCopyTranscript = useCallback(() => {
    setCopyError(null);
    const text = linesToPlainText(lines);
    if (text.length === 0) {
      setCopyError("Nothing to copy yet.");
      return;
    }
    void copyTextToClipboard(text)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      })
      .catch((e: unknown) => {
        setCopyError(String(e));
      });
  }, [lines]);

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
          borderBottom: `1px solid ${PANEL_ACCENTS.transcript.headerBorder}`,
          backgroundColor: PANEL_ACCENTS.transcript.headerBg,
          color: PANEL_ACCENTS.transcript.text,
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
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <button
            type="button"
            data-testid="copy-transcript-btn"
            onClick={handleCopyTranscript}
            disabled={lines.length === 0}
            style={{
              color: copied ? "#22c55e" : "#6b7280",
              fontSize: "10px",
              letterSpacing: "0.04em",
              textTransform: "none",
              background: "transparent",
              border: "1px solid #1f2937",
              borderRadius: 4,
              padding: "2px 8px",
              cursor: lines.length === 0 ? "default" : "pointer",
            }}
            title="Copy the full transcript without selecting text"
          >
            {copied ? "Copied" : "Copy"}
          </button>
          <span
            style={{
              color: "#4b5563",
              fontSize: "10px",
              letterSpacing: "0.04em",
              textTransform: "none",
            }}
            title="Q sends the full merged interviewer utterance (same scope as Ctrl+Q for that burst)."
          >
            Q = full question
          </span>
        </div>
      </div>

      {copyError && (
        <div
          data-testid="transcript-copy-error"
          style={{
            padding: "6px 12px",
            color: "#ef4444",
            fontSize: "11px",
            borderBottom: "1px solid #1e2028",
            backgroundColor: "#1a0d0d",
          }}
        >
          {copyError}
        </div>
      )}

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

      {swapError && (
        <div
          data-testid="transcript-swap-error"
          style={{
            padding: "6px 12px",
            color: "#ef4444",
            fontSize: "11px",
            borderBottom: "1px solid #1e2028",
            backgroundColor: "#1a0d0d",
          }}
        >
          {swapError}
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
            onSwap={handleSwapSpeaker}
            swapping={swappingLineId === line.id}
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
  onAsk: (key: string, utterance: string) => void;
  onSwap: (line: TranscriptLine) => void;
  swapping: boolean;
}

const TranscriptLineRow = ({
  line,
  askingKey,
  onAsk,
  onSwap,
  swapping,
}: TranscriptLineRowProps) => {
  if (isAudioGap(line.text)) {
    return <AudioGapRow text={line.text} />;
  }

  const isSystem = line.speaker === "System";
  const canSwap = line.chunkIds.length > 0;

  if (!isSystem) {
    return (
      <UserBubble
        line={line}
        canSwap={canSwap}
        swapping={swapping}
        onSwap={() => onSwap(line)}
      />
    );
  }

  const key = String(line.id);
  const status = askingKey === key ? "asking" : "idle";
  const utterance = line.text.trim();
  const showQ = utterance.length >= Q_MIN_SENTENCE_CHARS;

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
      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <SpeakerLabel isSystem corrected={line.corrected} />
        {canSwap && (
          <SpeakerSwapButton
            swapping={swapping}
            onSwap={() => onSwap(line)}
            title="Swap speaker label for this line"
          />
        )}
      </div>
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
          {utterance}
        </span>
        {showQ && (
          <button
            type="button"
            data-testid="q-chip"
            data-status={status}
            onClick={() => onAsk(key, utterance)}
            disabled={status === "asking"}
            title={
              status === "asking"
                ? "Asking the AI to answer this question…"
                : "Send the full merged interviewer utterance to the AI"
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
        )}
      </div>
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

const SpeakerSwapButton = ({
  swapping,
  onSwap,
  title,
}: {
  swapping: boolean;
  onSwap: () => void;
  title: string;
}) => (
  <button
    type="button"
    data-testid="speaker-swap-btn"
    onClick={onSwap}
    disabled={swapping}
    title={title}
    style={{
      padding: "0 6px",
      height: 18,
      borderRadius: 4,
      border: "1px solid #1f2937",
      backgroundColor: "transparent",
      color: swapping ? "#4b5563" : "#9ca3af",
      fontSize: 9,
      fontWeight: 600,
      letterSpacing: "0.04em",
      textTransform: "uppercase",
      cursor: swapping ? "default" : "pointer",
      lineHeight: 1,
    }}
  >
    {swapping ? "…" : "Swap"}
  </button>
);

const UserBubble = ({
  line,
  canSwap,
  swapping,
  onSwap,
}: {
  line: TranscriptLine;
  canSwap: boolean;
  swapping: boolean;
  onSwap: () => void;
}) => (
  <div
    style={{
      display: "flex",
      flexDirection: "column",
      alignItems: "flex-start",
      padding: "2px 12px",
    }}
  >
    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
      <SpeakerLabel isSystem={false} corrected={line.corrected} />
      {canSwap && (
        <SpeakerSwapButton
          swapping={swapping}
          onSwap={onSwap}
          title="Swap speaker label for this line"
        />
      )}
    </div>
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
