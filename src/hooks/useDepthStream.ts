import { useEffect } from "react";

import { onDepthToken, onResponseMetadata } from "../events";
import { useUIStore } from "../store/ui";

export function useDepthStream(): void {
  const appendDepthToken = useUIStore((s) => s.appendDepthToken);
  const setDepthPrePrepared = useUIStore((s) => s.setDepthPrePrepared);

  useEffect(() => {
    let cancelled = false;
    let unlistenToken: (() => void) | null = null;
    let unlistenMeta: (() => void) | null = null;

    const setup = async () => {
      const fnToken = await onDepthToken(({ token }) => {
        appendDepthToken(token);
      });
      const fnMeta = await onResponseMetadata(({ pre_prepared }) => {
        setDepthPrePrepared(pre_prepared);
      });
      if (cancelled) {
        fnToken();
        fnMeta();
      } else {
        unlistenToken = fnToken;
        unlistenMeta = fnMeta;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlistenToken?.();
      unlistenMeta?.();
    };
  }, [appendDepthToken, setDepthPrePrepared]);
}
