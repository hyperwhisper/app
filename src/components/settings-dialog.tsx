import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Settings } from "lucide-react";
import { Button } from "@/components/ui/button";

interface SettingsDialogProps {
  disabled?: boolean;
  tooltipPosition?: 'top' | 'bottom';
}

export function SettingsDialog({ disabled = false, tooltipPosition = 'top' }: SettingsDialogProps) {
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
      height: 550,
      decorations: false,
      center: true,
      resizable: false,
      transparent: true,
      backgroundColor: [0, 0, 0, 0],
    });

    settingsWindow.once("tauri://error", (e) => {
      console.error("Failed to create settings window:", e);
    });
  };

  return (
    <div className="relative group">
      <Button
        variant="ghost"
        size="icon"
        className="h-8 w-8 text-muted-foreground hover:text-foreground"
        disabled={disabled}
        onMouseDown={(e) => {
          e.stopPropagation();
          e.preventDefault();
          openSettingsWindow();
        }}
      >
        <Settings className="h-4 w-4" />
        <span className="sr-only">Settings</span>
      </Button>
      <div className={`absolute left-0 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none ${
        tooltipPosition === 'bottom' ? 'top-full mt-2' : 'bottom-full mb-2'
      }`}>
        Settings
      </div>
    </div>
  );
}
