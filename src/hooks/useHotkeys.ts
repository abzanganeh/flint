import { useCallback, useEffect, useRef } from "react";

import { cancelInference, panicHideOverlay, signalQuestionEnded, triggerResponse } from "../commands";
import { onHotkeyTrigger, onOverlayVisibility } from "../events";
import { useUIStore } from "../store/ui";

const DOUBLE_TAP_MS = 400;
const HOLD_MS = 2000;

function isLinuxPlatform(): boolean {
  return typeof navigator !== "undefined" && /linux/i.test(navigator.userAgent);
}

function isSpaceKey(e: KeyboardEvent): boolean {
  return e.code === "Space" || e.key === " ";
}

function isEnterKey(e: KeyboardEvent): boolean {
  return e.code === "Enter" || e.code === "NumpadEnter" || e.key === "Enter";
}

function ctrlHeld(e: KeyboardEvent): boolean {
  return e.ctrlKey || e.getModifierState("Control");
}

function altHeld(e: KeyboardEvent): boolean {
  return e.altKey || e.getModifierState("Alt");
}

function metaHeld(e: KeyboardEvent): boolean {
  return e.metaKey || e.getModifierState("Meta");
}

function isQuestionEndedChord(e: KeyboardEvent): boolean {
  return ctrlHeld(e) && e.code === "KeyQ" && !altHeld(e) && !metaHeld(e) && !e.shiftKey;
}

/** Primary chord: Ctrl+Alt+Space (Linux also accepts Ctrl+Super/Option+Space). */
function isTriggerChord(e: KeyboardEvent): boolean {
  if (!isSpaceKey(e) || e.repeat || !ctrlHeld(e) || e.shiftKey) return false;
  if (altHeld(e)) return true;
  return isLinuxPlatform() && metaHeld(e);
}

/** Stealth panic hide: Ctrl+Alt+Shift+Space (Linux also Ctrl+Super+Shift+Space). */
function isPanicChord(e: KeyboardEvent): boolean {
  if (!isSpaceKey(e) || e.repeat || !ctrlHeld(e) || !e.shiftKey) return false;
  if (altHeld(e)) return true;
  return isLinuxPlatform() && metaHeld(e);
}

/** Wayland often blocks Ctrl+Alt+Space; tap-only fallback while Flint is focused. */
function isLinuxShiftTrigger(e: KeyboardEvent): boolean {
  return (
    isLinuxPlatform() &&
    ctrlHeld(e) &&
    e.shiftKey &&
    isSpaceKey(e) &&
    !altHeld(e) &&
    !metaHeld(e) &&
    !e.repeat
  );
}

/** Dev-only focused-window fallback (also registered globally in Rust on Linux). */
function isLinuxDevTriggerKey(e: KeyboardEvent): boolean {
  return isLinuxPlatform() && import.meta.env.DEV && e.code === "F8" && !e.repeat;
}

function isChordModifierRelease(e: KeyboardEvent): boolean {
  return (
    e.key === "Control" ||
    e.key === "Alt" ||
    e.key === "Meta" ||
    e.key === "Shift" ||
    e.code === "Space" ||
    e.key === " "
  );
}

/** Rehearsal Ask shortcut — also exported for document-level listeners. */
export function isRehearsalSubmitChord(e: KeyboardEvent): boolean {
  if (!isEnterKey(e) || e.repeat || e.shiftKey) return false;
  return ctrlHeld(e) || metaHeld(e);
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
  const chordActiveRef = useRef(false);
  const holdFiredRef = useRef(false);
  const holdTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearHoldTimer = useCallback(() => {
    if (holdTimeoutRef.current) {
      clearTimeout(holdTimeoutRef.current);
      holdTimeoutRef.current = null;
    }
  }, []);

  const fireTap = useCallback(() => {
    if (!lastQuestion.trim() || !sessionId) return;
    setAnswerNowMode(false);
    clearStreamingBuffers();
    void triggerResponse(lastQuestion, sessionId);
  }, [clearStreamingBuffers, lastQuestion, sessionId, setAnswerNowMode]);

  const fireHold = useCallback(() => {
    if (!lastQuestion.trim() || !sessionId) return;
    holdFiredRef.current = true;
    setAnswerNowMode(true);
    clearStreamingBuffers();
    void triggerResponse(lastQuestion, sessionId);
  }, [clearStreamingBuffers, lastQuestion, sessionId, setAnswerNowMode]);

  const registerPress = useCallback((): boolean => {
    if (!enabled || !sessionId) return false;

    const now = Date.now();
    if (now - lastChordRef.current < 80) return false;
    lastChordRef.current = now;

    if (now - lastPressRef.current < DOUBLE_TAP_MS) {
      clearHoldTimer();
      lastPressRef.current = 0;
      chordActiveRef.current = false;
      holdFiredRef.current = false;
      setAnswerNowMode(false);
      clearStreamingBuffers();
      void cancelInference();
      return false;
    }

    lastPressRef.current = now;
    return true;
  }, [clearHoldTimer, clearStreamingBuffers, enabled, sessionId, setAnswerNowMode]);

  const handleGlobalShortcut = useCallback(() => {
    if (!registerPress()) return;
    fireTap();
  }, [fireTap, registerPress]);

  const handleQuestionEnded = useCallback(
    (e: KeyboardEvent) => {
      if (!enabled || !sessionId || !isQuestionEndedChord(e) || e.repeat) return;
      e.preventDefault();
      void signalQuestionEnded(sessionId);
    },
    [enabled, sessionId],
  );

  const handleChordKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!enabled || !sessionId) return;

      if (isQuestionEndedChord(e)) {
        handleQuestionEnded(e);
        return;
      }

      if (isLinuxDevTriggerKey(e)) {
        e.preventDefault();
        if (!registerPress()) return;
        fireTap();
        return;
      }

      if (isPanicChord(e)) {
        e.preventDefault();
        void panicHideOverlay();
        return;
      }

      if (isLinuxShiftTrigger(e)) {
        e.preventDefault();
        if (!registerPress()) return;
        fireTap();
        return;
      }

      if (!isTriggerChord(e) || e.repeat) return;
      e.preventDefault();

      if (!registerPress()) return;

      chordActiveRef.current = true;
      holdFiredRef.current = false;
      clearHoldTimer();
      holdTimeoutRef.current = setTimeout(() => {
        holdTimeoutRef.current = null;
        if (chordActiveRef.current) {
          fireHold();
        }
      }, HOLD_MS);
    },
    [clearHoldTimer, enabled, fireHold, fireTap, handleQuestionEnded, registerPress, sessionId],
  );

  const handleChordKeyUp = useCallback(
    (e: KeyboardEvent) => {
      if (!chordActiveRef.current || !isChordModifierRelease(e)) return;

      const stillHeld =
        (!isSpaceKey(e) &&
          (e.getModifierState("Control") ||
            e.getModifierState("Alt") ||
            e.getModifierState("Meta") ||
            e.getModifierState("Shift"))) ||
        (isSpaceKey(e) && ctrlHeld(e) && (altHeld(e) || (isLinuxPlatform() && metaHeld(e))));
      if (stillHeld) return;

      chordActiveRef.current = false;
      clearHoldTimer();

      if (!holdFiredRef.current) {
        fireTap();
      }
      holdFiredRef.current = false;
    },
    [clearHoldTimer, fireTap],
  );

  useEffect(() => {
    let unlistenHotkey: (() => void) | null = null;
    let unlistenOverlay: (() => void) | null = null;
    let cancelled = false;

    const setup = async () => {
      const fnHotkey = await onHotkeyTrigger(() => {
        handleGlobalShortcut();
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
      clearHoldTimer();
      unlistenHotkey?.();
      unlistenOverlay?.();
    };
  }, [clearHoldTimer, handleGlobalShortcut, setAnswerNowMode, setPanicHideActive]);

  // Wayland: focused-window listeners cover hold timing + Ctrl+Shift+Space / F8
  // when OS-global Ctrl+Alt chords are blocked (accepted P2 for unfocused).
  useEffect(() => {
    window.addEventListener("keydown", handleChordKeyDown, true);
    window.addEventListener("keyup", handleChordKeyUp, true);
    return () => {
      window.removeEventListener("keydown", handleChordKeyDown, true);
      window.removeEventListener("keyup", handleChordKeyUp, true);
    };
  }, [handleChordKeyDown, handleChordKeyUp]);
}
