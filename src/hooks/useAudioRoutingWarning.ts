import { useEffect } from "react";

import { onAudioRoutingWarning } from "../events";
import { useUIStore } from "../store/ui";

/**
 * Surface the once-per-session `audio_routing_warning` emitted by the Rust
 * pipeline when the system loopback is detected to be capturing the user's own
 * microphone. The hint is actionable (use headphones) so speaker separation can
 * recover for the remainder of the session.
 */
export function useAudioRoutingWarning(): void {
  const pushNotification = useUIStore((s) => s.pushNotification);

  useEffect(() => {
    let cancelled = false;
    let unsub: (() => void) | null = null;

    void onAudioRoutingWarning((payload) => {
      pushNotification({
        id: `audio-routing-${payload.kind}-${Date.now()}`,
        level: "warn",
        message: payload.message,
      });
    }).then((fn) => {
      if (cancelled) fn();
      else unsub = fn;
    });

    return () => {
      cancelled = true;
      unsub?.();
    };
  }, [pushNotification]);
}
