import { describe, expect, it } from "vitest";

import { useUIStore } from "../store/ui";

describe("UI store panel layout", () => {
  it("toggles panel collapse", () => {
    const { togglePanelCollapsed, panelLayout } = useUIStore.getState();
    expect(panelLayout.collapsed.directional).toBe(false);
    togglePanelCollapsed("directional");
    expect(useUIStore.getState().panelLayout.collapsed.directional).toBe(true);
  });

  it("enforces minimum panel size on resize", () => {
    const { setPanelSize } = useUIStore.getState();
    setPanelSize("transcript", 0.1);
    expect(useUIStore.getState().panelLayout.sizes.transcript).toBe(0.25);
  });
});
