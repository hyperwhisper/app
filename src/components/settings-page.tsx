import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Settings, Sun, Moon, Monitor, Download, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Separator } from "@/components/ui/separator";
import { Progress } from "@/components/ui/progress";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useTheme } from "@/components/theme-provider";

interface WpDevice {
  id: number;
  name: string;
  is_default: boolean;
}

interface DownloadProgressEvent {
  downloaded: number;
  total: number;
  percent: number;
}

export function SettingsPage() {
  const { theme, setTheme } = useTheme();

  // STT service state
  const [sttService, setSttService] = useState<"deepgram" | "whisper">(
    () => (localStorage.getItem("stt_service") as "deepgram" | "whisper") || "deepgram"
  );
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(null);
  const [isDownloading, setIsDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState(0);

  // Audio device state
  const [audioDevices, setAudioDevices] = useState<WpDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState<number | null>(() => {
    const stored = localStorage.getItem("selected_audio_device_id");
    return stored ? parseInt(stored, 10) : null;
  });

  // API keys
  const [apiKey, setApiKey] = useState(
    () => localStorage.getItem("deepgram_api_key") || ""
  );
  const [openRouterApiKey, setOpenRouterApiKey] = useState(
    () => localStorage.getItem("openrouter_api_key") || ""
  );

  // Toggles
  const [llmEnabled, setLlmEnabled] = useState(
    () => localStorage.getItem("llm_enabled") !== "false"
  );

  // Save settings to localStorage and sync with backend
  useEffect(() => {
    localStorage.setItem("deepgram_api_key", apiKey);
    if (apiKey.trim()) {
      invoke("set_api_key", { apiKey });
    }
  }, [apiKey]);

  useEffect(() => {
    localStorage.setItem("openrouter_api_key", openRouterApiKey);
    if (openRouterApiKey.trim()) {
      invoke("set_openrouter_api_key", { apiKey: openRouterApiKey });
    }
  }, [openRouterApiKey]);

  useEffect(() => {
    localStorage.setItem("stt_service", sttService);
    invoke("set_stt_service", { service: sttService });
  }, [sttService]);

  useEffect(() => {
    localStorage.setItem("llm_enabled", String(llmEnabled));
  }, [llmEnabled]);

  useEffect(() => {
    if (selectedDeviceId !== null) {
      localStorage.setItem("selected_audio_device_id", String(selectedDeviceId));
    } else {
      localStorage.removeItem("selected_audio_device_id");
    }
    invoke("set_selected_device", { deviceId: selectedDeviceId });
  }, [selectedDeviceId]);

  // Check if whisper model exists
  useEffect(() => {
    const checkModel = async () => {
      try {
        const exists = await invoke<boolean>("check_whisper_model");
        setWhisperModelExists(exists);
      } catch {
        setWhisperModelExists(false);
      }
    };
    checkModel();
  }, []);

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

  // Listen for download events
  useEffect(() => {
    const unlistenProgress = listen<DownloadProgressEvent>(
      "download-progress",
      (event) => {
        setDownloadProgress(event.payload.percent);
      }
    );

    const unlistenComplete = listen<boolean>("download-complete", () => {
      setIsDownloading(false);
      setDownloadProgress(100);
      setWhisperModelExists(true);
    });

    const unlistenError = listen<string>("download-error", (event) => {
      setIsDownloading(false);
      setDownloadProgress(0);
      alert(`Download failed: ${event.payload}`);
    });

    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
      unlistenError.then((fn) => fn());
    };
  }, []);

  const downloadWhisperModel = async () => {
    setIsDownloading(true);
    setDownloadProgress(0);
    try {
      await invoke("download_whisper_model");
    } catch (err) {
      setIsDownloading(false);
      alert(`Could not start download: ${err}`);
    }
  };

  const handleClose = () => {
    getCurrentWindow().close();
  };

  const handleDrag = () => getCurrentWindow().startDragging();

  return (
    <main className="flex flex-col h-screen w-screen bg-background">
      {/* Drag handle area */}
      <div
        className="absolute top-0 left-0 right-0 h-5 cursor-move z-50"
        onMouseDown={handleDrag}
      />

      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b">
        <div className="flex items-center gap-2">
          <Settings className="h-5 w-5 text-muted-foreground" />
          <h1 className="text-lg font-semibold">Settings</h1>
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8"
          onClick={handleClose}
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        <div className="space-y-6 max-w-md mx-auto">
          {/* STT Service */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-muted-foreground">
              STT Service
            </Label>
            <Tabs
              value={sttService}
              onValueChange={(v) => setSttService(v as "deepgram" | "whisper")}
              className="w-full"
            >
              <TabsList className="grid w-full grid-cols-2">
                <TabsTrigger value="deepgram">Deepgram</TabsTrigger>
                <TabsTrigger value="whisper">Whisper</TabsTrigger>
              </TabsList>
            </Tabs>

            {sttService === "whisper" && (
              <div className="mt-3">
                {whisperModelExists === null ? (
                  <span className="text-sm text-muted-foreground">
                    Checking model...
                  </span>
                ) : whisperModelExists ? (
                  <span className="text-sm text-green-500 font-medium">
                    Model ready
                  </span>
                ) : isDownloading ? (
                  <div className="space-y-2">
                    <Progress value={downloadProgress} />
                    <span className="text-xs text-muted-foreground">
                      {downloadProgress.toFixed(1)}%
                    </span>
                  </div>
                ) : (
                  <Button
                    onClick={downloadWhisperModel}
                    size="sm"
                    className="w-full"
                  >
                    <Download className="mr-2 h-4 w-4" />
                    Download Model
                  </Button>
                )}
              </div>
            )}
          </div>

          <Separator />

          {/* Audio Device */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-muted-foreground">
              Audio Device
            </Label>
            <Select
              value={selectedDeviceId?.toString() ?? "auto"}
              onValueChange={(v) =>
                setSelectedDeviceId(v === "auto" ? null : parseInt(v, 10))
              }
            >
              <SelectTrigger>
                <SelectValue placeholder="Auto-select" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="auto">Auto-select</SelectItem>
                {audioDevices.map((device) => (
                  <SelectItem key={device.id} value={device.id.toString()}>
                    {device.name}
                    {device.is_default ? " (default)" : ""}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <Separator />

          {/* API Keys */}
          <div className="space-y-4">
            <div className="space-y-2">
              <Label
                htmlFor="deepgram-key"
                className="text-xs uppercase tracking-wide text-muted-foreground"
              >
                Deepgram API Key
              </Label>
              <Input
                id="deepgram-key"
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="Enter API key"
              />
            </div>

            <div className="space-y-2">
              <Label
                htmlFor="openrouter-key"
                className="text-xs uppercase tracking-wide text-muted-foreground"
              >
                OpenRouter API Key
              </Label>
              <Input
                id="openrouter-key"
                type="password"
                value={openRouterApiKey}
                onChange={(e) => setOpenRouterApiKey(e.target.value)}
                placeholder="Enter API key"
              />
            </div>
          </div>

          <Separator />

          {/* Toggles */}
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <Label
                htmlFor="llm-processing"
                className="text-sm font-normal cursor-pointer"
              >
                LLM processing
              </Label>
              <Switch
                id="llm-processing"
                checked={llmEnabled}
                onCheckedChange={setLlmEnabled}
              />
            </div>
          </div>

          <Separator />

          {/* Theme */}
          <div className="flex items-center justify-between">
            <Label className="text-xs uppercase tracking-wide text-muted-foreground">
              Theme
            </Label>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="sm" className="gap-2">
                  {theme === "light" && <Sun className="h-4 w-4" />}
                  {theme === "dark" && <Moon className="h-4 w-4" />}
                  {theme === "system" && <Monitor className="h-4 w-4" />}
                  <span className="capitalize">{theme}</span>
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => setTheme("light")}>
                  <Sun className="mr-2 h-4 w-4" />
                  Light
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => setTheme("dark")}>
                  <Moon className="mr-2 h-4 w-4" />
                  Dark
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => setTheme("system")}>
                  <Monitor className="mr-2 h-4 w-4" />
                  System
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </div>
    </main>
  );
}
