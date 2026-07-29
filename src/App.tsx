import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { SettingsDialog } from "@/components/settings-dialog";
import { useTrialKey } from "@/hooks/use-trial-key";
import { db } from "@/lib/audio-level";

// Parse keybinding from dconf format (e.g., "<Super>m") to readable format (e.g., "Super+m")
function formatKeybinding(binding: string): string {
  // Extract modifiers and key from format like "<Super><Ctrl>m" or "<Super>m"
  const modifiers: string[] = [];
  let key = binding;

  // Extract all <Modifier> patterns
  const modifierRegex = /<([^>]+)>/g;
  let match;
  while ((match = modifierRegex.exec(binding)) !== null) {
    modifiers.push(match[1]);
  }

  // The remaining part after all modifiers is the key
  key = binding.replace(modifierRegex, '').trim();

  // Join with + separator
  return [...modifiers, key].join('+');
}

interface TranscriptionEvent {
  text: string;
  is_final: boolean;
}

// Numbers behind one finished local dictation, sent by Rust after each run.
interface DictationStats {
  model: string;
  seconds: number;
  level_before: number;
  level_after: number;
  gain: number;
  took: number;
  chars: number;
}

function App() {
  const [finalText, setFinalText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [copied, setCopied] = useState(false);
  const [stats, setStats] = useState<DictationStats | null>(null);
  const [showStats, setShowStats] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [apiKey] = useState(
    () => localStorage.getItem("deepgram_api_key") || ""
  );

  // Trial key management
  const { state: trialState, isInitializing: isTrialInitializing, refresh: refreshTrial } = useTrialKey();

  // Audio device state
  const [selectedDeviceId] = useState<number | null>(() => {
    const stored = localStorage.getItem("selected_audio_device_id");
    return stored ? parseInt(stored, 10) : null;
  });

  const [autoTypeEnabled, setAutoTypeEnabled] = useState(
    () => localStorage.getItem("auto_type_enabled") === "true"
  );

  // Keybinding display
  const [keybinding, setKeybinding] = useState<string | null>(null);

  // Bar position (top or bottom)
  const [barPosition, setBarPosition] = useState<'top' | 'bottom'>(() =>
    (localStorage.getItem("bar_position") as 'top' | 'bottom') || 'bottom'
  );

  // Toast notification state
  const [toast, setToast] = useState<{ message: string; type: 'error' | 'info' } | null>(null);
  const toastTimeoutRef = useRef<number | null>(null);

  const showToast = useCallback(
    (message: string, type: 'error' | 'info' = 'error', ms = 5000) => {
      if (toastTimeoutRef.current) {
        clearTimeout(toastTimeoutRef.current);
      }
      setToast({ message, type });
      toastTimeoutRef.current = window.setTimeout(() => {
        setToast(null);
      }, ms);
    },
    []
  );

  // Anything Rust found wrong at startup - a shortcut that would not register,
  // a missing permission. Asked for rather than pushed, because Rust finds
  // these before this window exists to hear them.
  useEffect(() => {
    invoke<string[]>("get_startup_warnings")
      .then((warnings) => {
        if (warnings.length > 0) showToast(warnings.join(" "), "error", 15000);
      })
      .catch(() => {});
  }, [showToast]);

  // Note: Hyperwhisper server settings are managed by useTrialKey hook

  const finalTextRef = useRef<string>("");

  // The recording bars. Written straight into the DOM rather than held in
  // React state: the levels arrive 20 times a second, and re-rendering the
  // whole window that often is wasted work, most of it while it is hidden.
  const BAR_COUNT = 48;
  const barsRef = useRef<(HTMLDivElement | null)[]>([]);

  // Keep ref in sync with state for use in callbacks
  useEffect(() => {
    finalTextRef.current = finalText;
  }, [finalText]);

  // Save API key to localStorage and send to backend
  useEffect(() => {
    localStorage.setItem("deepgram_api_key", apiKey);
    if (apiKey.trim()) {
      invoke("set_api_key", { apiKey });
    }
  }, [apiKey]);

  // Sync local transcription settings to backend on startup
  useEffect(() => {
    const syncLocalTranscriptionSettings = async () => {
      const provider = localStorage.getItem("transcription_provider") ||
        (localStorage.getItem("use_hyperwhisper_server") !== "false" ? "hyperwhisper" : "deepgram");

      const useLocal = provider === "local";
      await invoke("set_use_local_transcription", { enabled: useLocal });

      if (useLocal) {
        // Also sync the active model ID
        const activeModelId = localStorage.getItem("active_local_model_id");
        if (activeModelId) {
          try {
            await invoke("set_active_model", { modelId: activeModelId });
          } catch (e) {
            console.error("Failed to set active model on startup:", e);
          }
        }

        // Sync VAD setting
        const useVad = localStorage.getItem("use_vad") !== "false";
        await invoke("set_use_vad", { enabled: useVad });
      }
    };

    syncLocalTranscriptionSettings();
  }, []);

  useEffect(() => {
    localStorage.setItem("auto_type_enabled", String(autoTypeEnabled));
  }, [autoTypeEnabled]);

  // Listen for settings changes (from settings window)
  useEffect(() => {
    const unlisten = listen("settings-changed", () => {
      // Re-read bar position from localStorage
      const newPosition = (localStorage.getItem("bar_position") as 'top' | 'bottom') || 'bottom';
      setBarPosition(newPosition);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Fetch keybinding from dconf on startup
  useEffect(() => {
    invoke<string | null>("get_keybinding").then((binding) => {
      if (binding) {
        setKeybinding(binding);
      }
    }).catch(() => {
      // dconf not available or no keybinding found
    });
  }, []);

  // Note: Hyperwhisper server settings are synced via useTrialKey hook which handles auto-provisioning

  // Listen for settings changes from the settings window and refresh trial state
  useEffect(() => {
    const unlisten = listen("settings-changed", () => {
      refreshTrial();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshTrial]);

  // Save selected device to localStorage and sync with backend
  useEffect(() => {
    if (selectedDeviceId !== null) {
      localStorage.setItem(
        "selected_audio_device_id",
        String(selectedDeviceId)
      );
    } else {
      localStorage.removeItem("selected_audio_device_id");
    }
    invoke("set_selected_device", { deviceId: selectedDeviceId });
  }, [selectedDeviceId]);

  // Disable right-click context menu
  useEffect(() => {
    const handleContextMenu = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", handleContextMenu);
    return () => document.removeEventListener("contextmenu", handleContextMenu);
  }, []);

  // Listen for transcription events from backend
  useEffect(() => {
    const unlisten = listen<TranscriptionEvent>("transcription", (event) => {
      const { text, is_final } = event.payload;

      if (is_final && text) {
        setFinalText((prev) => prev + (prev ? " " : "") + text);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Play a soft two-note chime. Both cues use this, with different notes:
  // recording starts on a low descending pair, text arrives on a higher
  // ascending pair, so the two are easy to tell apart without being sharp.
  const playChime = useCallback((notes: { frequency: number; delay: number; duration: number }[]) => {
    const audioCtx = new AudioContext();

    const playNote = (frequency: number, startTime: number, duration: number) => {
      const oscillator = audioCtx.createOscillator();
      const gainNode = audioCtx.createGain();

      oscillator.connect(gainNode);
      gainNode.connect(audioCtx.destination);

      // Use a soft sine wave
      oscillator.type = "sine";
      oscillator.frequency.value = frequency;

      // Gentle envelope: quick fade in, long slow fade out
      const peakTime = startTime + 0.03;
      const endTime = startTime + duration;

      gainNode.gain.setValueAtTime(0, startTime);
      gainNode.gain.linearRampToValueAtTime(0.12, peakTime);
      // Use linear ramp to near-zero for smoother fade (exponential can't go to 0)
      gainNode.gain.setValueAtTime(0.12, peakTime);
      gainNode.gain.linearRampToValueAtTime(0.001, endTime);

      oscillator.start(startTime);
      oscillator.stop(endTime + 0.1);
    };

    const now = audioCtx.currentTime;
    notes.forEach((n) => playNote(n.frequency, now + n.delay, n.duration));

    // Clean up after tones finish
    setTimeout(() => audioCtx.close(), 1200);
  }, []);

  // Recording started: G4 down to C4, a calm descending fifth in a lower
  // register. Kept short so it does not sit on top of your first words.
  const playStartTone = useCallback(() => {
    playChime([
      { frequency: 392.0, delay: 0, duration: 0.3 },
      { frequency: 261.63, delay: 0.08, duration: 0.42 },
    ]);
  }, [playChime]);

  // Text is ready: C5 up to E5, a calming ascending major third.
  const playCompletionTone = useCallback(() => {
    playChime([
      { frequency: 523.25, delay: 0, duration: 0.4 },
      { frequency: 659.25, delay: 0.08, duration: 0.52 },
    ]);
  }, [playChime]);

  // Listen for transcription processing state (grace period after recording stops)
  useEffect(() => {
    const unlistenProcessing = listen("transcription-processing", () => {
      setIsProcessing(true);
    });

    const unlistenComplete = listen("transcription-complete", () => {
      setIsProcessing(false);
      playCompletionTone();
    });

    return () => {
      unlistenProcessing.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
    };
  }, [playCompletionTone]);

  // What the last dictation did, shown as one line under the text.
  useEffect(() => {
    const unlisten = listen<DictationStats>("dictation-stats", (event) => {
      setStats(event.payload);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Typing into the other app was refused. Put the text in the clipboard so a
  // long dictation is still one paste away, and say so.
  useEffect(() => {
    const unlisten = listen<string>("auto-type-failed", (event) => {
      navigator.clipboard
        .writeText(event.payload)
        .then(() =>
          showToast("Typing is blocked. Text copied - press Cmd+V.", "info")
        )
        .catch(() => {});
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [showToast]);

  // Confirmation that the tray's Delete Recordings actually did something.
  useEffect(() => {
    const unlisten = listen<number>("recordings-deleted", (event) => {
      const n = event.payload;
      showToast(
        n === 1 ? "1 recording deleted." : `${n} recordings deleted.`,
        "info"
      );
    });

    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [showToast]);

  // The debug stats line is off unless switched on in the tray menu.
  useEffect(() => {
    invoke<boolean>("get_debug_stats")
      .then(setShowStats)
      .catch(() => {});
    const unlisten = listen<boolean>("debug-stats-changed", (event) => {
      setShowStats(event.payload);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Ask Rust whether it is recording. Rust owns that answer; the window only
  // mirrors it. Guessing here is what left the two disagreeing for good.
  const syncRecordingState = useCallback(() => {
    invoke<boolean>("is_recording").then(setIsRecording).catch(() => {});
  }, []);

  // Listen for transcription errors (e.g., auth failure, connection issues)
  useEffect(() => {
    const unlisten = listen<string>("transcription-error", (event) => {
      const errorMsg = event.payload;
      setConnectionError(errorMsg);
      // This error can belong to the previous dictation and arrive after the
      // next recording has already started. Turning the flag off here stopped
      // the shortcut working until the app was restarted.
      syncRecordingState();
      // Auto-clear error after 10 seconds
      setTimeout(() => setConnectionError(null), 10000);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [syncRecordingState]);

  // Store handleRecord in a ref so the D-Bus listener always has the latest version
  const handleRecordRef = useRef<() => void>(() => {});

  // Toggle recording from the global shortcut event.
  useEffect(() => {
    const unlisten = listen("recording-toggled", () => {
      handleRecordRef.current();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Show the spectrogram indicator window while recording or processing.
  useEffect(() => {
    const shouldShow = isRecording || isProcessing;
    (async () => {
      await emit("indicator-active", shouldShow);
      const indicator = await WebviewWindow.getByLabel("indicator");
      if (!indicator) return;
      if (shouldShow) {
        // Re-centre first: the screen it belongs on may have been added,
        // removed or resized since the last time it was shown.
        await invoke("position_indicator");
        await indicator.show();
      } else {
        await indicator.hide();
      }
    })().catch(() => {});
  }, [isRecording, isProcessing]);

  // The bars while recording come from Rust, from the audio being recorded.
  // Neither window may open the microphone itself: WebKit ignores the "raw
  // stream" constraints, which puts the device into processed mode and makes
  // macOS wind its gain up over the first seconds of every recording.
  useEffect(() => {
    const unlisten = listen<{ bands: number[] }>("mic-level", (event) => {
      const bands = event.payload.bands ?? [];
      // Each bar is 4% to 100% tall.
      for (let i = 0; i < BAR_COUNT; i++) {
        const bar = barsRef.current[i];
        if (!bar) continue;
        const value = bands[Math.floor((i * bands.length) / BAR_COUNT)] ?? 0;
        bar.style.height = `${Math.max(4, value * 100)}%`;
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Start recording
  const startRecording = async () => {
    // Check for the appropriate API key based on provider setting
    const provider = localStorage.getItem("transcription_provider") ||
      (localStorage.getItem("use_hyperwhisper_server") !== "false" ? "hyperwhisper" : "deepgram");

    if (provider === "local") {
      // Local transcription doesn't need API key, but needs model
      // The Rust backend will check if model is available
    } else if (provider === "hyperwhisper") {
      // When using Hyperwhisper server, check trial key status (unless user has their own key)
      if (trialState.status === "loading" || isTrialInitializing) {
        showToast("Please wait, initializing...", "info");
        return;
      }

      if (trialState.status === "error") {
        // Check if user has an API key despite the error
        const currentHyperwhisperKey = localStorage.getItem("hyperwhisper_api_key") || "";
        if (!currentHyperwhisperKey.trim()) {
          showToast(`Connection error: ${trialState.error}`);
          return;
        }
        // User has their own key, allow recording
      } else if (trialState.status === "quota_exceeded") {
        const upgradeUrl = trialState.info.upgrade_url || "https://hyperwhisper.dev/signup";
        showToast(`Trial quota exceeded. Please upgrade at: ${upgradeUrl}`);
        return;
      } else if (trialState.status === "expired") {
        const upgradeUrl = trialState.info.upgrade_url || "https://hyperwhisper.dev/signup";
        showToast(`Trial expired. Please upgrade at: ${upgradeUrl}`);
        return;
      } else if (trialState.status === "no_key") {
        showToast("No API key available. Please restart the app or enter an API key in settings.");
        return;
      }
      // trialState.status is "active" or "has_api_key" - both are good to proceed

      const currentHyperwhisperKey = localStorage.getItem("hyperwhisper_api_key") || "";
      if (!currentHyperwhisperKey.trim()) {
        showToast("Please enter your Hyperwhisper API key first");
        return;
      }
    } else {
      // Deepgram
      const currentApiKey = localStorage.getItem("deepgram_api_key") || "";
      if (!currentApiKey.trim()) {
        showToast("Please enter your Deepgram API key first");
        return;
      }
    }

    try {
      // The text from earlier dictations stays. It used to be wiped here, so
      // starting a second recording erased what the first one had produced -
      // and if that second one came back empty, the text was gone for good.
      // Use the clear button to empty the box.
      await invoke("set_auto_type_transcription", { enabled: autoTypeEnabled });

      // Clear any previous error
      setConnectionError(null);

      await invoke("start_recording");
      setIsRecording(true);
      playStartTone();
    } catch (err) {
      showToast(`Could not start recording: ${err}`);
      syncRecordingState();
    }
  };

  // Stop recording
  const stopRecording = async () => {
    try {
      await invoke<string>("stop_recording");
      setIsRecording(false);
      // Refresh trial status after recording to update usage (only for Hyperwhisper)
      const provider = localStorage.getItem("transcription_provider") ||
        (localStorage.getItem("use_hyperwhisper_server") !== "false" ? "hyperwhisper" : "deepgram");
      if (provider === "hyperwhisper") {
        refreshTrial();
      }
    } catch (err) {
      showToast(`Could not stop recording: ${err}`);
      syncRecordingState();
    }
  };

  // Toggle recording
  const handleRecord = () => {
    if (isRecording) {
      stopRecording();
    } else {
      startRecording();
    }
  };

  // Updated after each render, not during one: assigning while rendering can
  // leave the shortcut holding a stale copy that toggles the wrong way.
  useEffect(() => {
    handleRecordRef.current = handleRecord;
  });

  const handleDrag = () => getCurrentWindow().startDragging();

  return (
    <main
      className="flex flex-col h-screen w-screen relative bg-neutral-800/80 backdrop-blur-2xl rounded-2xl shadow-xl overflow-hidden"
      onMouseDown={handleDrag}
    >
      {/* Hide the window and go back to being a menu-bar app. There is no
          title bar on this window, so without this there is no way to put it
          away with the mouse. */}
      <button
        onMouseDown={(e) => {
          e.stopPropagation();
          e.preventDefault();
          invoke("hide_main_window").catch(() => {});
        }}
        title="Hide window"
        aria-label="Hide window"
        className="absolute top-2 right-2 z-50 p-1.5 rounded-md text-white/35 hover:text-white hover:bg-white/10 transition-colors"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </button>

      {/* Toast notification */}
      {toast && (
        <div
          className={`absolute top-3 left-1/2 -translate-x-1/2 px-4 py-2 rounded-lg text-sm z-50 flex items-center gap-2 shadow-lg ${
            toast.type === 'error' ? 'bg-red-500/90 text-white' : 'bg-neutral-600/90 text-white'
          }`}
        >
          {toast.type === 'error' && (
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="10"/>
              <line x1="12" y1="8" x2="12" y2="12"/>
              <line x1="12" y1="16" x2="12.01" y2="16"/>
            </svg>
          )}
          <span>{toast.message}</span>
          <button
            onMouseDown={(e) => {
              e.stopPropagation();
              e.preventDefault();
              setToast(null);
            }}
            className="ml-2 text-white/60 hover:text-white"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="18" y1="6" x2="6" y2="18"/>
              <line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>
      )}

      {/* Control bar - shown at top when barPosition is 'top' */}
      {barPosition === 'top' && (
        <div className="h-14 bg-neutral-900/80 flex items-center justify-between px-4">
          {/* Left: Recording status */}
          <div className="flex items-center gap-2">
            {isRecording ? (
              <>
                <div className="w-4 h-4 rounded-full bg-red-500 animate-pulse" />
                <span className="text-white font-medium text-sm">Recording</span>
              </>
            ) : isProcessing ? (
              <>
                <div className="w-4 h-4 rounded-full bg-amber-500 animate-pulse" />
                <span className="text-white/80 text-sm">Processing...</span>
              </>
            ) : isTrialInitializing ? (
              <>
                <div className="w-4 h-4 rounded-full bg-blue-500 animate-pulse" />
                <span className="text-white/60 text-sm">Initializing...</span>
              </>
            ) : (
              <>
                <SettingsDialog disabled={isRecording || isProcessing} tooltipPosition="bottom" />
                <span className="text-white/60 text-sm">Ready</span>
              </>
            )}
          </div>

          {/* Right: Controls */}
          <div className="flex items-center gap-3">
            {/* Auto-type toggle */}
            <div className="relative group">
              <button
                onMouseDown={(e) => {
                  e.stopPropagation();
                  e.preventDefault();
                  setAutoTypeEnabled(!autoTypeEnabled);
                }}
                disabled={isRecording || isProcessing}
                className={`flex items-center gap-1.5 text-xs transition-colors ${
                  isRecording || isProcessing
                    ? "opacity-50 cursor-not-allowed"
                    : "hover:text-white/80"
                } ${autoTypeEnabled ? "text-white/80" : "text-white/40"}`}
              >
                <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="2" y="4" width="20" height="16" rx="2"/>
                  <path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10"/>
                </svg>
                <div className={`w-6 h-3 rounded-full transition-colors ${autoTypeEnabled ? "bg-green-500" : "bg-white/20"}`}>
                  <div className={`w-2.5 h-2.5 rounded-full bg-white mt-[1px] transition-transform ${autoTypeEnabled ? "translate-x-3" : "translate-x-0.5"}`} />
                </div>
              </button>
              <div className="absolute top-full left-1/2 -translate-x-1/2 mt-2 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none">
                {autoTypeEnabled ? "Auto-type enabled" : "Auto-type disabled"}
              </div>
            </div>

            {isRecording ? (
              <div className="relative group">
                <button
                  onMouseDown={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    handleRecord();
                  }}
                  className="p-2 text-white/80 hover:text-white hover:bg-white/10 rounded-md transition-colors"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="currentColor" stroke="none">
                    <rect x="6" y="6" width="12" height="12" rx="1" />
                  </svg>
                </button>
                <div className="absolute top-full right-0 mt-2 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none">
                  Stop recording
                </div>
              </div>
            ) : (
              <div className="relative group">
                <button
                  onMouseDown={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    if (!isProcessing) handleRecord();
                  }}
                  disabled={isProcessing}
                  className={`p-2 rounded-md transition-colors ${
                    isProcessing
                      ? "text-white/30 cursor-not-allowed"
                      : "text-white/80 hover:text-white hover:bg-white/10"
                  }`}
                >
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <rect x="9" y="2" width="6" height="11" rx="3" />
                    <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                    <line x1="12" y1="19" x2="12" y2="22" />
                  </svg>
                </button>
                <div className="absolute top-full right-0 mt-2 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none flex flex-col items-center">
                  <span>{isProcessing ? "Processing..." : "Start recording"}</span>
                  {keybinding && (
                    <span className="text-white/50">{formatKeybinding(keybinding)}</span>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Waveform area */}
      <div className="flex-1 flex items-center justify-center px-8 relative">
        {connectionError ? (
          <div className="px-4 w-full max-h-[100px] overflow-y-auto">
            <div className="flex items-start justify-start gap-2">
              <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-red-400 flex-shrink-0 mt-0.5">
                <circle cx="12" cy="12" r="10"/>
                <line x1="12" y1="8" x2="12" y2="12"/>
                <line x1="12" y1="16" x2="12.01" y2="16"/>
              </svg>
              {/* The message itself. It used to be replaced with one of four
                  fixed phrases, so everything Rust explains about the
                  microphone arrived as the words "Connection error". */}
              <p className="text-sm text-red-400 text-left leading-relaxed">
                {connectionError}
              </p>
            </div>
            <button
              onMouseDown={(e) => {
                e.stopPropagation();
                e.preventDefault();
                setConnectionError(null);
              }}
              className="mt-2 mx-auto block text-xs text-white/40 hover:text-white/80 transition-colors"
            >
              Dismiss
            </button>
          </div>
        ) : isRecording ? (
          <div className="flex items-center justify-center gap-[1px] h-[60px] w-full">
            {Array.from({ length: BAR_COUNT }, (_, i) => (
              <div
                key={i}
                ref={(el) => {
                  barsRef.current[i] = el;
                }}
                className="w-[2px] bg-white/70 rounded-full transition-all duration-75"
                style={{ height: "4%" }}
              />
            ))}
          </div>
        ) : finalText ? (
          <div className="px-4 w-full max-h-[100px] overflow-y-auto">
            <p className="text-sm text-white/80 text-center leading-relaxed">
              {finalText}
            </p>
          </div>
        ) : (
          <div className="flex items-center justify-center gap-[3px] h-[60px] w-full opacity-30">
            {[...Array(80)].map((_, i) => (
              <div
                key={i}
                className="w-[2px] bg-white/50 rounded-full"
                style={{ height: `${4 + (i % 3) * 2}px` }}
              />
            ))}
          </div>
        )}
        {/* What the last dictation did. Kept on screen because the useful
            case is the one where no text came back at all. */}
        {showStats && stats && !isRecording && (
          <div
            className={`absolute bottom-2 left-4 text-[10px] font-mono ${
              stats.chars === 0 ? "text-red-400/70" : "text-white/35"
            }`}
            title={`${stats.model}, level before boost ${db(
              stats.level_before
            )} dB`}
          >
            {stats.seconds.toFixed(1)}s · {db(stats.level_after)} dB · gain{" "}
            {stats.gain.toFixed(1)}x · {stats.took.toFixed(1)}s ·{" "}
            {stats.chars} chars
          </div>
        )}
        {/* Copy button - bottom right of waveform area */}
        {finalText && !isRecording && (
          <button
            onMouseDown={(e) => {
              e.stopPropagation();
              e.preventDefault();
              navigator.clipboard.writeText(finalText).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              });
            }}
            className={`absolute bottom-2 right-4 p-1 transition-colors text-xs flex items-center gap-1 ${
              copied ? "text-green-400" : "text-white/40 hover:text-white/80"
            }`}
            title="Copy to clipboard"
          >
            {copied ? (
              <span>Copied!</span>
            ) : (
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
                <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
              </svg>
            )}
          </button>
        )}
        {/* Empty the box. Dictations pile up until this is pressed. */}
        {finalText && !isRecording && (
          <button
            onMouseDown={(e) => {
              e.stopPropagation();
              e.preventDefault();
              setFinalText("");
            }}
            className="absolute bottom-2 right-12 p-1 text-white/40 hover:text-white/80 transition-colors"
            title="Clear the text"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M3 6h18" />
              <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
              <path d="M10 11v6M14 11v6" />
            </svg>
          </button>
        )}
      </div>

      {/* Control bar - shown at bottom when barPosition is 'bottom' */}
      {barPosition === 'bottom' && (
        <div className="h-14 bg-neutral-900/80 flex items-center justify-between px-4">
          {/* Left: Recording status */}
          <div className="flex items-center gap-2">
            {isRecording ? (
              <>
                <div className="w-4 h-4 rounded-full bg-red-500 animate-pulse" />
                <span className="text-white font-medium text-sm">Recording</span>
              </>
            ) : isProcessing ? (
              <>
                <div className="w-4 h-4 rounded-full bg-amber-500 animate-pulse" />
                <span className="text-white/80 text-sm">Processing...</span>
              </>
            ) : isTrialInitializing ? (
              <>
                <div className="w-4 h-4 rounded-full bg-blue-500 animate-pulse" />
                <span className="text-white/60 text-sm">Initializing...</span>
              </>
            ) : (
              <>
                <SettingsDialog disabled={isRecording || isProcessing} tooltipPosition="top" />
                <span className="text-white/60 text-sm">Ready</span>
              </>
            )}
          </div>

          {/* Right: Controls */}
          <div className="flex items-center gap-3">
            {/* Auto-type toggle */}
            <div className="relative group">
              <button
                onMouseDown={(e) => {
                  e.stopPropagation();
                  e.preventDefault();
                  setAutoTypeEnabled(!autoTypeEnabled);
                }}
                disabled={isRecording || isProcessing}
                className={`flex items-center gap-1.5 text-xs transition-colors ${
                  isRecording || isProcessing
                    ? "opacity-50 cursor-not-allowed"
                    : "hover:text-white/80"
                } ${autoTypeEnabled ? "text-white/80" : "text-white/40"}`}
              >
                <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="2" y="4" width="20" height="16" rx="2"/>
                  <path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10"/>
                </svg>
                <div className={`w-6 h-3 rounded-full transition-colors ${autoTypeEnabled ? "bg-green-500" : "bg-white/20"}`}>
                  <div className={`w-2.5 h-2.5 rounded-full bg-white mt-[1px] transition-transform ${autoTypeEnabled ? "translate-x-3" : "translate-x-0.5"}`} />
                </div>
              </button>
              <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none">
                {autoTypeEnabled ? "Auto-type enabled" : "Auto-type disabled"}
              </div>
            </div>

            {isRecording ? (
              <div className="relative group">
                <button
                  onMouseDown={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    handleRecord();
                  }}
                  className="p-2 text-white/80 hover:text-white hover:bg-white/10 rounded-md transition-colors"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="currentColor" stroke="none">
                    <rect x="6" y="6" width="12" height="12" rx="1" />
                  </svg>
                </button>
                <div className="absolute bottom-full right-0 mb-2 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none">
                  Stop recording
                </div>
              </div>
            ) : (
              <div className="relative group">
                <button
                  onMouseDown={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    if (!isProcessing) handleRecord();
                  }}
                  disabled={isProcessing}
                  className={`p-2 rounded-md transition-colors ${
                    isProcessing
                      ? "text-white/30 cursor-not-allowed"
                      : "text-white/80 hover:text-white hover:bg-white/10"
                  }`}
                >
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <rect x="9" y="2" width="6" height="11" rx="3" />
                    <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                    <line x1="12" y1="19" x2="12" y2="22" />
                  </svg>
                </button>
                <div className="absolute bottom-full right-0 mb-2 px-2 py-1.5 bg-neutral-700 text-white/80 text-xs rounded opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap pointer-events-none flex flex-col items-center">
                  <span>{isProcessing ? "Processing..." : "Start recording"}</span>
                  {keybinding && (
                    <span className="text-white/50">{formatKeybinding(keybinding)}</span>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </main>
  );
}

export default App;
