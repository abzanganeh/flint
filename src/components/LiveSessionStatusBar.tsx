import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getProviderPriority, triggerResponse } from "../commands";
import {
  onDirectionalToken,
  onFailoverTriggered,
  onPrimaryRestored,
  onThreadStatus,
  onTurnStarted,
} from "../events";
import { useTranscriptionStream } from "../hooks/useTranscriptionStream";
import { useUIStore } from "../store/ui";

const ROLLING_WINDOW_MS = 30_000;
const Q_FLASH_MS = 450;
const PROCESSING_IDLE_MS = 1_500;

type DetectionPhase = "listening" | "processing" | "detected" | "generating";

export interface LiveSessionStatusBarProps {
  sessionId: string;
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

const LiveSessionStatusBar = ({ sessionId }: LiveSessionStatusBarProps) => {
  const [activeProvider, setActiveProvider] = useState("groq");
  const [failoverActive, setFailoverActive] = useState(false);
  const [detectionPhase, setDetectionPhase] = useState<DetectionPhase>("listening");
  const [systemLines, setSystemLines] = useState<Array<{ text: string; timestamp: number }>>([]);
  const [qFlash, setQFlash] = useState(false);
  const [capturedPreview, setCapturedPreview] = useState<string | null>(null);
  const processingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setLastManualQuestion = useUIStore((s) => s.setLastManualQuestion);

  useEffect(() => {
    void getProviderPriority()
      .then((order) => {
        if (order[0]) setActiveProvider(order[0]);
      })
      .catch(() => undefined);
  }, []);

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
      }),
      onDirectionalToken(() => {
        setDetectionPhase("generating");
      }),
      onThreadStatus(({ thread, status }) => {
        if (thread === "directional" && status === "idle") {
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
  }, []);

  const onSystemChunk = useCallback(
    (line: { text: string; speaker: string; timestamp: number }) => {
      if (line.speaker !== "System") return;
      const cutoff = Date.now() - ROLLING_WINDOW_MS;
      setSystemLines((prev) => {
        const next = [...prev, { text: line.text, timestamp: line.timestamp || Date.now() }];
        return next.filter((entry) => (entry.timestamp || Date.now()) >= cutoff);
      });
      setDetectionPhase("processing");
      if (processingTimerRef.current) clearTimeout(processingTimerRef.current);
      processingTimerRef.current = setTimeout(() => {
        setDetectionPhase((phase) => (phase === "processing" ? "listening" : phase));
      }, PROCESSING_IDLE_MS);
    },
    [],
  );

  useTranscriptionStream(onSystemChunk);

  const rollingText = useMemo(
    () => systemLines.map((line) => line.text).join(" "),
    [systemLines],
  );

  const fireManualQuestion = useCallback(async () => {
    const text = rollingText.trim();
    if (!text) return;
    setQFlash(true);
    setCapturedPreview(text);
    setLastManualQuestion(text);
    setDetectionPhase("generating");
    window.setTimeout(() => setQFlash(false), Q_FLASH_MS);
    try {
      await triggerResponse(text, sessionId);
    } catch {
      setDetectionPhase("listening");
    }
  }, [rollingText, sessionId, setLastManualQuestion]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.metaKey || event.altKey) return;
      if (event.key.toLowerCase() !== "q") return;
      event.preventDefault();
      void fireManualQuestion();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [fireManualQuestion]);

  const detectionLabel =
    detectionPhase === "processing"
      ? "Processing…"
      : detectionPhase === "detected"
        ? "Question detected"
        : detectionPhase === "generating"
          ? "Generating response…"
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
        <span className="live-status-bar__transcript-label">Last 30s (interviewer)</span>
        <div className="live-status-bar__transcript-body" data-testid="live-rolling-transcript">
          {rollingText ? (
            rollingText
          ) : (
            <span className="live-status-bar__transcript-empty">Waiting for interviewer audio…</span>
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
        <button
          type="button"
          className={`live-q-button${qFlash ? " live-q-button--flash" : ""}`}
          data-testid="live-q-button"
          title="Mark question ended (Ctrl+Q)"
          onClick={() => void fireManualQuestion()}
        >
          Q
        </button>
      </div>
    </div>
  );
};

export default LiveSessionStatusBar;
