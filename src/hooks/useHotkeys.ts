import { useCallback, useEffect, useRef } from "react";

import { cancelInference, panicHideOverlay, triggerResponse } from "../commands";
import { onHotkeyTrigger, onOverlayVisibility } from "../events";
import { useUIStore } from "../store/ui";

const DOUBLE_TAP_MS = 400;
const TAP_DEBOUNCE_MS = 250;
const HOLD_MS = 2000;

function isTriggerChord(e: KeyboardEvent): boolean {
  return (
    e.ctrlKey &&
    e.altKey &&
    !e.metaKey &&
    (e.code === "Space" || e.key === " ")
  );
}

export function useHotkeys(
  sessionId: string | null,
  lastQuestion: string,
  enabled: boolean,
): void {
  const setAnswerNowMode = useUIStore((s) => s.setAnswerNowMode);
  const setPanicHideActive = useUIStore((s) => s.setPanicHideActive);
  const clearStreamingBuffers = useUIStore((s) => s.clearStreamingBuffers);
  const lastPressRef = useRef(0);
  const lastChordRef = useRef(0);
  const tapTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const holdTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimers = useCallback(() => {
    if (tapTimeoutRef.current) {
      clearTimeout(tapTimeoutRef.current);
      tapTimeoutRef.current = null;
    }
    if (holdTimeoutRef.current) {
      clearTimeout(holdTimeoutRef.current);
      holdTimeoutRef.current = null;
    }
  }, []);

  const handleTriggerPress = useCallback(() => {
    if (!enabled || !sessionId) return;

    const now = Date.now();
    // X11 may deliver both the global-shortcut event and a window keydown for one press.
    if (now - lastChordRef.current < 80) return;
    lastChordRef.current = now;

    if (now - lastPressRef.current < DOUBLE_TAP_MS) {
      clearTimers();
      lastPressRef.current = 0;
      setAnswerNowMode(false);
      void cancelInference();
      return;
    }

    lastPressRef.current = now;
    clearTimers();

    tapTimeoutRef.current = setTimeout(() => {
      if (lastQuestion.trim()) {
        setAnswerNowMode(false);
        clearStreamingBuffers();
        void triggerResponse(lastQuestion, sessionId);
      }
      tapTimeoutRef.current = null;
    }, TAP_DEBOUNCE_MS);

    holdTimeoutRef.current = setTimeout(() => {
      if (tapTimeoutRef.current) {
        clearTimeout(tapTimeoutRef.current);
        tapTimeoutRef.current = null;
      }
      setAnswerNowMode(true);
      if (lastQuestion.trim()) {
        clearStreamingBuffers();
        void triggerResponse(lastQuestion, sessionId);
      }
    }, HOLD_MS);
  }, [
    clearStreamingBuffers,
    clearTimers,
    enabled,
    lastQuestion,
    sessionId,
    setAnswerNowMode,
  ]);

  useEffect(() => {
    let unlistenHotkey: (() => void) | null = null;
    let unlistenOverlay: (() => void) | null = null;
    let cancelled = false;

    const setup = async () => {
      const fnHotkey = await onHotkeyTrigger(() => {
        handleTriggerPress();
      });

      const fnOverlay = await onOverlayVisibility(({ hidden }) => {
        setPanicHideActive(hidden);
        if (hidden) setAnswerNowMode(false);
      });

      if (cancelled) {
        fnHotkey();
        fnOverlay();
      } else {
        unlistenHotkey = fnHotkey;
        unlistenOverlay = fnOverlay;
      }
    };

    void setup();

    return () => {
      cancelled = true;
      clearTimers();
      unlistenHotkey?.();
      unlistenOverlay?.();
    };
  }, [clearTimers, handleTriggerPress, setAnswerNowMode, setPanicHideActive]);

  // Wayland compositors do not deliver true OS-global shortcuts to Tauri apps.
  // Listen while the Flint window is focused so Ctrl+Alt+Space still works in dev.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!enabled || !sessionId || !isTriggerChord(e) || e.repeat) return;
      e.preventDefault();
      if (e.shiftKey) {
        void panicHideOverlay();
        return;
      }
      handleTriggerPress();
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [enabled, handleTriggerPress, sessionId]);
}
