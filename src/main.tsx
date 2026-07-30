import React from "react";
import ReactDOM from "react-dom/client";
import { SettingsPage } from "@/components/settings-page";
import { Indicator } from "@/components/indicator";
import "./index.css";

function Router() {
  const path = window.location.pathname;

  if (path === "/settings") {
    return <SettingsPage />;
  }

  // The recording waveform strip runs in its own window at /indicator.
  if (path === "/indicator") {
    return <Indicator />;
  }

  // Nothing opens this. The app has no main window: it lives in the menu bar,
  // and Rust does the work. Say so rather than showing an empty rectangle.
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        height: "100vh",
        padding: "0 24px",
        textAlign: "center",
        fontFamily: "ui-sans-serif, system-ui, -apple-system, sans-serif",
        fontSize: 13,
        lineHeight: 1.5,
        color: "rgba(255, 255, 255, 0.7)",
      }}
    >
      Omegawhisper runs in the menu bar. Press your dictation key to speak, or
      open Settings from the menu-bar icon.
    </div>
  );
}

// The dark look is set once on <html> in index.html, so nothing here has to
// carry a theme around.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Router />
  </React.StrictMode>
);
