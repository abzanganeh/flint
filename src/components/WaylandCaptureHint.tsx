import { useEffect, useState } from "react";

const STORAGE_KEY = "flint:wayland-capture-hint-dismissed";

const isLinux = (): boolean =>
  typeof navigator !== "undefined" && /Linux/i.test(navigator.userAgent);

/**
 * One-time banner shown on Linux at the top of the live overlay.
 *
 * No Wayland compositor implements a standardised "exclude window from
 * capture" protocol, so the only real protection against the overlay being
 * recorded is the user selecting the correct source in the PipeWire portal
 * picker. The hint warns the user explicitly before that dialog appears.
 *
 * Dismissal is persisted in `localStorage` — not security-critical state,
 * does not warrant a keychain round trip.
 */
const WaylandCaptureHint = () => {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (!isLinux()) return;
    try {
      if (window.localStorage.getItem(STORAGE_KEY) === "1") return;
    } catch {
      // localStorage unavailable (private mode etc.) — show once per session.
    }
    setVisible(true);
  }, []);

  const dismiss = () => {
    try {
      window.localStorage.setItem(STORAGE_KEY, "1");
    } catch {
      // best-effort
    }
    setVisible(false);
  };

  if (!visible) return null;

  return (
    <div
      data-testid="wayland-capture-hint"
      role="alert"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "8px 12px",
        backgroundColor: "#1f1407",
        borderBottom: "1px solid #b45309",
        color: "#fbbf24",
        fontSize: "12px",
        lineHeight: 1.5,
        flexShrink: 0,
      }}
    >
      <span style={{ fontWeight: 700, letterSpacing: "0.04em" }}>
        Heads up:
      </span>
      <span style={{ color: "#fde68a", flex: 1 }}>
        When the screen-share dialog opens, share a specific window — do{" "}
        <strong>not</strong> select the Flint overlay or your whole desktop.
        Wayland has no protocol to hide windows from capture.
      </span>
      <button
        data-testid="wayland-capture-hint-dismiss"
        onClick={dismiss}
        style={{
          padding: "3px 10px",
          fontSize: "11px",
          fontWeight: 600,
          borderRadius: 4,
          border: "1px solid #b45309",
          backgroundColor: "transparent",
          color: "#fbbf24",
          cursor: "pointer",
        }}
      >
        Got it
      </button>
    </div>
  );
};

export default WaylandCaptureHint;
