import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./TitleBar.css";

export interface NavItem {
  label: string;
  onClick: () => void;
  active?: boolean;
}

interface TitleBarProps {
  nav?: NavItem[];
}

export default function TitleBar({ nav }: TitleBarProps) {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getCurrentWindow()
      .isMaximized()
      .then((v) => { if (!cancelled) setIsMaximized(v); })
      .catch(() => {});

    const win = getCurrentWindow();
    let unlisten: (() => void) | null = null;
    win
      .onResized(() => {
        win.isMaximized().then((v) => { if (!cancelled) setIsMaximized(v); }).catch(() => {});
      })
      .then((fn) => { unlisten = fn; })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const handleMinimize = () => { void getCurrentWindow().minimize(); };

  const handleToggleMaximize = () => { void getCurrentWindow().toggleMaximize(); };

  const handleClose = () => { void getCurrentWindow().close(); };

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left" data-tauri-drag-region>
        <span className="titlebar-brand">Flint</span>
      </div>

      {nav && nav.length > 0 && (
        <nav className="titlebar-nav">
          {nav.map((item) => (
            <button
              key={item.label}
              className={`titlebar-nav-item${item.active ? " active" : ""}`}
              onClick={item.onClick}
              type="button"
            >
              {item.label}
            </button>
          ))}
        </nav>
      )}

      <div className="titlebar-controls">
        <button
          className="titlebar-btn minimize"
          onClick={handleMinimize}
          title="Minimize"
          type="button"
          aria-label="Minimize"
        >
          <svg width="10" height="2" viewBox="0 0 10 2" aria-hidden>
            <line x1="0" y1="1" x2="10" y2="1" stroke="currentColor" strokeWidth="1.5" />
          </svg>
        </button>

        <button
          className="titlebar-btn maximize"
          onClick={handleToggleMaximize}
          title={isMaximized ? "Restore" : "Maximize"}
          type="button"
          aria-label={isMaximized ? "Restore" : "Maximize"}
        >
          {isMaximized ? (
            <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden>
              <path d="M3 0H11V8H8V11H0V3H3V0Z" stroke="currentColor" strokeWidth="1.2" fill="none" />
              <rect x="0" y="3" width="8" height="8" stroke="currentColor" strokeWidth="1.2" fill="none" />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
              <rect x="0.6" y="0.6" width="8.8" height="8.8" stroke="currentColor" strokeWidth="1.2" fill="none" />
            </svg>
          )}
        </button>

        <button
          className="titlebar-btn close"
          onClick={handleClose}
          title="Close"
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <line x1="0.5" y1="0.5" x2="9.5" y2="9.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <line x1="9.5" y1="0.5" x2="0.5" y2="9.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      </div>
    </div>
  );
}
