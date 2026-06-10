import { useEffect, useRef, useState } from "react";

import { runResearchChat } from "../commands";
import { onResearchCitation, onResearchToken } from "../events";

interface Message {
  role: "user" | "assistant";
  text: string;
  citations?: string[];
}

interface ResearchChatProps {
  sessionId: string;
}

export default function ResearchChat({ sessionId }: ResearchChatProps) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

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
            return [...prev, { role: "assistant", text: token }];
          });
        }),
        onResearchCitation(({ chunks }) => {
          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.role === "assistant") {
              return [...prev.slice(0, -1), { ...last, citations: chunks }];
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

  return (
    <div className="research-chat">
      <div className="research-chat__header">
        <span className="research-chat__title">Research Chat</span>
        <span className="research-chat__hint">RAG-only — no live web</span>
      </div>

      <div className="research-chat__messages">
        {messages.length === 0 && (
          <p className="research-chat__empty">
            Ask anything about your pasted context. Answers are grounded in what you
            submitted — Flint will tell you if the information isn&apos;t there.
          </p>
        )}
        {messages.map((msg, i) => (
          <div key={i} className={`research-chat__message research-chat__message--${msg.role}`}>
            <p className="research-chat__message-text">{msg.text}</p>
            {msg.citations && msg.citations.length > 0 && (
              <details className="research-chat__citations">
                <summary>Sources ({msg.citations.length})</summary>
                <ul>
                  {msg.citations.map((c, ci) => (
                    <li key={ci} className="research-chat__citation">
                      {c.length > 200 ? `${c.slice(0, 200)}…` : c}
                    </li>
                  ))}
                </ul>
              </details>
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
          placeholder="Ask about your context…"
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
