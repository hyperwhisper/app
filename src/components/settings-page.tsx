import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { Settings, X, Download, Loader2, Trash2, Zap, Target, ChevronDown, ChevronUp } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { handOverBrowserSettings } from "@/lib/browser-settings";

interface AudioDevice {
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

// Everything Rust has saved. This window shows it and changes it; it does not
// keep its own copy.
interface Settings {
  active_local_model_id: string | null;
  selected_microphone: string | null;
}

export function SettingsPage() {
  // App version
  const [appVersion, setAppVersion] = useState<string>("");

  // Audio device state
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
  // The microphone by name. null means whichever one the system has set.
  const [microphone, setMicrophone] = useState<string | null>(null);
  const [deviceError, setDeviceError] = useState<string | null>(null);

  // Nothing is saved until the saved settings are on screen. Without this the
  // empty starting values below would be written over the real ones.
  const [loaded, setLoaded] = useState(false);

  // Multi-model state
  const [availableModels, setAvailableModels] = useState<ModelInfo[]>([]);
  const [activeModelId, setActiveModelId] = useState<string | null>(null);
  const [downloadingModels, setDownloadingModels] = useState<Set<string>>(new Set());
  const [downloadProgress, setDownloadProgress] = useState<Record<string, ModelDownloadProgress>>({});
  const [modelError, setModelError] = useState<string | null>(null);

  // The debug line. Read on its own below because the tray menu sets it too.
  const [showDebugStats, setShowDebugStats] = useState(false);

  // The dictation key. Rust owns it: it has to be registered at startup, long
  // before this window exists.
  const [shortcut, setShortcut] = useState("F3");
  const [capturing, setCapturing] = useState(false);
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [showAllModels, setShowAllModels] = useState(false);

  // Disable right-click context menu
  useEffect(() => {
    const handleContextMenu = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", handleContextMenu);
    return () => document.removeEventListener("contextmenu", handleContextMenu);
  }, []);

  // Hand the old browser-held settings to Rust before reading anything, so
  // what appears here is what Rust will use. Rust ignores the second attempt.
  useEffect(() => {
    (async () => {
      try {
        await handOverBrowserSettings();
      } catch (err) {
        console.error("Could not hand the old settings to Rust:", err);
      }
      try {
        const saved = await invoke<Settings>("get_settings");
        setActiveModelId(saved.active_local_model_id);
        setMicrophone(saved.selected_microphone);
      } catch (err) {
        console.error("Could not read the saved settings:", err);
      }
      setLoaded(true);
    })();
  }, []);

  // Save the chosen microphone
  useEffect(() => {
    if (!loaded) return;
    invoke("set_selected_device", { name: microphone });
  }, [loaded, microphone]);

  // Load available models on mount and when provider changes to local
  useEffect(() => {
    const loadModels = async () => {
      try {
        const models = await invoke<ModelInfo[]>("list_available_models");
        setAvailableModels(models);

        // The saved model may have been deleted since; fall back to one that
        // is still on disk rather than leaving the app with nothing to run.
        const active = await invoke<string | null>("get_active_model");
        const stillHere = models.find(m => m.id === active && m.status === "downloaded");
        if (stillHere) {
          setActiveModelId(stillHere.id);
        } else {
          const firstDownloaded = models.find(m => m.status === "downloaded");
          if (firstDownloaded) {
            setActiveModelId(firstDownloaded.id);
            invoke("set_active_model", { modelId: firstDownloaded.id });
          }
        }
      } catch (err) {
        console.error("Failed to load models:", err);
      }
    };
    if (loaded) {
      loadModels();
    }
  }, [loaded]);

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

  useEffect(() => {
    invoke<string>("get_shortcut").then(setShortcut).catch(() => {});
  }, []);

  // While capturing, the next key combination becomes the new shortcut. Written
  // the way Tauri parses it: "CommandOrControl+Shift+D".
  useEffect(() => {
    if (!capturing) return;

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        setCapturing(false);
        return;
      }

      // A modifier on its own is not a shortcut, so keep waiting for a real key.
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

      const parts: string[] = [];
      if (e.metaKey) parts.push("CommandOrControl");
      if (e.ctrlKey && !e.metaKey) parts.push("Control");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");

      // e.code is the physical key, so the combination does not change with the
      // keyboard layout. KeyA -> A, Digit1 -> 1, F3 -> F3.
      const code = e.code
        .replace(/^Key/, "")
        .replace(/^Digit/, "")
        .replace(/^Numpad/, "Num");
      parts.push(code);
      const accelerator = parts.join("+");

      setCapturing(false);
      setShortcutError(null);
      invoke("set_shortcut", { accelerator })
        .then(() => setShortcut(accelerator))
        .catch((err) => setShortcutError(String(err)));
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing]);

  // Read the current setting, then follow it if the tray changes it.
  useEffect(() => {
    invoke<boolean>("get_debug_stats").then(setShowDebugStats).catch(() => {});
    const unlisten = listen<boolean>("debug-stats-changed", (event) =>
      setShowDebugStats(event.payload)
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  // Load audio devices and app version
  useEffect(() => {
    const loadDevices = async () => {
      try {
        setAudioDevices(await invoke<AudioDevice[]>("list_audio_devices"));
        setDeviceError(null);
      } catch (err) {
        setDeviceError(String(err));
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
          invoke("set_active_model", { modelId: nextDownloaded.id });
        } else {
          setActiveModelId(null);
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
          {/* Dictation key. Above everything else: it works whichever backend
              is chosen, and it is the only way to use the app hands-free. */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Dictation key
            </Label>
            <div className="flex items-center justify-between p-3 bg-white/5 rounded-lg">
              <div>
                <span className="text-sm text-white font-mono">
                  {capturing ? "Press any key..." : shortcut}
                </span>
                <p className="text-xs text-white/40">
                  {capturing
                    ? "Escape to keep the current one"
                    : "Starts and stops dictation from any app"}
                </p>
              </div>
              <button
                onClick={() => {
                  setShortcutError(null);
                  setCapturing((on) => !on);
                }}
                className={`px-3 py-1.5 text-xs rounded-md transition-colors ${
                  capturing
                    ? "bg-white/20 text-white"
                    : "bg-white/10 text-white/80 hover:bg-white/20 hover:text-white"
                }`}
              >
                {capturing ? "Cancel" : "Change"}
              </button>
            </div>
            {shortcutError && (
              <p className="text-xs text-red-400">{shortcutError}</p>
            )}
          </div>

          {/* Microphone */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Microphone
            </Label>
            <Select
              value={microphone ?? "system"}
              onValueChange={(v) => setMicrophone(v === "system" ? null : v)}
            >
              <SelectTrigger className="bg-white/5 border-0 text-white">
                <SelectValue>
                  {microphone ?? "Whatever the system is using"}
                </SelectValue>
              </SelectTrigger>
              <SelectContent className="bg-neutral-800/95 backdrop-blur-xl border-0">
                <SelectItem value="system" className="text-white/80 focus:bg-white/10 focus:text-white">
                  Whatever the system is using
                </SelectItem>
                {audioDevices.map((device) => (
                  <SelectItem
                    key={device.name}
                    value={device.name}
                    className="text-white/80 focus:bg-white/10 focus:text-white"
                  >
                    {device.name}
                    {device.is_default && " (system default)"}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {/* A microphone that has been unplugged since it was chosen is
                still shown here, because that is what will be looked for. */}
            {microphone && !audioDevices.some((d) => d.name === microphone) && (
              <p className="text-xs text-amber-400">
                {microphone} is not connected. The system's own microphone will
                be used until it is plugged back in.
              </p>
            )}
            {deviceError && <p className="text-xs text-red-400">{deviceError}</p>}
          </div>

          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <Label className="text-xs uppercase tracking-wide text-white/50">
                Models
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

          {/* Diagnostics. Its own section: it has nothing to do with which
              backend is chosen, and it lived under the local-only settings
              where it vanished if you switched backend. */}
          <div className="space-y-2">
            <Label className="text-xs uppercase tracking-wide text-white/50">
              Diagnostics
            </Label>
            <div className="flex items-center justify-between p-3 bg-white/5 rounded-lg">
              <div>
                <span className="text-sm text-white">Show debug stats</span>
                <p className="text-xs text-white/40">Live microphone numbers, and one line per dictation</p>
              </div>
              <button
                onClick={() => {
                  const next = !showDebugStats;
                  setShowDebugStats(next);
                  invoke("set_debug_stats", { enabled: next }).catch(() => {});
                }}
                className={`w-10 h-5 rounded-full transition-colors ${
                  showDebugStats ? "bg-green-500" : "bg-white/20"
                }`}
              >
                <div
                  className={`w-4 h-4 rounded-full bg-white transition-transform ${
                    showDebugStats ? "translate-x-5" : "translate-x-0.5"
                  }`}
                />
              </button>
            </div>
          </div>

          {/* Version */}
          {appVersion && (
            <div className="pt-4 mt-4 border-t border-white/10">
              <p className="text-xs text-white/30 text-center">
                Omegawhisper v{appVersion}
              </p>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}
