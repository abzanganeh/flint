import { useCallback, useEffect, useState } from "react";

import {
  clearProviderKey,
  isProviderKeyPresent,
  saveProviderKey,
  type ApiKeyProvider,
} from "../commands";

interface ProviderRow {
  provider: ApiKeyProvider;
  label: string;
  placeholder: string;
  helpUrl: string;
  description?: string;
}

const LLM_PROVIDERS: ProviderRow[] = [
  {
    provider: "groq",
    label: "Groq",
    placeholder: "gsk_…",
    helpUrl: "https://console.groq.com/keys",
    description: "Primary LLM for rehearsal answers and digest extraction.",
  },
  {
    provider: "openrouter",
    label: "OpenRouter (fallback)",
    placeholder: "sk-or-…",
    helpUrl: "https://openrouter.ai/keys",
    description:
      "Used automatically when Groq is rate-limited or unavailable. Same Llama 3.3 70B model.",
  },
  {
    provider: "openai",
    label: "OpenAI",
    placeholder: "sk-…",
    helpUrl: "https://platform.openai.com/api-keys",
  },
  {
    provider: "anthropic",
    label: "Anthropic",
    placeholder: "sk-ant-…",
    helpUrl: "https://console.anthropic.com/settings/keys",
  },
];

const WEB_PROVIDERS: ProviderRow[] = [
  {
    provider: "tavily",
    label: "Tavily (web search)",
    placeholder: "tvly-…",
    helpUrl: "https://tavily.com/",
    description: "Enables web research during rehearsal when your pasted context is insufficient.",
  },
];

interface ProviderEntryProps {
  row: ProviderRow;
}

function ProviderEntry({ row }: ProviderEntryProps) {
  const [keyPresent, setKeyPresent] = useState<boolean | null>(null);
  const [input, setInput] = useState("");
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const present = await isProviderKeyPresent(row.provider);
      setKeyPresent(present);
    } catch {
      setKeyPresent(false);
    }
  }, [row.provider]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleSave = async () => {
    const trimmed = input.trim();
    if (!trimmed) return;
    setSaving(true);
    setError(null);
    setSuccess(false);
    try {
      await saveProviderKey(row.provider, trimmed);
      setInput("");
      setEditing(false);
      setSuccess(true);
      setKeyPresent(true);
      setTimeout(() => setSuccess(false), 2000);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleClear = async () => {
    try {
      await clearProviderKey(row.provider);
      setKeyPresent(false);
      setEditing(false);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="provider-settings__row">
      <div className="provider-settings__row-header">
        <span className="provider-settings__provider-label">{row.label}</span>
        {keyPresent === true && (
          <span className="provider-settings__status provider-settings__status--set">
            Key stored
          </span>
        )}
        {keyPresent === false && (
          <span className="provider-settings__status provider-settings__status--missing">
            Not set
          </span>
        )}
      </div>

      {row.description && (
        <p className="provider-settings__description">{row.description}</p>
      )}

      {error && <p className="provider-settings__error">{error}</p>}
      {success && <p className="provider-settings__success">Saved.</p>}

      {!editing ? (
        <div className="provider-settings__actions">
          <button
            type="button"
            className="provider-settings__btn provider-settings__btn--edit"
            onClick={() => setEditing(true)}
          >
            {keyPresent ? "Replace key" : "Add key"}
          </button>
          {keyPresent && (
            <button
              type="button"
              className="provider-settings__btn provider-settings__btn--clear"
              onClick={() => void handleClear()}
            >
              Remove
            </button>
          )}
          <a
            href={row.helpUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="provider-settings__help-link"
          >
            Get key ↗
          </a>
        </div>
      ) : (
        <div className="provider-settings__edit">
          <input
            className="provider-settings__key-input"
            type="password"
            autoComplete="off"
            placeholder={row.placeholder}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void handleSave();
              if (e.key === "Escape") setEditing(false);
            }}
            autoFocus
          />
          <button
            className="provider-settings__btn provider-settings__btn--save"
            disabled={!input.trim() || saving}
            onClick={() => void handleSave()}
          >
            {saving ? "Saving…" : "Save"}
          </button>
          <button
            className="provider-settings__btn provider-settings__btn--cancel"
            type="button"
            onClick={() => setEditing(false)}
          >
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}

interface ProviderSettingsProps {
  onBack?: () => void;
}

export default function ProviderSettings({ onBack }: ProviderSettingsProps) {
  return (
    <div className="provider-settings">
      <div className="provider-settings__header">
        {onBack && (
          <button className="provider-settings__back-btn" onClick={onBack}>
            ← Back
          </button>
        )}
        <h2 className="provider-settings__title">API Keys</h2>
        <p className="provider-settings__subtitle">
          Keys are stored in your OS keychain — never in plain text or uploaded anywhere.
          Groq is required for digest extraction. OpenRouter is used as fallback when Groq
          is rate-limited. Tavily enables web research during rehearsal prep.
        </p>
      </div>

      <h3 className="provider-settings__section-title">LLM providers</h3>
      <div className="provider-settings__list">
        {LLM_PROVIDERS.map((row) => (
          <ProviderEntry key={row.provider} row={row} />
        ))}
      </div>

      <h3 className="provider-settings__section-title">Web search</h3>
      <div className="provider-settings__list">
        {WEB_PROVIDERS.map((row) => (
          <ProviderEntry key={row.provider} row={row} />
        ))}
      </div>
    </div>
  );
}
