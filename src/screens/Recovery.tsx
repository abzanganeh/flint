import React, { useCallback } from "react";
import type { RecoveryOffer } from "../commands";
import {
  discardCrashedSession,
  resumeCrashedSession,
} from "../commands";

interface Props {
  offer: RecoveryOffer;
  onResume: () => void;
  onDiscard: () => void;
}

export const Recovery: React.FC<Props> = ({ offer, onResume, onDiscard }) => {
  const [loading, setLoading] = React.useState<"resume" | "discard" | null>(null);
  const [error, setError] = React.useState<string | null>(null);

  const handleResume = useCallback(async () => {
    setLoading("resume");
    setError(null);
    try {
      await resumeCrashedSession();
      onResume();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(null);
    }
  }, [onResume]);

  const handleDiscard = useCallback(async () => {
    setLoading("discard");
    setError(null);
    try {
      await discardCrashedSession();
      onDiscard();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(null);
    }
  }, [onDiscard]);

  const stateLabel = offer.interruptedState
    .replace(/_/g, " ")
    .toLowerCase()
    .replace(/^./, (c) => c.toUpperCase());

  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-neutral-950 text-white px-6">
      <div className="max-w-md w-full bg-neutral-900 border border-amber-500/40 rounded-xl p-8 shadow-2xl">
        <div className="flex items-center gap-3 mb-6">
          <div className="w-3 h-3 rounded-full bg-amber-400 animate-pulse" />
          <h1 className="text-lg font-semibold text-amber-400">
            Session Recovery
          </h1>
        </div>

        <p className="text-neutral-300 text-sm mb-6 leading-relaxed">
          A previous session was interrupted while in{" "}
          <span className="text-white font-medium">{stateLabel}</span> state.
          Would you like to resume it?
        </p>

        <div className="grid grid-cols-3 gap-3 mb-6 text-center">
          <div className="bg-neutral-800 rounded-lg py-3 px-2">
            <p className="text-2xl font-bold text-white">
              {offer.transcriptChunkCount}
            </p>
            <p className="text-xs text-neutral-400 mt-1">Transcript chunks</p>
          </div>
          <div className="bg-neutral-800 rounded-lg py-3 px-2">
            <p className="text-2xl font-bold text-white">
              {offer.responseCount}
            </p>
            <p className="text-xs text-neutral-400 mt-1">AI responses</p>
          </div>
          <div className="bg-neutral-800 rounded-lg py-3 px-2">
            <p className="text-xs font-medium text-amber-300 mt-2">
              {stateLabel}
            </p>
            <p className="text-xs text-neutral-400 mt-1">State</p>
          </div>
        </div>

        {error && (
          <p className="text-red-400 text-xs mb-4 bg-red-900/20 rounded-lg px-3 py-2">
            {error}
          </p>
        )}

        <div className="flex gap-3">
          <button
            onClick={handleResume}
            disabled={loading !== null}
            className="flex-1 bg-amber-500 hover:bg-amber-400 disabled:opacity-50 disabled:cursor-not-allowed text-black font-semibold rounded-lg py-2.5 text-sm transition-colors"
          >
            {loading === "resume" ? "Resuming…" : "Resume Session"}
          </button>
          <button
            onClick={handleDiscard}
            disabled={loading !== null}
            className="flex-1 bg-neutral-700 hover:bg-neutral-600 disabled:opacity-50 disabled:cursor-not-allowed text-white font-medium rounded-lg py-2.5 text-sm transition-colors"
          >
            {loading === "discard" ? "Discarding…" : "Discard"}
          </button>
        </div>

        <p className="text-neutral-500 text-xs mt-4 text-center">
          Discarding permanently deletes all local data for this session.
        </p>
      </div>
    </div>
  );
};
