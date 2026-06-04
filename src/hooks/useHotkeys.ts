import { useEffect, useRef } from "react";

import { cancelInference, triggerResponse } from "../commands";
import { onHotkeyTrigger, onOverlayVisibility } from "../events";
import { useUIStore } from "../store/ui";

const DOUBLE_TAP_MS = 400;
const TAP_DEBOUNCE_MS = 250;
const HOLD_MS = 2000;

export function useHotkeys(
  sessionId: string | null,
  lastQuestion: string,
  enabled: boolean,
): void {
  const setAnswerNowMode = useUIStore((s) => s.setAnswerNowMode);
  const setPanicHideActive = useUIStore((s) => s.setPanicHideActive);
  const lastPressRef = useRef(0);
  const tapTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const holdTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const clearTimers = () => {
      if (tapTimeoutRef.current) {
        clearTimeout(tapTimeoutRef.current);
        tapTimeoutRef.current = null;
      }
      if (holdTimeoutRef.current) {
        clearTimeout(holdTimeoutRef.current);
        holdTimeoutRef.current = null;
      }
    };

    let unlistenHotkey: (() => void) | null = null;
    let unlistenOverlay: (() => void) | null = null;
    let cancelled = false;

    const setup = async () => {
      const fnHotkey = await onHotkeyTrigger(() => {
        if (!enabled || !sessionId) return;

        const now = Date.now();
        if (now - lastPressRef.current < DOUBLE_TAP_MS) {
          clearTimers();
          lastPressRef.current = 0;
          void cancelInference();
          return;
        }

        lastPressRef.current = now;
        clearTimers();

        tapTimeoutRef.current = setTimeout(() => {
          if (lastQuestion.trim()) {
            void triggerResponse(lastQuestion, sessionId);
          }
          holdTimeoutRef.current = null;
        }, TAP_DEBOUNCE_MS);

        holdTimeoutRef.current = setTimeout(() => {
          if (tapTimeoutRef.current) {
            clearTimeout(tapTimeoutRef.current);
            tapTimeoutRef.current = null;
          }
          setAnswerNowMode(true);
        }, HOLD_MS);
      });

      const fnOverlay = await onOverlayVisibility(({ hidden }) => {
        setPanicHideActive(hidden);
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
  }, [enabled, lastQuestion, sessionId, setAnswerNowMode, setPanicHideActive]);
}
