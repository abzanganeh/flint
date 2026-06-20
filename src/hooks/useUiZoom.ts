import { useCallback, useEffect, useState } from "react";

import {
  applyUiZoom,
  readUiZoomPreference,
  UI_ZOOM_CHANGED_EVENT,
  writeUiZoomPreference,
} from "../lib/uiZoomPreference";

export function useUiZoom() {
  const [zoom, setZoomState] = useState(readUiZoomPreference);

  useEffect(() => {
    applyUiZoom(zoom);
  }, [zoom]);

  useEffect(() => {
    const onChange = (event: Event) => {
      const detail = (event as CustomEvent<number>).detail;
      if (typeof detail === "number") {
        setZoomState(detail);
      } else {
        setZoomState(readUiZoomPreference());
      }
    };
    window.addEventListener(UI_ZOOM_CHANGED_EVENT, onChange);
    return () => window.removeEventListener(UI_ZOOM_CHANGED_EVENT, onChange);
  }, []);

  const setZoom = useCallback((value: number) => {
    const next = writeUiZoomPreference(value);
    applyUiZoom(next);
    setZoomState(next);
  }, []);

  return { zoom, setZoom };
}
