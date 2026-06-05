import { useEffect, useState } from "react";

import { isFeatureEnabled } from "../commands";

/**
 * React hook that resolves a single feature flag for the current user.
 *
 * The backend call is non-blocking (reads from the in-memory cache that
 * was loaded at startup) so the hook resolves on first mount within a
 * single event-loop tick. Until the first response lands, the hook
 * returns the supplied `fallback` (default: `false`) — the UI should
 * prefer the "off" state during that gap rather than flashing the gated
 * feature on and then hiding it.
 *
 * Re-checks the flag whenever `flag` changes. Callers that need to react
 * to remote refresh events should call `refreshFeatureFlags()` and then
 * remount the consuming subtree (the simplest correct approach for a UI
 * decision that rarely changes mid-session).
 */
export function useFeatureFlag(flag: string, fallback = false): boolean {
  const [enabled, setEnabled] = useState<boolean>(fallback);

  useEffect(() => {
    let cancelled = false;
    isFeatureEnabled(flag)
      .then((value) => {
        if (!cancelled) {
          setEnabled(value);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setEnabled(fallback);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [flag, fallback]);

  return enabled;
}
