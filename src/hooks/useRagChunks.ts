import { useEffect } from "react";

import { getSessionSnapshot } from "../commands";
import { onRagChunksUpdate } from "../events";
import { useUIStore } from "../store/ui";

export function useRagChunks(sessionId: string | null): void {
  const setRagChunks = useUIStore((s) => s.setRagChunks);
  const setDigestSummary = useUIStore((s) => s.setDigestSummary);

  useEffect(() => {
    if (!sessionId) return;

    let cancelled = false;

    void getSessionSnapshot().then((snap) => {
      if (cancelled || !snap.digest) return;
      const d = snap.digest;
      setDigestSummary(
        `${d.role} @ ${d.company} · ${d.domain} · ${d.seniority}`,
      );
    });

    return () => {
      cancelled = true;
    };
  }, [sessionId, setDigestSummary]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const fn = await onRagChunksUpdate(({ chunks }) => {
        setRagChunks(
          chunks.map((c) => ({ text: c.text, score: c.score })),
        );
      });
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
  }, [setRagChunks]);
}
