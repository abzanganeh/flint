const STORAGE_KEY = "flint-ui-zoom";
const MIN_ZOOM = 0.85;
const MAX_ZOOM = 1.3;
const DEFAULT_ZOOM = 1;

export const UI_ZOOM_CHANGED_EVENT = "flint-ui-zoom-changed";

export function clampUiZoom(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_ZOOM;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, value));
}

export function readUiZoomPreference(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return DEFAULT_ZOOM;
    const parsed = Number.parseFloat(raw);
    return clampUiZoom(parsed);
  } catch {
    return DEFAULT_ZOOM;
  }
}

export function writeUiZoomPreference(zoom: number): number {
  const clamped = clampUiZoom(zoom);
  try {
    localStorage.setItem(STORAGE_KEY, String(clamped));
    window.dispatchEvent(new CustomEvent(UI_ZOOM_CHANGED_EVENT, { detail: clamped }));
  } catch {
    // Private browsing or storage blocked — ignore.
  }
  return clamped;
}

export function applyUiZoom(zoom: number): number {
  const clamped = clampUiZoom(zoom);
  document.documentElement.style.setProperty("--flint-ui-zoom", String(clamped));
  return clamped;
}

export const UI_ZOOM_MIN = MIN_ZOOM;
export const UI_ZOOM_MAX = MAX_ZOOM;
export const UI_ZOOM_DEFAULT = DEFAULT_ZOOM;
