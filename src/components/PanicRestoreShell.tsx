import type { ReactNode } from "react";

import { panicHideOverlay } from "../commands";
import { useUIStore } from "../store/ui";

interface PanicRestoreShellProps {
  children: ReactNode;
}

/**
 * When panic hide is active, hides all stealth chrome and shows only a small
 * restore control. OverlayLayout alone is insufficient — rehearsal/live also
 * render question inputs, sidebars, and footers outside the panel stack.
 */
export function PanicRestoreShell({ children }: PanicRestoreShellProps) {
  const panicHideActive = useUIStore((s) => s.panicHideActive);

  if (panicHideActive) {
    return (
      <div
        data-testid="panic-restore-shell"
        style={{
          position: "fixed",
          bottom: 12,
          right: 12,
          zIndex: 9999,
        }}
      >
        <button
          type="button"
          data-testid="panic-restore-button"
          onClick={() => void panicHideOverlay()}
          style={{
            padding: "6px 12px",
            fontSize: "11px",
            fontWeight: 600,
            borderRadius: 6,
            border: "1px solid #374151",
            backgroundColor: "rgba(15, 17, 23, 0.92)",
            color: "#94a3b8",
            cursor: "pointer",
          }}
        >
          Show Flint (Ctrl+Alt+Shift+Space)
        </button>
      </div>
    );
  }

  return <>{children}</>;
}

export default PanicRestoreShell;
