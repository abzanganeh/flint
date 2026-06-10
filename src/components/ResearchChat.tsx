import { useEffect, useRef, useState } from "react";

import {
  appendResearchToContext,
  runResearchChat,
  type WebSource,
} from "../commands";
import { onResearchCitation, onResearchToken } from "../events";

type ResearchSource = "rag" | "web" | "rag_and_web" | "none";

interface Message {
  role: "user" | "assistant";
  text: string;
  question?: string;
  citations?: string[];
  webSources?: WebSource[];
  source?: ResearchSource;
  canAddToContext?: boolean;
  addedToContext?: boolean;
  addingToContext?: boolean;
}

interface ResearchChatProps {
  sessionId: string;
}

const SOURCE_LABELS: Record<ResearchSource, string> = {
  rag: "From your pasted context",
  web: "From web search",
  rag_and_web: "Context + web search",
  none: "No sources",
};

export default function ResearchChat({ sessionId }: ResearchChatProps) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const pendingQuestionRef = useRef<string>("");

  useEffect(() => {
    let unlistenToken: (() => void) | null = null;
    let unlistenCitation: (() => void) | null = null;
    let cancelled = false;

    const setup = async () => {
      const [fnToken, fnCitation] = await Promise.all([
        onResearchToken(({ token }) => {
          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.role === "assistant") {
              return [
                ...prev.slice(0, -1),
                { ...last, text: last.text + token },
              ];
            }
            return [
              ...prev,
              {
                role: "assistant",
                text: token,
                question: pendingQuestionRef.current,
              },
            ];
          });
        }),
        onResearchCitation(({ chunks, webSources, source, canAddToContext }) => {
          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.role === "assistant") {
              return [
                ...prev.slice(0, -1),
                {
                  ...last,
                  citations: chunks,
                  webSources: webSources ?? [],
                  source: source ?? "none",
                  canAddToContext: canAddToContext ?? false,
                },
              ];
            }
            return prev;
          });
          setAsking(false);
        }),
      ]);

      if (cancelled) {
        fnToken();
        fnCitation();
      } else {
        unlistenToken = fnToken;
        unlistenCitation = fnCitation;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlistenToken?.();
      unlistenCitation?.();
    };
  }, []);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSend = async () => {
    const trimmed = input.trim();
    if (!trimmed || asking) return;

    pendingQuestionRef.current = trimmed;
    setMessages((prev) => [...prev, { role: "user", text: trimmed }]);
    setInput("");
    setError(null);
    setAsking(true);

    try {
      await runResearchChat(sessionId, trimmed);
    } catch (e) {
      setError(String(e));
      setAsking(false);
    }
  };

  const handleAddToContext = async (index: number) => {
    const msg = messages[index];
    if (
      !msg ||
      msg.role !== "assistant" ||
      !msg.canAddToContext ||
      !msg.question?.trim() ||
      !msg.text.trim()
    ) {
      return;
    }

    setMessages((prev) =>
      prev.map((m, i) => (i === index ? { ...m, addingToContext: true } : m)),
    );
    setError(null);

    try {
      const result = await appendResearchToContext(
        sessionId,
        msg.question,
        msg.text,
        msg.webSources ?? [],
      );
      setMessages((prev) =>
        prev.map((m, i) =>
          i === index
            ? {
                ...m,
                addingToContext: false,
                addedToContext: true,
                text:
                  result.chunksAdded > 0
                    ? `${m.text}\n\n(Added ${result.chunksAdded} chunk(s) to Technical Prep.)`
                    : m.text,
              }
            : m,
        ),
      );
    } catch (e) {
      setError(String(e));
      setMessages((prev) =>
        prev.map((m, i) => (i === index ? { ...m, addingToContext: false } : m)),
      );
    }
  };

  return (
    <div className="research-chat">
      <div className="research-chat__header">
        <span className="research-chat__title">Research Chat</span>
        <span className="research-chat__hint">RAG first, then web (Tavily)</span>
      </div>

      <div className="research-chat__messages">
        {messages.length === 0 && (
          <p className="research-chat__empty">
            Ask about your meeting criteria, technical topics, or company research.
            Flint checks your pasted context first; if it&apos;s not there, it searches
            the web (requires Tavily key in Settings). Save useful answers into
            Technical Prep with &quot;Add to session context&quot;.
          </p>
        )}
        {messages.map((msg, i) => (
          <div key={i} className={`research-chat__message research-chat__message--${msg.role}`}>
            {msg.role === "assistant" && msg.source && (
              <span className="research-chat__source-badge">
                {SOURCE_LABELS[msg.source]}
              </span>
            )}
            <p className="research-chat__message-text">{msg.text}</p>
            {msg.webSources && msg.webSources.length > 0 && (
              <details className="research-chat__web-sources">
                <summary>Web sources ({msg.webSources.length})</summary>
                <ul>
                  {msg.webSources.map((s, si) => (
                    <li key={si}>
                      <a href={s.url} target="_blank" rel="noopener noreferrer">
                        {s.title || s.url}
                      </a>
                      {s.snippet && (
                        <p className="research-chat__web-snippet">
                          {s.snippet.length > 160 ? `${s.snippet.slice(0, 160)}…` : s.snippet}
                        </p>
                      )}
                    </li>
                  ))}
                </ul>
              </details>
            )}
            {msg.citations && msg.citations.length > 0 && (
              <details className="research-chat__citations">
                <summary>Context chunks ({msg.citations.length})</summary>
                <ul>
                  {msg.citations.map((c, ci) => (
                    <li key={ci} className="research-chat__citation">
                      {c.length > 200 ? `${c.slice(0, 200)}…` : c}
                    </li>
                  ))}
                </ul>
              </details>
            )}
            {msg.role === "assistant" && msg.canAddToContext && !msg.addedToContext && (
              <button
                type="button"
                className="research-chat__add-btn"
                disabled={msg.addingToContext}
                onClick={() => void handleAddToContext(i)}
              >
                {msg.addingToContext ? "Adding…" : "Add to session context"}
              </button>
            )}
            {msg.addedToContext && (
              <span className="research-chat__added-label">Saved to Technical Prep</span>
            )}
          </div>
        ))}
        {asking && (
          <div className="research-chat__message research-chat__message--assistant research-chat__message--thinking">
            …
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {error && <div className="research-chat__error">{error}</div>}

      <div className="research-chat__input-row">
        <textarea
          className="research-chat__input"
          rows={2}
          placeholder="Ask about a topic, company, or technical concept…"
          value={input}
          disabled={asking}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void handleSend();
            }
          }}
        />
        <button
          className="research-chat__send-btn"
          onClick={() => void handleSend()}
          disabled={asking || !input.trim()}
        >
          Send
        </button>
      </div>
    </div>
  );
}
