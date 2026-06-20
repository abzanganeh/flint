import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import WindowResizeGrip from "./components/WindowResizeGrip";
import { applyUiZoom, readUiZoomPreference } from "./lib/uiZoomPreference";
import "./styles/global.css";

applyUiZoom(readUiZoomPreference());

if (import.meta.env.DEV) {
  document.documentElement.style.background = "#121216";
  document.body.style.background = "#121216";
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
    <WindowResizeGrip />
  </React.StrictMode>,
);
