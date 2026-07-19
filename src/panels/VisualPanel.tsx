import { useEffect, useMemo, useRef, useState } from "react";
import mermaid from "mermaid";
import { codeToHtml } from "shiki";

import { copyTextToClipboard } from "../commands";
import { useUIStore } from "../store/ui";
import { QuestionHeading } from "./TurnCards";

mermaid.initialize({
  startOnLoad: false,
  theme: "dark",
  securityLevel: "strict",
  fontFamily: "'Inter', 'SF Pro Text', system-ui, sans-serif",
});

export interface VisualPanelProps {
  isGenerating?: boolean;
  /**
   * When present with `onGenerateDiagram`, enables the manual "Generate
   * diagram" trigger (and silent Mermaid auto-retry). Parents inject the
   * dispatch path — Live uses `triggerVisualResponse`, Rehearsal uses
   * `runRehearsalTurn(..., forceVisual: true)`.
   */
  sessionId?: string;
  /** Injected trigger; required for Generate diagram / auto-retry dispatch. */
  onGenerateDiagram?: (question: string) => Promise<void>;
}

export interface FencedBlock {
  lang: string;
  code: string;
}

/**
 * Matches the diagram declaration mermaid's own parser dispatches on (see
 * `detectType` in mermaid's source) — keyed off the block's content, not its
 * fence language tag, so detection survives the model tagging the fence as
 * ```mermaid, ```flowchart, or leaving the tag off entirely. Covers all 10
 * diagram types the Visual prompt (prompts/visual/default.txt) can produce.
 */
const MERMAID_DIAGRAM_PATTERN =
  /^(flowchart|graph|sequenceDiagram|classDiagram|erDiagram|stateDiagram(-v2)?|quadrantChart|timeline|mindmap|pie|xychart-beta)\b/i;

/** Extracts the first fenced code block from `text`, or `null` if none is present (raw fallback). */
export function extractFencedBlock(text: string): FencedBlock | null {
  const match = /```([\w-]*)\n?([\s\S]*?)```/.exec(text);
  if (!match) return null;
  const [, lang, rawCode] = match;
  const code = (rawCode ?? "").trim();
  if (code.length === 0) return null;
  return { lang: (lang ?? "").trim().toLowerCase(), code };
}

export function isMermaidBlock(block: FencedBlock): boolean {
  return block.lang === "mermaid" || MERMAID_DIAGRAM_PATTERN.test(block.code);
}

let mermaidRenderCounter = 0;

const VisualPanel = ({
  isGenerating = false,
  sessionId,
  onGenerateDiagram,
}: VisualPanelProps) => {
  const { streamingBuffers, depthPrePrepared, currentQuestion } = useUIStore();
  const pushNotification = useUIStore((s) => s.pushNotification);
  const [copied, setCopied] = useState(false);
  const [requesting, setRequesting] = useState(false);
  const [svg, setSvg] = useState<string | null>(null);
  const [highlightedHtml, setHighlightedHtml] = useState<string | null>(null);
  const [renderFailed, setRenderFailed] = useState(false);
  const renderTokenRef = useRef(0);
  /** Questions already auto-retried once after a Mermaid render failure. */
  const autoRetriedQuestionsRef = useRef<Set<string>>(new Set());

  const text = streamingBuffers.visual;
  const block = useMemo(() => extractFencedBlock(text), [text]);

  useEffect(() => {
    const token = ++renderTokenRef.current;
    setSvg(null);
    setHighlightedHtml(null);
    setRenderFailed(false);

    if (!block) return;

    if (isMermaidBlock(block)) {
      const id = `visual-mermaid-${mermaidRenderCounter++}`;
      mermaid
        .render(id, block.code)
        .then(({ svg: rendered }) => {
          if (renderTokenRef.current === token) setSvg(rendered);
        })
        .catch(() => {
          if (renderTokenRef.current === token) setRenderFailed(true);
        });
    } else {
      void codeToHtml(block.code, { lang: block.lang || "text", theme: "github-dark" })
        .then((html) => {
          if (renderTokenRef.current === token) setHighlightedHtml(html);
        })
        .catch(() => {
          if (renderTokenRef.current === token) setRenderFailed(true);
        });
    }
  }, [block]);

  const canTriggerManually =
    sessionId != null &&
    onGenerateDiagram != null &&
    !isGenerating &&
    currentQuestion.trim().length > 0;

  // Silent one-shot retry when Mermaid fails to render — genuine model
  // mistakes only; ref-keyed so a second failure for the same question
  // falls through to the raw-text fallback without looping.
  useEffect(() => {
    if (!renderFailed || !block || !isMermaidBlock(block)) return;
    if (!canTriggerManually || !onGenerateDiagram) return;

    const retryKey = currentQuestion.trim();
    if (autoRetriedQuestionsRef.current.has(retryKey)) return;
    autoRetriedQuestionsRef.current.add(retryKey);

    setRequesting(true);
    void onGenerateDiagram(currentQuestion)
      .catch((err: unknown) => {
        pushNotification({
          id: crypto.randomUUID(),
          message: `Couldn't generate diagram: ${String(err)}`,
          level: "error",
        });
      })
      .finally(() => setRequesting(false));
  }, [
    renderFailed,
    block,
    canTriggerManually,
    onGenerateDiagram,
    currentQuestion,
    pushNotification,
  ]);

  const handleUseAnswer = () => {
    if (text.length === 0) return;
    void copyTextToClipboard(text)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      })
      .catch((err: unknown) => {
        pushNotification({
          id: crypto.randomUUID(),
          message: `Copy failed: ${String(err)}`,
          level: "error",
        });
      });
  };

  const handleGenerateDiagram = () => {
    if (!canTriggerManually || !onGenerateDiagram) return;
    setRequesting(true);
    void onGenerateDiagram(currentQuestion)
      .catch((err: unknown) => {
        pushNotification({
          id: crypto.randomUUID(),
          message: `Couldn't generate diagram: ${String(err)}`,
          level: "error",
        });
      })
      .finally(() => setRequesting(false));
  };

  // Suppress raw fallback while a silent auto-retry is in flight so the
  // existing "Generating visual response…" placeholder can surface instead.
  const showRawFallback =
    text.length > 0 && (!block || renderFailed) && !requesting;

  return (
    <div
      data-testid="visual-panel"
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
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "6px 12px",
          borderBottom: "1px solid #1e2028",
          flexShrink: 0,
          gap: 8,
        }}
      >
        <span
          style={{
            color: "#6b7280",
            fontSize: "11px",
            letterSpacing: "0.08em",
            textTransform: "uppercase",
          }}
        >
          Visual
        </span>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {depthPrePrepared && text.length > 0 && (
            <span
              style={{
                fontSize: "10px",
                color: "#a78bfa",
                fontWeight: 600,
                letterSpacing: "0.04em",
              }}
            >
              pre-prepared
            </span>
          )}
          <button
            type="button"
            data-testid="refine-diagram-button"
            disabled
            title="Refine diagram — coming soon (Tier 2b)"
            style={{
              padding: "3px 8px",
              fontSize: "10px",
              fontWeight: 600,
              borderRadius: 4,
              border: "1px solid #2d3748",
              backgroundColor: "transparent",
              color: "#4b5563",
              cursor: "not-allowed",
            }}
          >
            Refine diagram
          </button>
        </div>
      </div>

      <div
        style={{
          flex: 1,
          overflow: "auto",
          padding: "10px 12px",
          color: "#e5e7eb",
          lineHeight: "1.65",
        }}
      >
        {currentQuestion.length > 0 && (
          <QuestionHeading question={currentQuestion} />
        )}
        {text.length === 0 ? (
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <span
              style={{ color: "#4b5563", fontStyle: "italic", fontSize: "12px" }}
            >
              {isGenerating || requesting
                ? "Generating visual response…"
                : "Waiting for visual response…"}
            </span>
            {canTriggerManually && !requesting && (
              <button
                type="button"
                data-testid="generate-diagram-button"
                onClick={handleGenerateDiagram}
                title="Force a diagram for the current question, even if it wasn't flagged as visual"
                style={{
                  alignSelf: "flex-start",
                  padding: "4px 10px",
                  fontSize: "11px",
                  fontWeight: 600,
                  borderRadius: 4,
                  border: "1px solid #7c3aed",
                  backgroundColor: "transparent",
                  color: "#a78bfa",
                  cursor: "pointer",
                }}
              >
                Generate diagram
              </button>
            )}
          </div>
        ) : svg ? (
          <div data-testid="visual-mermaid-svg" dangerouslySetInnerHTML={{ __html: svg }} />
        ) : highlightedHtml ? (
          <div
            data-testid="visual-code-block"
            dangerouslySetInnerHTML={{ __html: highlightedHtml }}
          />
        ) : showRawFallback ? (
          <pre
            data-testid="visual-raw-fallback"
            style={{ margin: 0, whiteSpace: "pre-wrap", wordBreak: "break-word" }}
          >
            {text}
          </pre>
        ) : requesting || isGenerating ? (
          <span style={{ color: "#4b5563", fontStyle: "italic", fontSize: "12px" }}>
            Generating visual response…
          </span>
        ) : (
          <span style={{ color: "#4b5563", fontStyle: "italic", fontSize: "12px" }}>
            Rendering…
          </span>
        )}
      </div>

      {text.length > 0 && (
        <div
          style={{
            padding: "6px 12px",
            borderTop: "1px solid #1e2028",
            flexShrink: 0,
          }}
        >
          <button
            type="button"
            onClick={handleUseAnswer}
            title="Copy the full visual answer to your clipboard"
            style={{
              padding: "4px 10px",
              fontSize: "11px",
              fontWeight: 600,
              borderRadius: 4,
              border: "none",
              backgroundColor: "#7c3aed",
              color: "#fff",
              cursor: "pointer",
            }}
          >
            {copied ? "Copied!" : "Use This Answer"}
          </button>
        </div>
      )}
    </div>
  );
};

export default VisualPanel;
