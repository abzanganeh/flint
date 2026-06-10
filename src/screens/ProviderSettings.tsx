import { useCallback, useEffect, useState } from "react";

import {
  clearProviderKey,
  isProviderKeyPresent,
  saveProviderKey,
  type LlmProvider,
} from "../commands";

interface ProviderRow {
  provider: LlmProvider;
  label: string;
  placeholder: string;
  helpUrl: string;
}

const PROVIDERS: ProviderRow[] = [
  {
    provider: "groq",
    label: "Groq",
    placeholder: "gsk_…",
    helpUrl: "https://console.groq.com/keys",
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

      {error && <p className="provider-settings__error">{error}</p>}
      {success && <p className="provider-settings__success">Saved.</p>}

      {!editing ? (
        <div className="provider-settings__actions">
          <button
            className="provider-settings__btn provider-settings__btn--edit"
            onClick={() => setEditing(true)}
          >
            {keyPresent ? "Replace key" : "Add key"}
          </button>
          {keyPresent && (
            <button
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
          Groq is required for digest extraction and research chat.
        </p>
      </div>

      <div className="provider-settings__list">
        {PROVIDERS.map((row) => (
          <ProviderEntry key={row.provider} row={row} />
        ))}
      </div>
    </div>
  );
}
