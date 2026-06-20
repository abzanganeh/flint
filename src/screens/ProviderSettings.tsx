import { useCallback, useEffect, useState } from "react";

import {
  clearProviderKey,
  getConfiguredProviders,
  getProviderPriority,
  isProviderKeyPresent,
  saveProviderKey,
  setProviderPriority,
  type ApiKeyProvider,
  type ConfiguredProviderDto,
} from "../commands";

interface ProviderRow {
  provider: ApiKeyProvider;
  label: string;
  placeholder: string;
  helpUrl: string;
  description?: string;
}


const PROVIDER_LABELS: Record<string, string> = {
  groq: "Groq",
  deepseek: "DeepSeek",
  openai: "OpenAI",
  anthropic: "Anthropic",
  openrouter: "OpenRouter",
  ollama: "Ollama (local)",
};

const SLOT_LABELS = ["Default", "Fallback 1", "Fallback 2", "Fallback 3", "Fallback 4"];

function providerLabel(name: string): string {
  return PROVIDER_LABELS[name] ?? name;
}

const LLM_PROVIDERS: ProviderRow[] = [
  {
    provider: "groq",
    label: "Groq",
    placeholder: "gsk_…",
    helpUrl: "https://console.groq.com/keys",
    description: "Default primary LLM for rehearsal and live sessions.",
  },
  {
    provider: "deepseek",
    label: "DeepSeek",
    placeholder: "sk-…",
    helpUrl: "https://platform.deepseek.com/api_keys",
    description: "Cloud fallback tier #1 when primary is unavailable.",
  },
  {
    provider: "openrouter",
    label: "OpenRouter (fallback)",
    placeholder: "sk-or-…",
    helpUrl: "https://openrouter.ai/keys",
    description: "Cloud fallback tier #2 — multi-model gateway.",
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
  onKeyChanged?: () => void;
}

function ProviderEntry({ row, onKeyChanged }: ProviderEntryProps) {
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
      onKeyChanged?.();
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
      onKeyChanged?.();
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

function ProviderPrioritySection() {
  const [order, setOrder] = useState<string[]>([]);
  const [configured, setConfigured] = useState<ConfiguredProviderDto[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const refresh = useCallback(async () => {
    const [priority, providers] = await Promise.all([
      getProviderPriority(),
      getConfiguredProviders(),
    ]);
    setConfigured(providers);
    const keyed = priority.filter((name) => {
      const row = providers.find((p) => p.name === name);
      return row?.hasKey;
    });
    setOrder(keyed);
  }, []);

  useEffect(() => {
    void refresh().catch((e: unknown) => setError(String(e)));
  }, [refresh]);

  const persistOrder = async (next: string[]) => {
    setSaving(true);
    setError(null);
    try {
      const fullPriority = await getProviderPriority();
      const unconfigured = fullPriority.filter((name) => !next.includes(name));
      await setProviderPriority([...next, ...unconfigured]);
      setOrder(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const move = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    if (target < 0 || target >= order.length) return;
    const next = [...order];
    [next[index], next[target]] = [next[target], next[index]];
    void persistOrder(next);
  };

  const ollama = configured.find((p) => p.name === "ollama");

  return (
    <div className="provider-settings__primary">
      <h3 className="provider-settings__section-title">LLM Providers</h3>
      <p className="provider-settings__subtitle">
        Reorder configured cloud providers. Flint tries them top-to-bottom; Ollama is always the
        local last resort and cannot be moved.
      </p>
      {error && <p className="provider-settings__error">{error}</p>}
      {order.length === 0 ? (
        <p className="provider-settings__description">Add at least one cloud API key below.</p>
      ) : (
        <ul className="provider-priority__list">
          {order.map((name, index) => {
            const row = configured.find((p) => p.name === name);
            return (
              <li key={name} className="provider-priority__row">
                <span className="provider-priority__slot">{SLOT_LABELS[index] ?? `Fallback ${index}`}</span>
                <span className="provider-priority__name">{providerLabel(name)}</span>
                <span
                  className={`provider-priority__status ${
                    row?.isReachable
                      ? "provider-priority__status--ok"
                      : "provider-priority__status--warn"
                  }`}
                >
                  {row?.isReachable ? "Reachable" : "Key set"}
                </span>
                <div className="provider-priority__actions">
                  <button
                    type="button"
                    className="provider-settings__btn"
                    disabled={index === 0 || saving}
                    aria-label={`Move ${providerLabel(name)} up`}
                    onClick={() => move(index, -1)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="provider-settings__btn"
                    disabled={index === order.length - 1 || saving}
                    aria-label={`Move ${providerLabel(name)} down`}
                    onClick={() => move(index, 1)}
                  >
                    ↓
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {ollama && (
        <div className="provider-priority__ollama">
          <span className="provider-priority__slot">Local</span>
          <span className="provider-priority__name">{providerLabel("ollama")}</span>
          <span
            className={`provider-priority__status ${
              ollama.isReachable
                ? "provider-priority__status--ok"
                : "provider-priority__status--warn"
            }`}
          >
            {ollama.isReachable ? "Running" : "Not detected"}
          </span>
        </div>
      )}
    </div>
  );
}

interface ProviderSettingsProps {
  onBack?: () => void;
}

export default function ProviderSettings({ onBack }: ProviderSettingsProps) {
  const [, setKeysVersion] = useState(0);

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
          At least one cloud LLM key is required. Tavily enables web research during rehearsal prep.
        </p>
      </div>

      <ProviderPrioritySection />

      <h3 className="provider-settings__section-title">LLM providers</h3>
      <div className="provider-settings__list">
        {LLM_PROVIDERS.map((row) => (
          <ProviderEntry
            key={row.provider}
            row={row}
            onKeyChanged={() => setKeysVersion((v) => v + 1)}
          />
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
