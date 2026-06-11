import { useCallback, useEffect, useRef, useState } from "react";

import {
  deleteAccount,
  exportUserData,
  getCostStatus,
  liftCostSuspension,
  resetCostTracker,
  setCostCap,
  type CostStatusDto,
  type DeleteAccountReport,
} from "../commands";
import ProviderSettings from "./ProviderSettings";

type Tab = "api-keys" | "usage-cap" | "privacy";

interface Props {
  onBack?: () => void;
  initialTab?: Tab;
}

// ── Cost Cap Tab ──────────────────────────────────────────────────────────────

function CostCapTab() {
  const [status, setStatus] = useState<CostStatusDto | null>(null);
  const [tokenInput, setTokenInput] = useState("");
  const [usdInput, setUsdInput] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [successMsg, setSuccessMsg] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const s = await getCostStatus();
      setStatus(s);
      if (s.maxTotalTokens != null) setTokenInput(String(s.maxTotalTokens));
      if (s.maxCostEstimateUsd != null) setUsdInput(String(s.maxCostEstimateUsd));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    setSuccessMsg(null);
    try {
      const tokens = tokenInput.trim() ? parseInt(tokenInput.trim(), 10) : null;
      const usd = usdInput.trim() ? parseFloat(usdInput.trim()) : null;
      const updated = await setCostCap(tokens, usd);
      setStatus(updated);
      setSuccessMsg("Limits saved.");
      setTimeout(() => setSuccessMsg(null), 2000);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleLiftSuspension = async () => {
    try {
      const updated = await liftCostSuspension();
      setStatus(updated);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleReset = async () => {
    try {
      const updated = await resetCostTracker();
      setStatus(updated);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="settings-tab">
      <h3 className="settings-tab__heading">Usage limits</h3>
      <p className="settings-tab__description">
        These are spending ceilings — not targets. Each rehearsal question
        uses ~300+ estimated tokens (3 LLM calls). Token limit must be at
        least 500 if set. Leave blank to remove that dimension.
      </p>

      {status?.suspended && (
        <div className="settings-tab__alert settings-tab__alert--warn" role="alert">
          Inference is suspended — limit reached.
          <button
            className="settings-tab__inline-btn"
            onClick={() => void handleLiftSuspension()}
          >
            Resume
          </button>
        </div>
      )}

      {error && (
        <p className="settings-tab__error" role="alert">
          {error}
        </p>
      )}
      {successMsg && <p className="settings-tab__success">{successMsg}</p>}

      <div className="settings-tab__field-group">
        <label className="settings-tab__label">
          Token limit
          <input
            className="settings-tab__input"
            type="number"
            min={0}
            placeholder="e.g. 100000"
            value={tokenInput}
            onChange={(e) => setTokenInput(e.target.value)}
          />
        </label>
        <label className="settings-tab__label">
          Cost limit (USD)
          <input
            className="settings-tab__input"
            type="number"
            min={0}
            step={0.01}
            placeholder="e.g. 2.00"
            value={usdInput}
            onChange={(e) => setUsdInput(e.target.value)}
          />
        </label>
      </div>

      {status && (
        <p className="settings-tab__status-line">
          Used: {status.totalTokens.toLocaleString()} tokens /{" "}
          {status.costEstimateUsd != null
            ? `$${status.costEstimateUsd.toFixed(4)}`
            : "—"}
        </p>
      )}

      <div className="settings-tab__actions">
        <button
          className="settings-tab__btn settings-tab__btn--primary"
          disabled={saving}
          onClick={() => void handleSave()}
        >
          {saving ? "Saving…" : "Save limits"}
        </button>
        <button
          className="settings-tab__btn"
          onClick={() => void handleReset()}
          title="Zero the cumulative counters (does not change the cap)"
        >
          Reset counters
        </button>
      </div>
    </div>
  );
}

// ── Privacy Tab ───────────────────────────────────────────────────────────────

function PrivacyTab() {
  const [report, setReport] = useState<DeleteAccountReport | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportDone, setExportDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLInputElement>(null);

  const handleExport = async () => {
    setExporting(true);
    setError(null);
    try {
      const json = await exportUserData();
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `flint-export-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      setExportDone(true);
      setTimeout(() => setExportDone(false), 3000);
    } catch (e) {
      setError(String(e));
    } finally {
      setExporting(false);
    }
  };

  const handleDelete = async () => {
    if (confirmRef.current?.value !== "DELETE") {
      setError('Type DELETE in the confirmation field first.');
      return;
    }
    setDeleting(true);
    setError(null);
    try {
      const r = await deleteAccount();
      setReport(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setDeleting(false);
    }
  };

  if (report) {
    const issues = [
      !report.supabaseDeleted && report.supabaseError && `Supabase: ${report.supabaseError}`,
      !report.keychainCleared && report.keychainError && `Keychain: ${report.keychainError}`,
      !report.vectorStoreCleared && report.vectorStoreError && `Vector store: ${report.vectorStoreError}`,
      !report.sqliteCleared && report.sqliteError && `SQLite: ${report.sqliteError}`,
    ].filter(Boolean) as string[];

    return (
      <div className="settings-tab">
        <h3 className="settings-tab__heading">Account deleted</h3>
        <p className="settings-tab__description">
          {report.sessionsCleared} session{report.sessionsCleared !== 1 ? "s" : ""} cleared.
        </p>
        {issues.length > 0 && (
          <div className="settings-tab__alert settings-tab__alert--warn">
            <p>Some steps failed — you may need to clean up manually:</p>
            <ul>
              {issues.map((i) => (
                <li key={i}>{i}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="settings-tab">
      <h3 className="settings-tab__heading">Your data</h3>

      {error && (
        <p className="settings-tab__error" role="alert">
          {error}
        </p>
      )}

      <section className="settings-tab__section">
        <h4 className="settings-tab__subheading">Export</h4>
        <p className="settings-tab__description">
          Download all your sessions, transcripts, and responses as JSON.
        </p>
        <button
          className="settings-tab__btn settings-tab__btn--primary"
          disabled={exporting}
          onClick={() => void handleExport()}
        >
          {exporting ? "Exporting…" : exportDone ? "Downloaded" : "Export my data"}
        </button>
      </section>

      <section className="settings-tab__section settings-tab__section--danger">
        <h4 className="settings-tab__subheading settings-tab__subheading--danger">
          Delete account
        </h4>
        <p className="settings-tab__description">
          Permanently deletes your account and all session data. API keys in your
          OS keychain are kept so you do not need to re-enter them. This cannot be
          undone.
        </p>
        <label className="settings-tab__label">
          Type <strong>DELETE</strong> to confirm
          <input
            ref={confirmRef}
            className="settings-tab__input"
            type="text"
            placeholder="DELETE"
          />
        </label>
        <button
          className="settings-tab__btn settings-tab__btn--danger"
          disabled={deleting}
          onClick={() => void handleDelete()}
        >
          {deleting ? "Deleting…" : "Delete my account"}
        </button>
      </section>
    </div>
  );
}

// ── Settings screen ───────────────────────────────────────────────────────────

const TAB_LABELS: Record<Tab, string> = {
  "api-keys": "API Keys",
  "usage-cap": "Usage Cap",
  privacy: "Privacy",
};

export default function Settings({ onBack, initialTab = "api-keys" }: Props) {
  const [activeTab, setActiveTab] = useState<Tab>(initialTab);

  return (
    <div className="settings-screen">
      <div className="settings-screen__header">
        {onBack && (
          <button className="settings-screen__back-btn" onClick={onBack}>
            ← Back
          </button>
        )}
        <h2 className="settings-screen__title">Settings</h2>
      </div>

      <nav className="settings-screen__tabs" role="tablist">
        {(Object.keys(TAB_LABELS) as Tab[]).map((tab) => (
          <button
            key={tab}
            role="tab"
            aria-selected={activeTab === tab}
            className={`settings-screen__tab${activeTab === tab ? " settings-screen__tab--active" : ""}`}
            onClick={() => setActiveTab(tab)}
          >
            {TAB_LABELS[tab]}
          </button>
        ))}
      </nav>

      <div className="settings-screen__panel" role="tabpanel">
        {activeTab === "api-keys" && <ProviderSettings />}
        {activeTab === "usage-cap" && <CostCapTab />}
        {activeTab === "privacy" && <PrivacyTab />}
      </div>
    </div>
  );
}
