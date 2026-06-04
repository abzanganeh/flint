import { useEffect } from "react";

import { onConfidenceScore, onDirectionalToken } from "../events";
import { useUIStore } from "../store/ui";

export function useDirectionalStream(): void {
  const appendDirectionalToken = useUIStore((s) => s.appendDirectionalToken);
  const setConfidenceLevel = useUIStore((s) => s.setConfidenceLevel);

  useEffect(() => {
    let cancelled = false;
    let unlistenToken: (() => void) | null = null;
    let unlistenConf: (() => void) | null = null;

    const setup = async () => {
      const fnToken = await onDirectionalToken(({ token }) => {
        appendDirectionalToken(token);
      });
      const fnConf = await onConfidenceScore(({ level }) => {
        setConfidenceLevel(level);
      });
      if (cancelled) {
        fnToken();
        fnConf();
      } else {
        unlistenToken = fnToken;
        unlistenConf = fnConf;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlistenToken?.();
      unlistenConf?.();
    };
  }, [appendDirectionalToken, setConfidenceLevel]);
}
