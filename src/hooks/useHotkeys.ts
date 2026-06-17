import { useCallback, useEffect, useRef } from "react";

import { cancelInference, panicHideOverlay, triggerResponse } from "../commands";
import { onHotkeyTrigger, onOverlayVisibility } from "../events";
import { useUIStore } from "../store/ui";

const DOUBLE_TAP_MS = 400;
const HOLD_MS = 2000;

function isTriggerChord(e: KeyboardEvent): boolean {
  return (
    e.ctrlKey &&
    e.altKey &&
    !e.metaKey &&
    (e.code === "Space" || e.key === " ")
  );
}

function isChordModifierRelease(e: KeyboardEvent): boolean {
  return (
    e.key === "Control" ||
    e.key === "Alt" ||
    e.code === "Space" ||
    e.key === " "
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
  }, [
    clearStreamingBuffers,
    lastQuestion,
    sessionId,
    setAnswerNowMode,
  ]);

  const fireHold = useCallback(() => {
    if (!lastQuestion.trim() || !sessionId) return;
    holdFiredRef.current = true;
    setAnswerNowMode(true);
    clearStreamingBuffers();
    void triggerResponse(lastQuestion, sessionId);
  }, [
    clearStreamingBuffers,
    lastQuestion,
    sessionId,
    setAnswerNowMode,
  ]);

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
      void cancelInference();
      return false;
    }

    lastPressRef.current = now;
    return true;
  }, [clearHoldTimer, enabled, sessionId, setAnswerNowMode]);

  /** OS-global shortcut (X11 / macOS / Windows) — single fire on press, tap only. */
  const handleGlobalShortcut = useCallback(() => {
    if (!registerPress()) return;
    fireTap();
  }, [fireTap, registerPress]);

  /** Window-focused chord — supports hold-to-Answer-Now via keyup timing. */
  const handleChordKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!enabled || !sessionId || !isTriggerChord(e) || e.repeat) return;
      e.preventDefault();

      if (e.shiftKey) {
        void panicHideOverlay();
        return;
      }

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
    [clearHoldTimer, enabled, fireHold, registerPress, sessionId],
  );

  const handleChordKeyUp = useCallback(
    (e: KeyboardEvent) => {
      if (!chordActiveRef.current || !isChordModifierRelease(e)) return;

      const stillHeld =
        (e.code !== "Space" && e.key !== " " && (e.getModifierState("Control") || e.getModifierState("Alt"))) ||
        ((e.code === "Space" || e.key === " ") && e.ctrlKey && e.altKey);
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

  // Wayland does not deliver true OS-global shortcuts; macOS/Windows/X11 also
  // benefit from focused-window hold detection (global plugin fires once on press).
  useEffect(() => {
    window.addEventListener("keydown", handleChordKeyDown, true);
    window.addEventListener("keyup", handleChordKeyUp, true);
    return () => {
      window.removeEventListener("keydown", handleChordKeyDown, true);
      window.removeEventListener("keyup", handleChordKeyUp, true);
    };
  }, [handleChordKeyDown, handleChordKeyUp]);
}
