import { useEffect } from "react";

import {
  onClarifyingQuestion,
  onConfidenceScore,
  onDepthToken,
  onDirectionalToken,
  onResponseMetadata,
  onTurnStarted,
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
    onDirectionalToken(({ token }) => {
      useUIStore.getState().appendDirectionalToken(token);
    }),
    onDepthToken(({ token }) => {
      useUIStore.getState().appendDepthToken(token);
    }),
    onConfidenceScore(({ level }) => {
      useUIStore.getState().setConfidenceLevel(level);
    }),
    onResponseMetadata(({ pre_prepared }) => {
      useUIStore.getState().setDepthPrePrepared(pre_prepared);
    }),
    onClarifyingQuestion(({ question, rank }) => {
      useUIStore.getState().addClarifyingQuestion({ question, rank });
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
