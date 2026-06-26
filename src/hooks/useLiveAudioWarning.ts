import { useEffect, useState } from "react";

import { onLiveAudioWarning } from "../events";

/**
 * Track the live audio-flow watchdog. Returns the active "no audio captured"
 * message, or `null` when audio is flowing. Unlike a transient toast this is
 * surfaced as a persistent banner because a silent capture means the entire
 * session is being lost — the single most damaging live-session failure.
 */
export function useLiveAudioWarning(): string | null {
  const [warning, setWarning] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unsub: (() => void) | null = null;

    void onLiveAudioWarning((payload) => {
      setWarning(payload.kind === "no_audio" ? payload.message : null);
    }).then((fn) => {
      if (cancelled) fn();
      else unsub = fn;
    });

    return () => {
      cancelled = true;
      unsub?.();
    };
  }, []);

  return warning;
}
