import type { PanelId } from "../types";

/**
 * One accent hue per overlay panel so Transcript/Answer/Visual/Context are
 * distinguishable at a glance instead of four identical gray title bars.
 * Hues are picked to stay clear of colors already carrying semantic meaning
 * elsewhere in the app (green = high confidence, amber = warning, red =
 * error, brand purple = primary actions) — see Answer's confidence border,
 * which stays independent of this accent.
 */
export interface PanelAccent {
  /** Muted label/icon color for the resting state. */
  text: string;
  /** Brighter color for hover/active states. */
  textStrong: string;
  /** Solid color for the panel's identity stripe (collapsed strip, top border). */
  stripe: string;
  /** Low-alpha background tint for header bars. */
  headerBg: string;
  /** Low-alpha border tint separating the header from panel content. */
  headerBorder: string;
}

export const PANEL_ACCENTS: Record<PanelId, PanelAccent> = {
  transcript: {
    text: "#7dd3fc",
    textStrong: "#38bdf8",
    stripe: "#0ea5e9",
    headerBg: "rgba(14, 165, 233, 0.10)",
    headerBorder: "rgba(14, 165, 233, 0.35)",
  },
  answer: {
    text: "#c4b5fd",
    textStrong: "#a78bfa",
    stripe: "#7c3aed",
    headerBg: "rgba(124, 58, 237, 0.10)",
    headerBorder: "rgba(124, 58, 237, 0.35)",
  },
  visual: {
    text: "#5eead4",
    textStrong: "#2dd4bf",
    stripe: "#0d9488",
    headerBg: "rgba(13, 148, 136, 0.10)",
    headerBorder: "rgba(13, 148, 136, 0.35)",
  },
  context: {
    text: "#f9a8d4",
    textStrong: "#f472b6",
    stripe: "#db2777",
    headerBg: "rgba(219, 39, 119, 0.10)",
    headerBorder: "rgba(219, 39, 119, 0.35)",
  },
};

export const PANEL_LABELS: Record<PanelId, string> = {
  transcript: "Transcript",
  answer: "Answer",
  visual: "Visual",
  context: "Context",
};

/**
 * Identity color for the Rehearsal prep-tools sidebar (Prep/Qs/Chat/Stories).
 * Indigo keeps it visually distinct from all four panel hues above while
 * still reading as a cohesive "cool" palette against the near-black chrome.
 */
export const SIDEBAR_ACCENT: PanelAccent = {
  text: "#a5b4fc",
  textStrong: "#818cf8",
  stripe: "#6366f1",
  headerBg: "rgba(99, 102, 241, 0.10)",
  headerBorder: "rgba(99, 102, 241, 0.35)",
};
