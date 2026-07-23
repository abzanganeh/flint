import { useCallback, useEffect, useState } from "react";

import DeepgramConsentModal from "../components/DeepgramConsentModal";
import {
  acceptDeepgramConsent,
  getDeepgramConsentStatus,
  getTranscriptionProviderPreference,
  setTranscriptionProviderPreference,
  type DeepgramReadinessDto,
  type TranscriptionProvider,
} from "../commands";
import { ProviderEntry, type ProviderRow } from "./ProviderSettings";

const DEEPGRAM_ROW: ProviderRow = {
  provider: "deepgram",
  label: "Deepgram API key",
  placeholder: "dg_…",
  helpUrl: "https://console.deepgram.com/",
  description:
    "Bring-your-own-key. Stored in your OS keychain — never uploaded or logged.",
};

interface TranscriptionSettingsProps {
  onBack?: () => void;
}

export default function TranscriptionSettings({ onBack }: TranscriptionSettingsProps) {
  const [preference, setPreference] = useState<TranscriptionProvider>("whisper");
  const [readiness, setReadiness] = useState<DeepgramReadinessDto>({
    consentAccepted: false,
    apiKeyPresent: false,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [savingPreference, setSavingPreference] = useState(false);
  const [showConsentModal, setShowConsentModal] = useState(false);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const [pref, ready] = await Promise.all([
        getTranscriptionProviderPreference(),
        getDeepgramConsentStatus(),
      ]);
      setPreference(pref);
      setReadiness(ready);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const persistPreference = async (next: TranscriptionProvider) => {
    setSavingPreference(true);
    setError(null);
    try {
      await setTranscriptionProviderPreference(next);
      setPreference(next);
    } catch (e) {
      setError(String(e));
      await refresh();
    } finally {
      setSavingPreference(false);
    }
  };

  const canEnableDeepgram = readiness.consentAccepted && readiness.apiKeyPresent;

  const handleSelectWhisper = () => {
    if (preference === "whisper" || savingPreference) return;
    void persistPreference("whisper");
  };

  const handleSelectDeepgram = () => {
    if (preference === "deepgram" || savingPreference) return;
    if (!readiness.consentAccepted) {
      setShowConsentModal(true);
      return;
    }
    if (!readiness.apiKeyPresent) {
      setError("Add a Deepgram API key below before enabling cloud transcription.");
      return;
    }
    void persistPreference("deepgram");
  };

  const handleConfirmConsent = async () => {
    setShowConsentModal(false);
    setError(null);
    try {
      await acceptDeepgramConsent();
      const nextReadiness = await getDeepgramConsentStatus();
      setReadiness(nextReadiness);
      if (nextReadiness.apiKeyPresent) {
        await persistPreference("deepgram");
      } else {
        setError(
          "Consent recorded. Add a Deepgram API key below to enable cloud transcription.",
        );
      }
    } catch (e) {
      setError(String(e));
    }
  };

  if (loading) {
    return (
      <div className="provider-settings">
        <div className="provider-settings__header">
          {onBack && (
            <button className="provider-settings__back-btn" onClick={onBack}>
              ← Back
            </button>
          )}
          <h2 className="provider-settings__title">Transcription</h2>
        </div>
        <p className="provider-settings__description">Loading…</p>
      </div>
    );
  }

  return (
    <div className="provider-settings">
      <div className="provider-settings__header">
        {onBack && (
          <button className="provider-settings__back-btn" onClick={onBack}>
            ← Back
          </button>
        )}
        <h2 className="provider-settings__title">Transcription</h2>
        <p className="provider-settings__subtitle">
          Choose how Flint turns speech into text. Whisper runs entirely on your
          device. Deepgram is a cloud service and requires explicit opt-in.
        </p>
      </div>

      {error && <p className="provider-settings__error">{error}</p>}

      <div className="provider-settings__list" data-testid="transcription-provider-choice">
        <label className="provider-settings__row" data-testid="transcription-whisper-option">
          <div className="provider-settings__row-header">
            <input
              type="radio"
              name="transcription-provider"
              value="whisper"
              checked={preference === "whisper"}
              onChange={handleSelectWhisper}
              disabled={savingPreference}
            />
            <span className="provider-settings__provider-label">
              Whisper (local, private)
            </span>
            {preference === "whisper" && (
              <span className="provider-settings__status provider-settings__status--set">
                Active
              </span>
            )}
          </div>
          <p className="provider-settings__description">
            Default. Runs on your device using whisper.cpp. No audio ever leaves
            your machine. Recommended for maximum privacy.
          </p>
        </label>

        <label
          className="provider-settings__row"
          data-testid="transcription-deepgram-option"
          aria-disabled={!canEnableDeepgram && preference !== "deepgram"}
        >
          <div className="provider-settings__row-header">
            <input
              type="radio"
              name="transcription-provider"
              value="deepgram"
              checked={preference === "deepgram"}
              onChange={handleSelectDeepgram}
              disabled={savingPreference}
              data-testid="transcription-deepgram-radio"
            />
            <span className="provider-settings__provider-label">
              Deepgram (cloud, faster on low-end hardware)
            </span>
            {preference === "deepgram" && (
              <span className="provider-settings__status provider-settings__status--set">
                Active
              </span>
            )}
            {!canEnableDeepgram && preference !== "deepgram" && (
              <span
                className="provider-settings__status provider-settings__status--missing"
                title={
                  !readiness.consentAccepted
                    ? "Accept the disclosure to enable"
                    : "Add a Deepgram API key below to enable"
                }
              >
                {!readiness.consentAccepted ? "Disclosure required" : "Key required"}
              </span>
            )}
          </div>
          <p className="provider-settings__description">
            Sends live conversation audio to Deepgram's servers. Whisper is used
            automatically as a fallback if Deepgram is unreachable. Requires
            accepting a disclosure and adding your own Deepgram API key.
          </p>
        </label>
      </div>

      <h3 className="provider-settings__section-title">Deepgram API key</h3>
      <div className="provider-settings__list">
        <ProviderEntry row={DEEPGRAM_ROW} onKeyChanged={() => void refresh()} />
      </div>

      {showConsentModal && (
        <DeepgramConsentModal
          onConfirm={() => void handleConfirmConsent()}
          onCancel={() => setShowConsentModal(false)}
        />
      )}
    </div>
  );
}
