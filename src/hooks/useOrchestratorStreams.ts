import { useEffect } from "react";

import {
  onAnswerToken,
  onConfidenceScore,
  onResponseMetadata,
  onTurnStarted,
  onVisualToken,
} from "../events";
import { useUIStore } from "../store/ui";

let listenerRefCount = 0;
let teardown: (() => void) | null = null;
let setupInFlight: Promise<void> | null = null;

async function attachOrchestratorListeners(): Promise<() => void> {
  const unlistenFns = await Promise.all([
    onTurnStarted(({ question, turn }) => {
      useUIStore.getState().startTurn(question, turn);
    }),
    onAnswerToken(({ token }) => {
      useUIStore.getState().appendAnswerToken(token);
    }),
    onVisualToken(({ token }) => {
      useUIStore.getState().appendVisualToken(token);
    }),
    onConfidenceScore(({ level }) => {
      useUIStore.getState().setConfidenceLevel(level);
    }),
    onResponseMetadata(({ pre_prepared }) => {
      useUIStore.getState().setDepthPrePrepared(pre_prepared);
    }),
  ]);

  return () => {
    unlistenFns.forEach((fn) => fn());
  };
}

function ensureOrchestratorListeners(): void {
  if (teardown || setupInFlight) return;

  setupInFlight = attachOrchestratorListeners()
    .then((fn) => {
      setupInFlight = null;
      if (listenerRefCount > 0) {
        teardown = fn;
      } else {
        fn();
      }
    })
    .catch(() => {
      setupInFlight = null;
    });
}

/**
 * Register orchestrator streaming listeners once per app lifetime (ref-counted).
 * Must live above OverlayLayout so collapsed panels never drop events.
 */
export function useOrchestratorStreams(): void {
  useEffect(() => {
    listenerRefCount += 1;
    ensureOrchestratorListeners();

    return () => {
      listenerRefCount -= 1;
      if (listenerRefCount === 0 && teardown) {
        teardown();
        teardown = null;
      }
    };
  }, []);
}
