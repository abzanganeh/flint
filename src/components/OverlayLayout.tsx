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

// ── Stack (vertical) resize handle ───────────────────────────────────────────

interface StackResizeHandleProps {
  topId: PanelId;
  bottomId: PanelId;
}

const StackResizeHandle = ({ topId, bottomId }: StackResizeHandleProps) => {
  const { setPanelSize, panelLayout } = useUIStore();

  const onMouseDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startTop = panelLayout.sizes[topId];
    const startBottom = panelLayout.sizes[bottomId];

    const onMove = (ev: MouseEvent) => {
      const dy = ev.clientY - startY;
      const delta = dy / 200;
      setPanelSize(topId, startTop + delta);
      setPanelSize(bottomId, startBottom - delta);
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
        height: 4,
        cursor: "row-resize",
        backgroundColor: "transparent",
        flexShrink: 0,
        zIndex: 10,
      }}
      onMouseEnter={(e) =>
        ((e.currentTarget as HTMLDivElement).style.backgroundColor = "#2d3748")
      }
      onMouseLeave={(e) =>
        ((e.currentTarget as HTMLDivElement).style.backgroundColor = "transparent")
      }
    />
  );
};

// ── Stack panel slot (horizontal header + content) ────────────────────────────

interface StackPanelSlotProps {
  id: PanelId;
  children: ReactNode;
}

// Default heights per spec (FR-4.6): Transcript 20%, Directional 30%, Depth 30%, Clarifying 10%, Context 10%.
const STACK_DEFAULT_SIZES: Record<PanelId, number> = {
  transcript: 1,
  directional: 1.5,
  depth: 1.5,
  clarifying: 0.5,
  context: 0.5,
};

const StackPanelSlot = ({ id, children }: StackPanelSlotProps) => {
  const { panelLayout, togglePanelCollapsed } = useUIStore();
  const collapsed = panelLayout.collapsed[id];
  const rawSize = panelLayout.sizes[id] ?? STACK_DEFAULT_SIZES[id];

  return (
    <div
      style={{
        flex: collapsed ? "0 0 28px" : `${rawSize} 1 0`,
        minHeight: collapsed ? 28 : 120,
        display: "flex",
        flexDirection: "column",
        borderBottom: "1px solid #1e2028",
        overflow: "hidden",
        transition: "flex 0.18s ease",
      }}
    >
      <button
        aria-label={collapsed ? `Expand ${PANEL_LABELS[id]}` : `Collapse ${PANEL_LABELS[id]}`}
        onClick={() => togglePanelCollapsed(id)}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0 12px",
          height: 28,
          background: "none",
          border: "none",
          cursor: "pointer",
          color: "#52525b",
          fontSize: 10,
          fontWeight: 600,
          letterSpacing: "0.06em",
          textTransform: "uppercase",
          flexShrink: 0,
          width: "100%",
          boxSizing: "border-box",
          transition: "color 0.12s",
        }}
        onMouseEnter={(e) => { (e.currentTarget as HTMLButtonElement).style.color = "#9ca3af"; }}
        onMouseLeave={(e) => { (e.currentTarget as HTMLButtonElement).style.color = "#52525b"; }}
      >
        <span>{PANEL_LABELS[id]}</span>
        <span>{collapsed ? "▼" : "▲"}</span>
      </button>

      {!collapsed && (
        <div style={{ flex: 1, overflow: "hidden" }}>{children}</div>
      )}
    </div>
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

const PANELS: Array<{ id: PanelId; label: string }> = [
  { id: "transcript", label: "Transcript" },
  { id: "directional", label: "Directional" },
  { id: "depth", label: "Depth" },
  { id: "clarifying", label: "Clarifying" },
  { id: "context", label: "Context" },
];

const OverlayLayout = ({
  transcript,
  directional,
  depth,
  clarifying,
  context,
}: OverlayLayoutProps) => {
  const { overlayMinimised, panicHideActive, layoutMode, setLayoutMode } = useUIStore();

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

  const panelContent: Record<PanelId, ReactNode> = {
    transcript,
    directional,
    depth,
    clarifying,
    context,
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        width: "100%",
        backgroundColor: "#0f1117",
        overflow: "hidden",
        position: "relative",
      }}
    >
      {/* Layout toggle */}
      <div
        role="group"
        aria-label="Panel layout"
        style={{
          position: "absolute",
          top: 4,
          right: 8,
          zIndex: 100,
          display: "flex",
          gap: 4,
        }}
      >
        <button
          onClick={() => setLayoutMode("stack")}
          aria-pressed={layoutMode === "stack"}
          aria-label="Vertical stack layout"
          title="Vertical stack layout"
          style={{
            background: layoutMode === "stack" ? "#3b82f6" : "#1e2028",
            border: "none",
            color: "#fff",
            borderRadius: 4,
            padding: "2px 7px",
            fontSize: 11,
            cursor: "pointer",
          }}
        >
          ≡
        </button>
        <button
          onClick={() => setLayoutMode("grid")}
          aria-pressed={layoutMode === "grid"}
          aria-label="Side-by-side grid layout"
          title="Side-by-side grid layout"
          style={{
            background: layoutMode === "grid" ? "#3b82f6" : "#1e2028",
            border: "none",
            color: "#fff",
            borderRadius: 4,
            padding: "2px 7px",
            fontSize: 11,
            cursor: "pointer",
          }}
        >
          ⊞
        </button>
      </div>

      {layoutMode === "stack" ? (
        // ── Vertical stack (default, FR-4.6) ───────────────────────────────
        <div style={{ display: "flex", flexDirection: "column", height: "100%", overflow: "hidden" }}>
          {PANELS.map((p, i) => (
            <div key={p.id} style={{ display: "contents" }}>
              <StackPanelSlot id={p.id}>{panelContent[p.id]}</StackPanelSlot>
              {i < PANELS.length - 1 && PANELS[i + 1] && (
                <StackResizeHandle topId={p.id} bottomId={PANELS[i + 1]!.id} />
              )}
            </div>
          ))}
        </div>
      ) : (
        // ── Horizontal grid ─────────────────────────────────────────────────
        <div style={{ display: "flex", flexDirection: "row", height: "100%", overflow: "hidden" }}>
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
      )}
    </div>
  );
};

export default OverlayLayout;
