import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { SettingsPage } from "@/components/settings-page";
import { ThemeProvider } from "@/components/theme-provider";
import "./index.css";

function Router() {
  const path = window.location.pathname;

  if (path === "/settings") {
    return <SettingsPage />;
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
