import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Settings } from "lucide-react";
import { Button } from "@/components/ui/button";

interface SettingsDialogProps {
  disabled?: boolean;
}

export function SettingsDialog({ disabled = false }: SettingsDialogProps) {
  const openSettingsWindow = async () => {
    // Check if settings window already exists
    const existingWindow = await WebviewWindow.getByLabel("settings");
    if (existingWindow) {
      await existingWindow.setFocus();
      return;
    }

    // Create new settings window
    const settingsWindow = new WebviewWindow("settings", {
      url: "/settings",
      title: "Settings",
      width: 450,
      height: 580,
      decorations: false,
      center: true,
      resizable: false,
    });

    settingsWindow.once("tauri://error", (e) => {
      console.error("Failed to create settings window:", e);
    });
  };

  return (
    <Button
      variant="ghost"
      size="icon"
      className="h-8 w-8 text-muted-foreground hover:text-foreground"
      disabled={disabled}
      onClick={openSettingsWindow}
    >
      <Settings className="h-4 w-4" />
      <span className="sr-only">Settings</span>
    </Button>
  );
}
