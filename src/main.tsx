import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { SettingsPage } from "@/components/settings-page";
import { Indicator } from "@/components/indicator";
import { ThemeProvider } from "@/components/theme-provider";
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

  return <App />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider defaultTheme="dark" storageKey="hyperwhisper-theme">
      <Router />
    </ThemeProvider>
  </React.StrictMode>
);
