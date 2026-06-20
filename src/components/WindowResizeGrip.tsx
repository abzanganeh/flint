import { getCurrentWindow } from "@tauri-apps/api/window";

import "./WindowResizeGrip.css";

/**
 * Bottom-right resize handle for the frameless Tauri window.
 */
export default function WindowResizeGrip() {
  const startResize = () => {
    void getCurrentWindow()
      .startResizeDragging("SouthEast")
      .catch(() => undefined);
  };

  return (
    <button
      type="button"
      className="window-resize-grip"
      aria-label="Resize window"
      title="Drag to resize"
      onMouseDown={(event) => {
        event.preventDefault();
        startResize();
      }}
    >
      <svg
        className="window-resize-grip__icon"
        width="12"
        height="12"
        viewBox="0 0 12 12"
        aria-hidden
      >
        <path d="M4 12L12 12L12 4" fill="none" stroke="currentColor" strokeWidth="1.5" />
        <path d="M8 12L12 12L12 8" fill="none" stroke="currentColor" strokeWidth="1.5" />
      </svg>
    </button>
  );
}
