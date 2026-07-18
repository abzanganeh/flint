import { useCallback, useEffect, useRef, useState } from "react";

import {
  getInterviewerSpanPreview,
  getProviderPriority,
  signalQuestionEnded,
} from "../commands";
import {
  onAnswerToken,
  onFailoverTriggered,
  onPrimaryRestored,
  onThreadStatus,
  onTurnStarted,
} from "../events";
import { useTranscriptionStream } from "../hooks/useTranscriptionStream";
import { useUIStore } from "../store/ui";

const SPAN_POLL_MS = 750;
const PHONE_FALLBACK_WINDOW_MS = 30_000;
const Q_FLASH_MS = 450;
const PROCESSING_IDLE_MS = 1_500;

type DetectionPhase = "listening" | "processing" | "detected" | "generating";

export interface LiveSessionStatusBarProps {
  sessionId: string;
  phoneCallMode?: boolean;
}

function providerDisplayName(name: string): string {
  const labels: Record<string, string> = {
    groq: "Groq",
    deepseek: "DeepSeek",
    openai: "OpenAI",
    anthropic: "Anthropic",
    openrouter: "OpenRouter",
    ollama: "Ollama",
  };
  return labels[name] ?? name;
}

interface FallbackLine {
  text: string;
  receivedAt: number;
}

const LiveSessionStatusBar = ({
  sessionId,
  phoneCallMode = false,
}: LiveSessionStatusBarProps) => {
  const [activeProvider, setActiveProvider] = useState("groq");
  const [failoverActive, setFailoverActive] = useState(false);
  const [detectionPhase, setDetectionPhase] = useState<DetectionPhase>("listening");
  const [spanText, setSpanText] = useState("");
  const [uncertainSpeaker, setUncertainSpeaker] = useState(false);
  const [fallbackLines, setFallbackLines] = useState<FallbackLine[]>([]);
  const [qFlash, setQFlash] = useState(false);
  const [capturedPreview, setCapturedPreview] = useState<string | null>(null);
  const [qError, setQError] = useState<string | null>(null);
  const processingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setLastManualQuestion = useUIStore((s) => s.setLastManualQuestion);
  const pushNotification = useUIStore((s) => s.pushNotification);

  const refreshSpanPreview = useCallback(async () => {
    try {
      const preview = await getInterviewerSpanPreview(sessionId);
      setSpanText(preview.text);
      setUncertainSpeaker(preview.uncertainSpeaker);
    } catch {
      // Non-fatal — preview degrades to empty/fallback.
    }
  }, [sessionId]);

  useEffect(() => {
    void getProviderPriority()
      .then((order) => {
        if (order[0]) setActiveProvider(order[0]);
      })
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    void refreshSpanPreview();
    const id = window.setInterval(() => {
      void refreshSpanPreview();
    }, SPAN_POLL_MS);
    return () => window.clearInterval(id);
  }, [refreshSpanPreview]);

  useEffect(() => {
    const unsubs: Array<Promise<() => void>> = [
      onFailoverTriggered(({ to }) => {
        setActiveProvider(to);
        setFailoverActive(true);
      }),
      onPrimaryRestored(({ provider }) => {
        setActiveProvider(provider);
        setFailoverActive(false);
      }),
      onTurnStarted(({ question }) => {
        setDetectionPhase("detected");
        setCapturedPreview(question);
        setLastManualQuestion(question);
        setQError(null);
        setSpanText("");
        setUncertainSpeaker(false);
        setFallbackLines([]);
      }),
      onAnswerToken(() => {
        setDetectionPhase("generating");
      }),
      onThreadStatus(({ thread, status }) => {
        if (thread === "answer" && status === "idle") {
          setDetectionPhase("listening");
        }
      }),
    ];

    let cancelled = false;
    const cleanups: Array<() => void> = [];

    void Promise.all(unsubs).then((fns) => {
      if (cancelled) {
        fns.forEach((fn) => fn());
      } else {
        cleanups.push(...fns);
      }
    });

    return () => {
      cancelled = true;
      cleanups.forEach((fn) => fn());
      if (processingTimerRef.current) {
        clearTimeout(processingTimerRef.current);
      }
    };
  }, [setLastManualQuestion]);

  const onSystemChunk = useCallback(
    (line: { text: string; speaker: string; timestamp: number }) => {
      if (line.speaker !== "System") return;
      void refreshSpanPreview();

      if (!phoneCallMode) return;

      const receivedAt = Date.now();
      const cutoff = receivedAt - PHONE_FALLBACK_WINDOW_MS;
      setFallbackLines((prev) => {
        const next = [...prev, { text: line.text, receivedAt }];
        return next.filter((entry) => entry.receivedAt >= cutoff);
      });

      setDetectionPhase("processing");
      if (processingTimerRef.current) clearTimeout(processingTimerRef.current);
      processingTimerRef.current = setTimeout(() => {
        setDetectionPhase((phase) => (phase === "processing" ? "listening" : phase));
      }, PROCESSING_IDLE_MS);
    },
    [phoneCallMode, refreshSpanPreview],
  );

  useTranscriptionStream(onSystemChunk);

  const fallbackText = fallbackLines.map((line) => line.text).join(" ");
  const usePhoneFallback =
    phoneCallMode && uncertainSpeaker && spanText.trim().length === 0 && fallbackText.length > 0;
  const displayText = usePhoneFallback ? fallbackText : spanText;

  /** Drain the backend System transcript buffer — authoritative for Q/Ctrl+Q. */
  const fireManualQuestion = useCallback(async () => {
    setQError(null);
    setQFlash(true);
    setDetectionPhase("generating");
    window.setTimeout(() => setQFlash(false), Q_FLASH_MS);
    try {
      await signalQuestionEnded(sessionId);
      setSpanText("");
      setUncertainSpeaker(false);
      setFallbackLines([]);
    } catch (err: unknown) {
      const message = String(err);
      setQError(message);
      setDetectionPhase("listening");
      pushNotification({
        id: `live-q-error-${Date.now()}`,
        level: "warn",
        message,
      });
    }
  }, [sessionId, pushNotification]);

  const detectionLabel =
    detectionPhase === "processing"
      ? "Processing…"
      : detectionPhase === "detected"
        ? "Question detected"
        : detectionPhase === "generating"
          ? "Generating response…"
          : phoneCallMode
            ? "Phone mode — press Q when interviewer finishes"
            : "Listening…";

  const providerDotClass = failoverActive
    ? "live-provider-badge__dot live-provider-badge__dot--amber"
    : activeProvider === "ollama"
      ? "live-provider-badge__dot live-provider-badge__dot--amber"
      : "live-provider-badge__dot";

  const detectionDotClass =
    detectionPhase === "processing"
      ? "live-detection-indicator__dot live-detection-indicator__dot--pulse"
      : detectionPhase === "detected"
        ? "live-detection-indicator__dot live-detection-indicator__dot--detected"
        : detectionPhase === "generating"
          ? "live-detection-indicator__dot live-detection-indicator__dot--generating"
          : "live-detection-indicator__dot";

  return (
    <div className="live-status-bar" data-testid="live-session-status-bar">
      <div className="live-status-bar__transcript">
        <span className="live-status-bar__transcript-label">
          {phoneCallMode ? "Interviewer span (phone)" : "Interviewer span"}
          {uncertainSpeaker && (
            <span
              data-testid="live-uncertain-speaker-hint"
              style={{ marginLeft: 6, color: "#f59e0b", textTransform: "none" }}
            >
              (uncertain speaker)
            </span>
          )}
        </span>
        <div className="live-status-bar__transcript-body" data-testid="live-rolling-transcript">
          {displayText ? (
            displayText
          ) : (
            <span className="live-status-bar__transcript-empty">
              {phoneCallMode
                ? "Waiting for audio… press Q when the interviewer finishes their question"
                : "Waiting for interviewer audio…"}
            </span>
          )}
        </div>
      </div>
      <div className="live-status-bar__controls">
        <div className="live-status-bar__badges">
          <span className="live-provider-badge" data-testid="live-provider-badge">
            <span className={providerDotClass} />
            {providerDisplayName(activeProvider)}
          </span>
          <span className="live-detection-indicator" data-testid="live-detection-indicator">
            <span className={detectionDotClass} />
            {detectionLabel}
          </span>
        </div>
        {capturedPreview && (
          <span className="live-q-capture-preview" data-testid="live-q-capture-preview">
            {capturedPreview}
          </span>
        )}
        {qError && (
          <span className="live-q-error" data-testid="live-q-error" style={{ color: "#f59e0b", fontSize: "11px" }}>
            {qError}
          </span>
        )}
        <button
          type="button"
          className={`live-q-button${qFlash ? " live-q-button--flash" : ""}`}
          data-testid="live-q-button"
          title="Mark question ended — sends captured interviewer speech to AI (Ctrl+Q)"
          onClick={() => void fireManualQuestion()}
        >
          Q
        </button>
      </div>
    </div>
  );
};

export default LiveSessionStatusBar;
