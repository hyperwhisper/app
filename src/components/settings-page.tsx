import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Settings, Download, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Progress } from "@/components/ui/progress";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
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

interface DownloadProgressEvent {
  downloaded: number;
  total: number;
  percent: number;
}

export function SettingsPage() {
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
          {/* STT Service */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              STT Service
            </Label>
            <Tabs
              value={sttService}
              onValueChange={(v) => setSttService(v as "deepgram" | "whisper")}
              className="w-full"
            >
              <TabsList className="grid w-full grid-cols-2 bg-white/5 border-0">
                <TabsTrigger value="deepgram" className="data-[state=active]:bg-white/15 data-[state=active]:text-white text-white/50 border-0">Deepgram</TabsTrigger>
                <TabsTrigger value="whisper" className="data-[state=active]:bg-white/15 data-[state=active]:text-white text-white/50 border-0">Whisper</TabsTrigger>
              </TabsList>
            </Tabs>

            {sttService === "whisper" && (
              <div className="mt-3">
                {whisperModelExists === null ? (
                  <span className="text-sm text-white/50">
                    Checking model...
                  </span>
                ) : whisperModelExists ? (
                  <span className="text-sm text-green-400 font-medium">
                    Model ready
                  </span>
                ) : isDownloading ? (
                  <div className="space-y-2">
                    <Progress value={downloadProgress} className="bg-white/5" />
                    <span className="text-xs text-white/50">
                      {downloadProgress.toFixed(1)}%
                    </span>
                  </div>
                ) : (
                  <Button
                    onClick={downloadWhisperModel}
                    size="sm"
                    className="w-full bg-white/10 hover:bg-white/15 text-white border-0"
                  >
                    <Download className="mr-2 h-4 w-4" />
                    Download Model
                  </Button>
                )}
              </div>
            )}
          </div>

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

          {/* API Keys */}
          <div className="space-y-3">
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

            <div className="space-y-2">
              <Label
                htmlFor="openrouter-key"
                className="text-xs uppercase tracking-wide text-white/50"
              >
                OpenRouter API Key
              </Label>
              <Input
                id="openrouter-key"
                type="password"
                value={openRouterApiKey}
                onChange={(e) => setOpenRouterApiKey(e.target.value)}
                placeholder="Enter API key"
                className="bg-white/5 border-0 text-white placeholder:text-white/30"
              />
            </div>
          </div>

          {/* Toggles */}
          <div className="flex items-center justify-between py-1">
            <Label
              htmlFor="llm-processing"
              className="text-sm font-normal cursor-pointer text-white/80"
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
      </div>
    </main>
  );
}
