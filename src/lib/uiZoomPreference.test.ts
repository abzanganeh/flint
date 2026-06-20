import { describe, expect, it } from "vitest";

import {
  clampUiZoom,
  readUiZoomPreference,
  UI_ZOOM_DEFAULT,
  UI_ZOOM_MAX,
  UI_ZOOM_MIN,
  writeUiZoomPreference,
} from "./uiZoomPreference";

describe("uiZoomPreference", () => {
  it("clamps zoom to supported range", () => {
    expect(clampUiZoom(0.5)).toBe(UI_ZOOM_MIN);
    expect(clampUiZoom(2)).toBe(UI_ZOOM_MAX);
    expect(clampUiZoom(1)).toBe(UI_ZOOM_DEFAULT);
  });

  it("persists zoom in localStorage", () => {
    const saved = writeUiZoomPreference(1.15);
    expect(saved).toBe(1.15);
    expect(readUiZoomPreference()).toBe(1.15);
    writeUiZoomPreference(UI_ZOOM_DEFAULT);
  });
});
