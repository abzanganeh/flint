import { useState } from "react";

import { resolveSmartResumeImportInput } from "../lib/smartResumeImport";
import "./SmartResumeLinkImport.css";

interface Props {
  disabled?: boolean;
  onImport: (token: string) => void;
}

export default function SmartResumeLinkImport({ disabled = false, onImport }: Props) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const handleImport = () => {
    const token = resolveSmartResumeImportInput(value);
    if (!token) {
      setError("Paste a flint://import link or the token from Smart Resume.");
      return;
    }
    setError(null);
    onImport(token);
  };

  return (
    <details className="sr-link-import" open>
      <summary className="sr-link-import-summary">
        Import from Smart Resume link
      </summary>
      <div className="sr-link-import-body">
        <p className="sr-link-import-hint">
          On Linux dev builds, deep links may not open Flint automatically. Copy the
          link from Smart Resume and paste it here.
        </p>
        <input
          type="text"
          className="sr-link-import-input"
          placeholder="flint://import?token=…"
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            if (error) setError(null);
          }}
          disabled={disabled}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              handleImport();
            }
          }}
        />
        {error && (
          <p className="sr-link-import-error" role="alert">
            {error}
          </p>
        )}
        <button
          type="button"
          className="sr-link-import-btn"
          onClick={handleImport}
          disabled={disabled || !value.trim()}
        >
          Import
        </button>
      </div>
    </details>
  );
}
