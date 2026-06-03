import { create } from "zustand";

import type { Notification, PanelLayout, PanelId, UIState } from "../types";

interface UIStore extends UIState {
  setPanelLayout: (panelLayout: PanelLayout) => void;
  setFocusedPanel: (focusedPanel: PanelId | null) => void;
  setStreamingBuffers: (streamingBuffers: UIState["streamingBuffers"]) => void;
  setNotificationQueue: (notificationQueue: Notification[]) => void;
  setTheme: (theme: UIState["theme"]) => void;
  setOverlayMinimised: (overlayMinimised: boolean) => void;
  setPanicHideActive: (panicHideActive: boolean) => void;
}

const defaultPanelLayout: PanelLayout = {
  sizes: {
    transcript: 1,
    directional: 1,
    depth: 1,
    clarifying: 1,
    context: 1,
  },
  collapsed: {
    transcript: false,
    directional: false,
    depth: false,
    clarifying: false,
    context: false,
  },
};

export const useUIStore = create<UIStore>((set) => ({
  panelLayout: defaultPanelLayout,
  focusedPanel: null,
  streamingBuffers: {
    directional: "",
    depth: "",
  },
  notificationQueue: [],
  theme: "system",
  overlayMinimised: false,
  panicHideActive: false,
  setPanelLayout: (panelLayout) => set({ panelLayout }),
  setFocusedPanel: (focusedPanel) => set({ focusedPanel }),
  setStreamingBuffers: (streamingBuffers) => set({ streamingBuffers }),
  setNotificationQueue: (notificationQueue) => set({ notificationQueue }),
  setTheme: (theme) => set({ theme }),
  setOverlayMinimised: (overlayMinimised) => set({ overlayMinimised }),
  setPanicHideActive: (panicHideActive) => set({ panicHideActive }),
}));
