import { useEffect } from "react";

import { onTranscriptionChunk } from "../events";
import type { Speaker } from "../types";

export interface TranscriptionLine {
  text: string;
  speaker: Speaker;
  timestamp: number;
}

type TranscriptionHandler = (line: TranscriptionLine) => void;

// Module-level singleton state — one Tauri event listener shared across all
// React instances, ref-counted so StrictMode double-mount is safe.
let listenerRefCount = 0;
const handlers = new Set<TranscriptionHandler>();
let teardown: (() => void) | null = null;
let setupInFlight: Promise<void> | null = null;

function dispatchLine(line: TranscriptionLine): void {
  handlers.forEach((handler) => handler(line));
}

async function attachTranscriptionListener(): Promise<() => void> {
  return onTranscriptionChunk(({ text, speaker, timestamp }) => {
    dispatchLine({ text, speaker, timestamp });
  });
}

function releaseTranscriptionListener(): void {
  if (teardown) {
    teardown();
    teardown = null;
  }
  // setupInFlight will call fn() when it resolves with refCount === 0, so no
  // explicit cancellation needed there — the resolved fn() is the unlisten.
}

function ensureTranscriptionListener(): void {
  if (teardown || setupInFlight) return;

  setupInFlight = attachTranscriptionListener()
    .then((fn) => {
      setupInFlight = null;
      if (listenerRefCount > 0) {
        teardown = fn;
      } else {
        // All subscribers unmounted before the async listen() resolved.
        fn();
      }
    })
    .catch((err: unknown) => {
      setupInFlight = null;
      console.error("[useTranscriptionStream] failed to attach Tauri listener:", err);
    });
}

/**
 * Subscribe to live transcription chunks. The underlying Tauri event listener
 * is ref-counted so React StrictMode double-mount and HMR do not register
 * duplicate handlers.
 */
export function useTranscriptionStream(handler: TranscriptionHandler): void {
  useEffect(() => {
    handlers.add(handler);
    listenerRefCount += 1;
    ensureTranscriptionListener();

    return () => {
      handlers.delete(handler);
      listenerRefCount -= 1;
      if (listenerRefCount === 0) {
        releaseTranscriptionListener();
      }
    };
  }, [handler]);
}

// Clean up the Tauri listener when Vite HMR disposes this module so a hot
// reload does not leave an orphan listener dispatching to a stale handlers Set.
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    releaseTranscriptionListener();
    handlers.clear();
    listenerRefCount = 0;
    setupInFlight = null;
  });
}
