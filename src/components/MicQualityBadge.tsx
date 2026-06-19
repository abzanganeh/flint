import { useEffect, useState } from "react";

interface Props {
  onNavigateToCalibration?: () => void;
}

export default function MicQualityBadge({}: Props) {
  const [level, setLevel] = useState<"ok" | "low">("ok");

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { onAudioQualityStatus } = await import("../events");
        unlisten = await onAudioQualityStatus((payload) => {
          setLevel(payload.level === "low" ? "low" : "ok");
        });
      } catch {
        // Vitest / non-Tauri shell.
      }
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  if (level !== "low") {
    return null;
  }

  return (
    <div
      className="mic-quality-badge"
      data-testid="mic-quality-badge"
      aria-live="polite"
    >
      Mic quality low
    </div>
  );
}
