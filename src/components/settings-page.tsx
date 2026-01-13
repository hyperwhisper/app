import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Settings, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface WpDevice {
  id: number;
  name: string;
  is_default: boolean;
}

export function SettingsPage() {
  // Audio device state
  const [audioDevices, setAudioDevices] = useState<WpDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState<number | null>(() => {
    const stored = localStorage.getItem("selected_audio_device_id");
    return stored ? parseInt(stored, 10) : null;
  });

  // API key
  const [apiKey, setApiKey] = useState(
    () => localStorage.getItem("deepgram_api_key") || ""
  );

  // Save settings to localStorage and sync with backend
  useEffect(() => {
    localStorage.setItem("deepgram_api_key", apiKey);
    if (apiKey.trim()) {
      invoke("set_api_key", { apiKey });
    }
  }, [apiKey]);

  useEffect(() => {
    if (selectedDeviceId !== null) {
      localStorage.setItem("selected_audio_device_id", String(selectedDeviceId));
    } else {
      localStorage.removeItem("selected_audio_device_id");
    }
    invoke("set_selected_device", { deviceId: selectedDeviceId });
  }, [selectedDeviceId]);

  // Load audio devices
  useEffect(() => {
    const loadDevices = async () => {
      try {
        const devices = await invoke<WpDevice[]>("list_audio_devices");
        setAudioDevices(devices);
      } catch (err) {
        console.error("Failed to load audio devices:", err);
      }
    };
    loadDevices();
  }, []);

  const handleClose = () => {
    getCurrentWindow().close();
  };

  const handleDrag = () => getCurrentWindow().startDragging();

  return (
    <main className="flex flex-col h-[calc(100vh-16px)] w-[calc(100vw-16px)] m-2 bg-[#171717] rounded-2xl shadow-2xl overflow-hidden">
      {/* Drag handle area */}
      <div
        className="absolute top-0 left-0 right-0 h-5 cursor-move z-50"
        onMouseDown={handleDrag}
      />

      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3">
        <div className="flex items-center gap-2">
          <Settings className="h-5 w-5 text-white/60" />
          <h1 className="text-lg font-semibold text-white">Settings</h1>
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-white/60 hover:text-white hover:bg-white/10"
          onClick={handleClose}
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        <div className="space-y-5 max-w-md mx-auto">
          {/* Audio Device */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Audio Device
            </Label>
            <Select
              value={selectedDeviceId?.toString() ?? "auto"}
              onValueChange={(v) =>
                setSelectedDeviceId(v === "auto" ? null : parseInt(v, 10))
              }
            >
              <SelectTrigger className="bg-white/5 border-0 text-white">
                <SelectValue placeholder="Auto-select" />
              </SelectTrigger>
              <SelectContent className="bg-neutral-800/95 backdrop-blur-xl border-0">
                <SelectItem value="auto" className="text-white/80 focus:bg-white/10 focus:text-white">Auto-select</SelectItem>
                {audioDevices.map((device) => (
                  <SelectItem key={device.id} value={device.id.toString()} className="text-white/80 focus:bg-white/10 focus:text-white">
                    {device.name}
                    {device.is_default ? " (default)" : ""}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* API Key */}
          <div className="space-y-2">
            <Label
              htmlFor="deepgram-key"
              className="text-xs uppercase tracking-wide text-white/50"
            >
              Deepgram API Key
            </Label>
            <Input
              id="deepgram-key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="Enter API key"
              className="bg-white/5 border-0 text-white placeholder:text-white/30"
            />
          </div>
        </div>
      </div>
    </main>
  );
}
