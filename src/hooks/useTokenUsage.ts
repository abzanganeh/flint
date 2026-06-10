import { useEffect } from "react";

import { onTokenUsageUpdate } from "../events";
import { useUIStore } from "../store/ui";

export function useTokenUsage(): void {
  const accumulateTokenUsage = useUIStore((s) => s.accumulateTokenUsage);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const fn = await onTokenUsageUpdate(
        ({ input, output, cost_estimate, usage_category }) => {
          accumulateTokenUsage(input, output, cost_estimate, usage_category);
        },
      );
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [accumulateTokenUsage]);
}
