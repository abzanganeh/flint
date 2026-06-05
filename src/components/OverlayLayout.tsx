import type { ReactNode } from "react";

import type { PanelId } from "../types";
import { useUIStore } from "../store/ui";

// ── Panel wrapper ─────────────────────────────────────────────────────────────

const PANEL_LABELS: Record<PanelId, string> = {
  transcript: "Transcript",
  directional: "Directional",
  depth: "Depth",
  clarifying: "Clarifying",
  context: "Context",
};

interface PanelSlotProps {
  id: PanelId;
  children: ReactNode;
}

const PanelSlot = ({ id, children }: PanelSlotProps) => {
  const { panelLayout, togglePanelCollapsed } = useUIStore();
  const collapsed = panelLayout.collapsed[id];
  const size = panelLayout.sizes[id];

  return (
    <div
      style={{
        flex: collapsed ? "0 0 28px" : `${size} 1 0`,
        minWidth: collapsed ? 28 : 120,
        display: "flex",
        flexDirection: "column",
        borderRight: "1px solid #1e2028",
        overflow: "hidden",
        transition: "flex 0.18s ease",
      }}
    >
      {/* Collapse strip — shows label when collapsed, just arrow when expanded */}
      <button
        aria-label={collapsed ? `Expand ${PANEL_LABELS[id]}` : `Collapse ${PANEL_LABELS[id]}`}
        onClick={() => togglePanelCollapsed(id)}
        title={collapsed ? `Expand ${PANEL_LABELS[id]}` : `Collapse ${PANEL_LABELS[id]}`}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: collapsed ? "center" : "flex-end",
          padding: "0 8px",
          height: collapsed ? 40 : 20,
          background: "none",
          border: "none",
          cursor: "pointer",
          color: "#52525b",
          fontSize: collapsed ? 10 : 9,
          fontWeight: 600,
          letterSpacing: "0.06em",
          textTransform: "uppercase",
          flexShrink: 0,
          width: "100%",
          boxSizing: "border-box",
          writingMode: collapsed ? "vertical-rl" : "horizontal-tb",
          transition: "color 0.12s",
        }}
        onMouseEnter={(e) => { (e.currentTarget as HTMLButtonElement).style.color = "#9ca3af"; }}
        onMouseLeave={(e) => { (e.currentTarget as HTMLButtonElement).style.color = "#52525b"; }}
      >
        {collapsed ? PANEL_LABELS[id] : "◀"}
      </button>

      {!collapsed && (
        <div style={{ flex: 1, overflow: "hidden" }}>{children}</div>
      )}
    </div>
  );
};

// ── Resize handle ─────────────────────────────────────────────────────────────

interface ResizeHandleProps {
  leftId: PanelId;
  rightId: PanelId;
}

const ResizeHandle = ({ leftId, rightId }: ResizeHandleProps) => {
  const { setPanelSize, panelLayout } = useUIStore();

  const onMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startLeft = panelLayout.sizes[leftId];
    const startRight = panelLayout.sizes[rightId];

    const onMove = (ev: MouseEvent) => {
      const dx = ev.clientX - startX;
      // Convert pixel delta to fractional unit change (200px = 1 unit).
      const delta = dx / 200;
      setPanelSize(leftId, startLeft + delta);
      setPanelSize(rightId, startRight - delta);
    };

    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div
      onMouseDown={onMouseDown}
      style={{
        width: 4,
        cursor: "col-resize",
        backgroundColor: "transparent",
        flexShrink: 0,
        zIndex: 10,
      }}
      onMouseEnter={(e) =>
        ((e.currentTarget as HTMLDivElement).style.backgroundColor = "#2d3748")
      }
      onMouseLeave={(e) =>
        ((e.currentTarget as HTMLDivElement).style.backgroundColor =
          "transparent")
      }
    />
  );
};

// ── Overlay layout ────────────────────────────────────────────────────────────

interface OverlayLayoutProps {
  transcript: ReactNode;
  directional: ReactNode;
  depth: ReactNode;
  clarifying: ReactNode;
  context: ReactNode;
}

const OverlayLayout = ({
  transcript,
  directional,
  depth,
  clarifying,
  context,
}: OverlayLayoutProps) => {
  const { overlayMinimised, panicHideActive } = useUIStore();

  if (panicHideActive) return null;

  if (overlayMinimised) {
    return (
      <div
        style={{
          position: "fixed",
          bottom: 16,
          right: 16,
          width: 8,
          height: 8,
          borderRadius: "50%",
          backgroundColor: "#3b82f6",
          opacity: 0.6,
        }}
      />
    );
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "row",
        height: "100%",
        width: "100%",
        backgroundColor: "#0f1117",
        overflow: "hidden",
      }}
    >
      <PanelSlot id="transcript">{transcript}</PanelSlot>
      <ResizeHandle leftId="transcript" rightId="directional" />
      <PanelSlot id="directional">{directional}</PanelSlot>
      <ResizeHandle leftId="directional" rightId="depth" />
      <PanelSlot id="depth">{depth}</PanelSlot>
      <ResizeHandle leftId="depth" rightId="clarifying" />
      <PanelSlot id="clarifying">{clarifying}</PanelSlot>
      <ResizeHandle leftId="clarifying" rightId="context" />
      <PanelSlot id="context">{context}</PanelSlot>
    </div>
  );
};

export default OverlayLayout;
