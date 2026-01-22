import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Settings, X, Download, Loader2, Trash2, Zap, Target, ChevronDown, ChevronUp } from "lucide-react";
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

interface ModelInfo {
  id: string;
  name: string;
  description: string;
  engine_type: string;
  total_size_bytes: number;
  accuracy_score: number;
  speed_score: number;
  status: string;
}

interface ModelDownloadProgress {
  model_id: string;
  file: string;
  progress: number;
  total_files: number;
  current_file: number;
  bytes_downloaded: number;
  total_bytes: number;
}

type TranscriptionProvider = "hyperwhisper" | "deepgram" | "local";

export function SettingsPage() {
  // App version
  const [appVersion, setAppVersion] = useState<string>("");

  // Audio device state
  const [audioDevices, setAudioDevices] = useState<WpDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState<number | null>(() => {
    const stored = localStorage.getItem("selected_audio_device_id");
    return stored ? parseInt(stored, 10) : null;
  });

  // Transcription provider (tri-state)
  const [provider, setProvider] = useState<TranscriptionProvider>(() => {
    const stored = localStorage.getItem("transcription_provider");
    if (stored === "local" || stored === "deepgram" || stored === "hyperwhisper") {
      return stored;
    }
    // Migration: check old useHyperwhisperServer flag
    const oldFlag = localStorage.getItem("use_hyperwhisper_server");
    return oldFlag === "false" ? "deepgram" : "hyperwhisper";
  });

  // Hyperwhisper Server settings
  const [hyperwhisperServerUrl, setHyperwhisperServerUrl] = useState(
    () => localStorage.getItem("hyperwhisper_server_url") || "hyperwhisper.dev"
  );
  const [hyperwhisperServerHttps, setHyperwhisperServerHttps] = useState(
    () => localStorage.getItem("hyperwhisper_server_https") !== "false"
  );
  const [hyperwhisperApiKey, setHyperwhisperApiKey] = useState(
    () => localStorage.getItem("hyperwhisper_api_key") || ""
  );
  const [showHyperwhisperApiKey, setShowHyperwhisperApiKey] = useState(false);

  // Deepgram API key
  const [apiKey, setApiKey] = useState(
    () => localStorage.getItem("deepgram_api_key") || ""
  );
  const [showApiKey, setShowApiKey] = useState(false);

  // Multi-model state
  const [availableModels, setAvailableModels] = useState<ModelInfo[]>([]);
  const [activeModelId, setActiveModelId] = useState<string | null>(() =>
    localStorage.getItem("active_local_model_id")
  );
  const [downloadingModels, setDownloadingModels] = useState<Set<string>>(new Set());
  const [downloadProgress, setDownloadProgress] = useState<Record<string, ModelDownloadProgress>>({});
  const [modelError, setModelError] = useState<string | null>(null);
  const [useVad, setUseVad] = useState(() =>
    localStorage.getItem("use_vad") !== "false"
  );
  const [showAllModels, setShowAllModels] = useState(false);

  // Appearance settings
  const [barPosition, setBarPosition] = useState<'top' | 'bottom'>(() =>
    (localStorage.getItem("bar_position") as 'top' | 'bottom') || 'bottom'
  );

  // Disable right-click context menu
  useEffect(() => {
    const handleContextMenu = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", handleContextMenu);
    return () => document.removeEventListener("contextmenu", handleContextMenu);
  }, []);

  // Save Deepgram API key
  useEffect(() => {
    localStorage.setItem("deepgram_api_key", apiKey);
    if (apiKey.trim()) {
      invoke("set_api_key", { apiKey });
    }
  }, [apiKey]);

  // Save provider settings
  useEffect(() => {
    localStorage.setItem("transcription_provider", provider);
    // Also update old flags for backward compatibility
    localStorage.setItem("use_hyperwhisper_server", String(provider === "hyperwhisper"));

    // Update Rust state based on provider
    if (provider === "local") {
      invoke("set_use_local_transcription", { enabled: true });
      invoke("set_hyperwhisper_server_settings", {
        useHyperwhisperServer: false,
        serverUrl: hyperwhisperServerUrl.trim() || "hyperwhisper.dev",
        useHttps: hyperwhisperServerHttps,
        apiKey: null,
      });
    } else {
      invoke("set_use_local_transcription", { enabled: false });
      invoke("set_hyperwhisper_server_settings", {
        useHyperwhisperServer: provider === "hyperwhisper",
        serverUrl: hyperwhisperServerUrl.trim() || "hyperwhisper.dev",
        useHttps: hyperwhisperServerHttps,
        apiKey: provider === "hyperwhisper" ? (hyperwhisperApiKey.trim() || null) : null,
      });
    }

    // Notify main window that settings changed
    emit("settings-changed");
  }, [provider, hyperwhisperServerUrl, hyperwhisperServerHttps, hyperwhisperApiKey]);

  // Save Hyperwhisper Server settings to localStorage
  useEffect(() => {
    localStorage.setItem("hyperwhisper_server_url", hyperwhisperServerUrl);
    localStorage.setItem("hyperwhisper_server_https", String(hyperwhisperServerHttps));
    localStorage.setItem("hyperwhisper_api_key", hyperwhisperApiKey);
  }, [hyperwhisperServerUrl, hyperwhisperServerHttps, hyperwhisperApiKey]);

  // Save selected device
  useEffect(() => {
    if (selectedDeviceId !== null) {
      localStorage.setItem("selected_audio_device_id", String(selectedDeviceId));
    } else {
      localStorage.removeItem("selected_audio_device_id");
    }
    invoke("set_selected_device", { deviceId: selectedDeviceId });
  }, [selectedDeviceId]);

  // Load available models on mount and when provider changes to local
  useEffect(() => {
    const loadModels = async () => {
      try {
        const models = await invoke<ModelInfo[]>("list_available_models");
        setAvailableModels(models);

        // If we have an active model set, verify it's still valid
        const active = await invoke<string | null>("get_active_model");
        if (active) {
          setActiveModelId(active);
          localStorage.setItem("active_local_model_id", active);
        } else if (activeModelId) {
          // Try to restore from localStorage
          const downloaded = models.find(m => m.id === activeModelId && m.status === "downloaded");
          if (downloaded) {
            invoke("set_active_model", { modelId: activeModelId });
          } else {
            // Find first downloaded model
            const firstDownloaded = models.find(m => m.status === "downloaded");
            if (firstDownloaded) {
              setActiveModelId(firstDownloaded.id);
              localStorage.setItem("active_local_model_id", firstDownloaded.id);
              invoke("set_active_model", { modelId: firstDownloaded.id });
            }
          }
        }
      } catch (err) {
        console.error("Failed to load models:", err);
      }
    };
    if (provider === "local") {
      loadModels();
    }
  }, [provider]);

  // Listen for model download progress events
  useEffect(() => {
    const unlisten = listen<ModelDownloadProgress>("model-download-progress", (event) => {
      setDownloadProgress(prev => ({
        ...prev,
        [event.payload.model_id]: event.payload
      }));

      // If download is complete (all files at 100%), refresh model list
      if (event.payload.progress >= 100 && event.payload.current_file === event.payload.total_files) {
        setTimeout(async () => {
          const models = await invoke<ModelInfo[]>("list_available_models");
          setAvailableModels(models);
          setDownloadingModels(prev => {
            const next = new Set(prev);
            next.delete(event.payload.model_id);
            return next;
          });
        }, 500);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Save VAD setting
  useEffect(() => {
    localStorage.setItem("use_vad", String(useVad));
    invoke("set_use_vad", { enabled: useVad });
  }, [useVad]);

  // Save bar position setting
  useEffect(() => {
    localStorage.setItem("bar_position", barPosition);
    emit("settings-changed");
  }, [barPosition]);

  // Load audio devices and app version
  useEffect(() => {
    const loadDevices = async () => {
      try {
        const devices = await invoke<WpDevice[]>("list_audio_devices");
        setAudioDevices(devices);

        // GNOME bug workaround: When a Bluetooth microphone is selected as the default
        // audio source, it can crash the entire desktop environment. To avoid this,
        // we explicitly select the Built-in Microphone instead of using "auto" (default).
        const selectBuiltInMic = () => {
          const builtInMic = devices.find((d) => d.name === "Built-in Microphone");
          if (builtInMic) {
            setSelectedDeviceId(builtInMic.id);
            localStorage.setItem("selected_audio_device_id", String(builtInMic.id));
          }
        };

        if (selectedDeviceId === null) {
          // No device selected yet - default to Built-in Microphone
          selectBuiltInMic();
        } else {
          // Check if selected device still exists
          const deviceExists = devices.some((d) => d.id === selectedDeviceId);
          if (!deviceExists) {
            selectBuiltInMic();
          }
        }
      } catch (err) {
        console.error("Failed to load audio devices:", err);
      }
    };
    const loadVersion = async () => {
      try {
        const version = await getVersion();
        setAppVersion(version);
      } catch (err) {
        console.error("Failed to get app version:", err);
      }
    };
    loadDevices();
    loadVersion();
  }, []);

  const handleClose = () => {
    getCurrentWindow().close();
  };

  const handleDrag = () => getCurrentWindow().startDragging();

  const handleDownloadModel = async (modelId: string) => {
    setDownloadingModels(prev => new Set(prev).add(modelId));
    setModelError(null);
    try {
      await invoke("download_model", { modelId });
      // Refresh models list after download
      const models = await invoke<ModelInfo[]>("list_available_models");
      setAvailableModels(models);

      // Auto-select this model if none selected
      if (!activeModelId) {
        setActiveModelId(modelId);
        localStorage.setItem("active_local_model_id", modelId);
        invoke("set_active_model", { modelId });
      }
    } catch (err) {
      console.error("Failed to download model:", err);
      setModelError(String(err));
    } finally {
      setDownloadingModels(prev => {
        const next = new Set(prev);
        next.delete(modelId);
        return next;
      });
    }
  };

  const handleDeleteModel = async (modelId: string) => {
    try {
      await invoke("delete_model", { modelId });
      // Refresh models list
      const models = await invoke<ModelInfo[]>("list_available_models");
      setAvailableModels(models);

      // If this was the active model, select another
      if (activeModelId === modelId) {
        const nextDownloaded = models.find(m => m.status === "downloaded" && m.id !== modelId);
        if (nextDownloaded) {
          setActiveModelId(nextDownloaded.id);
          localStorage.setItem("active_local_model_id", nextDownloaded.id);
          invoke("set_active_model", { modelId: nextDownloaded.id });
        } else {
          setActiveModelId(null);
          localStorage.removeItem("active_local_model_id");
        }
      }
    } catch (err) {
      console.error("Failed to delete model:", err);
      setModelError(String(err));
    }
  };

  const handleSelectModel = async (modelId: string) => {
    try {
      await invoke("set_active_model", { modelId });
      setActiveModelId(modelId);
      localStorage.setItem("active_local_model_id", modelId);
    } catch (err) {
      console.error("Failed to select model:", err);
      setModelError(String(err));
    }
  };

  const formatSize = (bytes: number) => {
    if (bytes >= 1_000_000_000) {
      return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
    }
    return `${Math.round(bytes / 1_000_000)} MB`;
  };

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
          {/* Microphone */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Microphone
            </Label>
            <Select
              value={selectedDeviceId !== null ? selectedDeviceId.toString() : "auto"}
              onValueChange={(v) =>
                setSelectedDeviceId(v === "auto" ? null : parseInt(v, 10))
              }
            >
              <SelectTrigger className="bg-white/5 border-0 text-white">
                <SelectValue>
                  {selectedDeviceId !== null
                    ? audioDevices.find((d) => d.id === selectedDeviceId)?.name ?? "Loading..."
                    : "Default microphone"}
                </SelectValue>
              </SelectTrigger>
              <SelectContent className="bg-neutral-800/95 backdrop-blur-xl border-0">
                <SelectItem value="auto" className="text-white/80 focus:bg-white/10 focus:text-white">
                  Default microphone
                </SelectItem>
                {audioDevices.map((device) => (
                  <SelectItem
                    key={device.id}
                    value={device.id.toString()}
                    className="text-white/80 focus:bg-white/10 focus:text-white"
                  >
                    {device.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-white/30">
              Select which microphone to use for recording
            </p>
          </div>

          {/* Service Selection - 3 buttons */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Transcription Service
            </Label>
            <div className="flex gap-2">
              <button
                onClick={() => setProvider("hyperwhisper")}
                className={`flex-1 py-2 px-3 rounded-lg text-sm transition-colors ${
                  provider === "hyperwhisper"
                    ? "bg-white/15 text-white"
                    : "bg-white/5 text-white/50 hover:bg-white/10"
                }`}
              >
                Hyperwhisper
              </button>
              <button
                onClick={() => setProvider("deepgram")}
                className={`flex-1 py-2 px-3 rounded-lg text-sm transition-colors ${
                  provider === "deepgram"
                    ? "bg-white/15 text-white"
                    : "bg-white/5 text-white/50 hover:bg-white/10"
                }`}
              >
                Deepgram
              </button>
              <button
                onClick={() => setProvider("local")}
                className={`flex-1 py-2 px-3 rounded-lg text-sm transition-colors ${
                  provider === "local"
                    ? "bg-white/15 text-white"
                    : "bg-white/5 text-white/50 hover:bg-white/10"
                }`}
              >
                Local
              </button>
            </div>
          </div>

          {/* Hyperwhisper Server settings - shown when using Hyperwhisper */}
          {provider === "hyperwhisper" && (
            <>
              {/* Server URL */}
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <Label
                    htmlFor="hyperwhisper-url"
                    className="text-xs uppercase tracking-wide text-white/50"
                  >
                    Server URL
                  </Label>
                  <button
                    type="button"
                    onClick={() => {
                      const url = hyperwhisperServerUrl.trim() || "hyperwhisper.dev";
                      const protocol = hyperwhisperServerHttps ? "https" : "http";
                      openUrl(`${protocol}://${url}`);
                    }}
                    className="text-xs text-white/50 hover:text-white/80 transition-colors underline"
                  >
                    Visit website
                  </button>
                </div>
                <div className="flex gap-2">
                  <button
                    onClick={() => setHyperwhisperServerHttps(false)}
                    className={`py-2 px-3 rounded-lg text-sm transition-colors ${
                      !hyperwhisperServerHttps
                        ? "bg-white/15 text-white"
                        : "bg-white/5 text-white/50 hover:bg-white/10"
                    }`}
                  >
                    http://
                  </button>
                  <button
                    onClick={() => setHyperwhisperServerHttps(true)}
                    className={`py-2 px-3 rounded-lg text-sm transition-colors ${
                      hyperwhisperServerHttps
                        ? "bg-white/15 text-white"
                        : "bg-white/5 text-white/50 hover:bg-white/10"
                    }`}
                  >
                    https://
                  </button>
                  <Input
                    id="hyperwhisper-url"
                    type="text"
                    value={hyperwhisperServerUrl}
                    onChange={(e) => setHyperwhisperServerUrl(e.target.value)}
                    placeholder="hyperwhisper.dev"
                    className="flex-1 bg-white/5 border-0 text-white placeholder:text-white/30"
                  />
                </div>
              </div>

              {/* Hyperwhisper API Key */}
              <div className="space-y-2">
                <Label
                  htmlFor="hyperwhisper-key"
                  className="text-xs uppercase tracking-wide text-white/50"
                >
                  API Key
                </Label>
                <div className="relative">
                  <Input
                    id="hyperwhisper-key"
                    type={showHyperwhisperApiKey ? "text" : "password"}
                    value={hyperwhisperApiKey}
                    onChange={(e) => setHyperwhisperApiKey(e.target.value)}
                    placeholder="Enter your API key"
                    className="bg-white/5 border-0 text-white placeholder:text-white/30 pr-10"
                  />
                  <button
                    type="button"
                    onClick={() => setShowHyperwhisperApiKey(!showHyperwhisperApiKey)}
                    className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-white/40 hover:text-white/80 transition-colors"
                    title={showHyperwhisperApiKey ? "Hide API key" : "Show API key"}
                  >
                    {showHyperwhisperApiKey ? (
                      <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
                        <line x1="1" y1="1" x2="23" y2="23"/>
                      </svg>
                    ) : (
                      <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                        <circle cx="12" cy="12" r="3"/>
                      </svg>
                    )}
                  </button>
                </div>
              </div>

              {/* Privacy note */}
              <p className="text-xs text-white/30">
                No audio or transcription data is stored on our servers.
              </p>
            </>
          )}

          {/* Deepgram API Key - shown when using Deepgram */}
          {provider === "deepgram" && (
            <div className="space-y-2">
              <Label
                htmlFor="deepgram-key"
                className="text-xs uppercase tracking-wide text-white/50"
              >
                Deepgram API Key
              </Label>
              <div className="relative">
                <Input
                  id="deepgram-key"
                  type={showApiKey ? "text" : "password"}
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="Enter your API key"
                  className="bg-white/5 border-0 text-white placeholder:text-white/30 pr-10"
                />
                <button
                  type="button"
                  onClick={() => setShowApiKey(!showApiKey)}
                  className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-white/40 hover:text-white/80 transition-colors"
                  title={showApiKey ? "Hide API key" : "Show API key"}
                >
                  {showApiKey ? (
                    <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>
                      <line x1="1" y1="1" x2="23" y2="23"/>
                    </svg>
                  ) : (
                    <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                      <circle cx="12" cy="12" r="3"/>
                    </svg>
                  )}
                </button>
              </div>
              <p className="text-xs text-white/30">
                Get your free API key at <span className="text-white/50">deepgram.com</span>
              </p>
            </div>
          )}

          {/* Local transcription settings - shown when using Local */}
          {provider === "local" && (
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <Label className="text-xs uppercase tracking-wide text-white/50">
                  Local Models
                </Label>
                <button
                  onClick={() => setShowAllModels(!showAllModels)}
                  className="text-xs text-white/50 hover:text-white/80 flex items-center gap-1"
                >
                  {showAllModels ? "Show less" : "Show all"}
                  {showAllModels ? <ChevronUp className="h-3 w-3" /> : <ChevronDown className="h-3 w-3" />}
                </button>
              </div>

              {/* Model list */}
              <div className="space-y-2">
                {availableModels
                  .filter(model => showAllModels || model.status === "downloaded" || ["moonshine-base", "parakeet-v3-int8", "whisper-small"].includes(model.id))
                  .map(model => {
                    const isDownloading = downloadingModels.has(model.id);
                    const progress = downloadProgress[model.id];
                    const isActive = activeModelId === model.id;
                    const isDownloaded = model.status === "downloaded";

                    return (
                      <div
                        key={model.id}
                        className={`p-3 rounded-lg transition-colors ${
                          isActive
                            ? "bg-white/15 ring-1 ring-white/20"
                            : "bg-white/5 hover:bg-white/10"
                        }`}
                      >
                        <div className="flex items-start justify-between gap-2">
                          <div className="flex-1 min-w-0">
                            <div className="flex items-center gap-2">
                              <span className="text-sm font-medium text-white truncate">
                                {model.name}
                              </span>
                              <span className="text-xs text-white/40 shrink-0">
                                {formatSize(model.total_size_bytes)}
                              </span>
                              {isActive && (
                                <span className="text-xs bg-green-500/20 text-green-400 px-1.5 py-0.5 rounded shrink-0">
                                  Active
                                </span>
                              )}
                            </div>
                            <p className="text-xs text-white/40 mt-0.5 truncate">
                              {model.description}
                            </p>
                            {/* Speed/Accuracy indicators */}
                            <div className="flex items-center gap-3 mt-1.5">
                              <div className="flex items-center gap-1">
                                <Zap className="h-3 w-3 text-yellow-400/70" />
                                <div className="w-12 h-1 bg-white/10 rounded-full overflow-hidden">
                                  <div
                                    className="h-full bg-yellow-400/70"
                                    style={{ width: `${model.speed_score * 100}%` }}
                                  />
                                </div>
                              </div>
                              <div className="flex items-center gap-1">
                                <Target className="h-3 w-3 text-blue-400/70" />
                                <div className="w-12 h-1 bg-white/10 rounded-full overflow-hidden">
                                  <div
                                    className="h-full bg-blue-400/70"
                                    style={{ width: `${model.accuracy_score * 100}%` }}
                                  />
                                </div>
                              </div>
                            </div>
                          </div>

                          {/* Action buttons */}
                          <div className="flex items-center gap-1 shrink-0">
                            {isDownloading ? (
                              <div className="flex items-center gap-2">
                                <Loader2 className="h-4 w-4 text-white/60 animate-spin" />
                                <span className="text-xs text-white/60">
                                  {progress ? `${Math.round(progress.progress)}%` : "..."}
                                </span>
                              </div>
                            ) : isDownloaded ? (
                              <>
                                {!isActive && (
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    onClick={() => handleSelectModel(model.id)}
                                    className="h-7 px-2 text-xs text-white/60 hover:text-white hover:bg-white/10"
                                  >
                                    Use
                                  </Button>
                                )}
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  onClick={() => handleDeleteModel(model.id)}
                                  className="h-7 w-7 p-0 text-white/40 hover:text-red-400 hover:bg-red-400/10"
                                >
                                  <Trash2 className="h-3.5 w-3.5" />
                                </Button>
                              </>
                            ) : (
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => handleDownloadModel(model.id)}
                                className="h-7 px-2 text-xs text-white/60 hover:text-white hover:bg-white/10"
                              >
                                <Download className="h-3.5 w-3.5 mr-1" />
                                Download
                              </Button>
                            )}
                          </div>
                        </div>

                        {/* Download progress bar */}
                        {isDownloading && progress && (
                          <div className="mt-2">
                            <div className="h-1 bg-white/10 rounded-full overflow-hidden">
                              <div
                                className="h-full bg-white/60 transition-all duration-300"
                                style={{
                                  width: `${((progress.current_file - 1) / progress.total_files * 100) + (progress.progress / progress.total_files)}%`
                                }}
                              />
                            </div>
                            <p className="text-xs text-white/30 mt-1">
                              {progress.file} ({progress.current_file}/{progress.total_files})
                            </p>
                          </div>
                        )}
                      </div>
                    );
                  })}
              </div>

              {/* VAD toggle */}
              <div className="flex items-center justify-between p-3 bg-white/5 rounded-lg">
                <div>
                  <span className="text-sm text-white">Voice Activity Detection</span>
                  <p className="text-xs text-white/40">Filter silence for better accuracy</p>
                </div>
                <button
                  onClick={() => setUseVad(!useVad)}
                  className={`w-10 h-5 rounded-full transition-colors ${
                    useVad ? "bg-green-500" : "bg-white/20"
                  }`}
                >
                  <div
                    className={`w-4 h-4 rounded-full bg-white transition-transform ${
                      useVad ? "translate-x-5" : "translate-x-0.5"
                    }`}
                  />
                </button>
              </div>

              {/* Error message */}
              {modelError && (
                <p className="text-xs text-red-400">
                  Error: {modelError}
                </p>
              )}

              {/* Info */}
              <p className="text-xs text-white/30">
                Works offline. Whisper models support all languages; Parakeet is English only.
              </p>
            </div>
          )}

          {/* Appearance */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Appearance
            </Label>
            <div className="flex items-center justify-between p-3 bg-white/5 rounded-lg">
              <div>
                <span className="text-sm text-white">Control Bar Position</span>
                <p className="text-xs text-white/40">Show controls at top or bottom</p>
              </div>
              <div className="flex gap-1">
                <button
                  onClick={() => setBarPosition('top')}
                  className={`px-3 py-1.5 text-xs rounded transition-colors ${
                    barPosition === 'top'
                      ? "bg-white/15 text-white"
                      : "bg-white/5 text-white/50 hover:bg-white/10"
                  }`}
                >
                  Top
                </button>
                <button
                  onClick={() => setBarPosition('bottom')}
                  className={`px-3 py-1.5 text-xs rounded transition-colors ${
                    barPosition === 'bottom'
                      ? "bg-white/15 text-white"
                      : "bg-white/5 text-white/50 hover:bg-white/10"
                  }`}
                >
                  Bottom
                </button>
              </div>
            </div>
          </div>

          {/* Version */}
          {appVersion && (
            <div className="pt-4 mt-4 border-t border-white/10">
              <p className="text-xs text-white/30 text-center">
                HyperWhisper v{appVersion}
              </p>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}
