import { useEffect, useId, useRef, useState } from "react";

export interface InfoPopoverProps {
  /** Short label next to the trigger (optional). */
  label?: string;
  /** Accessible name when label is omitted. */
  ariaLabel: string;
  children: React.ReactNode;
}

/**
 * Compact [i] help trigger — keeps instructional copy out of the main layout
 * until the user asks for it.
 */
export default function InfoPopover({ label, ariaLabel, children }: InfoPopoverProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="info-popover" ref={rootRef}>
      {label && <span className="info-popover__label">{label}</span>}
      <button
        type="button"
        className="info-popover__trigger"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={panelId}
        onClick={() => setOpen((v) => !v)}
      >
        i
      </button>
      {open && (
        <div id={panelId} className="info-popover__panel" role="tooltip">
          {children}
        </div>
      )}
    </div>
  );
}
