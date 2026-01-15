import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { SettingsDialog } from "@/components/settings-dialog";

interface TranscriptionEvent {
  text: string;
  is_final: boolean;
}

function App() {
  const [finalText, setFinalText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [copied, setCopied] = useState(false);
  const [apiKey] = useState(
    () => localStorage.getItem("deepgram_api_key") || ""
  );

  // Audio device state
  const [selectedDeviceId] = useState<number | null>(() => {
    const stored = localStorage.getItem("selected_audio_device_id");
    return stored ? parseInt(stored, 10) : null;
  });

  const [autoTypeEnabled, setAutoTypeEnabled] = useState(
    () => localStorage.getItem("auto_type_enabled") === "true"
  );

  // Hyperwhisper server settings
  const [useHyperwhisperServer] = useState(
    () => localStorage.getItem("use_hyperwhisper_server") !== "false"
  );
  const [hyperwhisperServerUrl] = useState(
    () => localStorage.getItem("hyperwhisper_server_url") || "localhost:1323"
  );
  const [hyperwhisperServerHttps] = useState(
    () => localStorage.getItem("hyperwhisper_server_https") === "true"
  );
  const [hyperwhisperApiKey] = useState(
    () => localStorage.getItem("hyperwhisper_api_key") || ""
  );

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const animationRef = useRef<number | null>(null);
  const finalTextRef = useRef<string>("");
  const audioContextRef = useRef<AudioContext | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const microphoneRef = useRef<MediaStream | null>(null);

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

  useEffect(() => {
    localStorage.setItem("auto_type_enabled", String(autoTypeEnabled));
  }, [autoTypeEnabled]);

  // Sync Hyperwhisper server settings to backend on load
  useEffect(() => {
    invoke("set_hyperwhisper_server_settings", {
      useHyperwhisperServer,
      serverUrl: hyperwhisperServerUrl.trim() || "localhost:1323",
      useHttps: hyperwhisperServerHttps,
      apiKey: hyperwhisperApiKey.trim() || null,
    });
  }, [useHyperwhisperServer, hyperwhisperServerUrl, hyperwhisperServerHttps, hyperwhisperApiKey]);

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

  // Store handleRecord in a ref so the D-Bus listener always has the latest version
  const handleRecordRef = useRef<() => void>(() => {});

  // Listen for D-Bus toggle events (from global keyboard shortcut)
  useEffect(() => {
    const unlisten = listen("recording-toggled", () => {
      handleRecordRef.current();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Real-time waveform visualization during recording
  const startRealTimeWaveform = useCallback(async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      microphoneRef.current = stream;

      const audioContext = new AudioContext();
      audioContextRef.current = audioContext;
      const analyser = audioContext.createAnalyser();
      analyser.fftSize = 256;
      analyserRef.current = analyser;

      const source = audioContext.createMediaStreamSource(stream);
      source.connect(analyser);

      const dataArray = new Uint8Array(analyser.frequencyBinCount);

      const draw = () => {
        const canvas = canvasRef.current;
        if (!canvas) {
          console.log("No canvas");
          return;
        }
        if (!analyserRef.current) {
          console.log("No analyser");
          return;
        }

        const ctx = canvas.getContext("2d");
        if (!ctx) return;

        analyserRef.current.getByteFrequencyData(dataArray);

        const width = canvas.width;
        const height = canvas.height;

        ctx.clearRect(0, 0, width, height);

        const isDark = document.documentElement.classList.contains("dark");
        const barColor = isDark ? "hsl(186, 100%, 50%)" : "hsl(221, 83%, 53%)";

        // Draw waveform as slim bars centered vertically
        const barWidth = 2;
        const gap = 3;
        const totalBars = 48;
        const totalWidth = totalBars * (barWidth + gap) - gap;
        const startX = (width - totalWidth) / 2;
        const step = Math.floor(dataArray.length / totalBars);

        for (let i = 0; i < totalBars; i++) {
          const dataIndex = i * step;
          const value = dataArray[dataIndex] || 0;
          // Min height of 4px, max of 90% canvas height
          const minHeight = 4;
          const barHeight = Math.max(minHeight, (value / 255) * height * 0.9);
          const x = startX + i * (barWidth + gap);
          const y = (height - barHeight) / 2;

          ctx.fillStyle = barColor;
          ctx.fillRect(x, y, barWidth, barHeight);
        }

        animationRef.current = requestAnimationFrame(draw);
      };

      draw();
    } catch (err) {
      console.error("Error accessing microphone for waveform:", err);
    }
  }, [isRecording]);

  const stopRealTimeWaveform = useCallback(() => {
    if (animationRef.current) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }
    if (microphoneRef.current) {
      microphoneRef.current.getTracks().forEach((track) => track.stop());
      microphoneRef.current = null;
    }
    if (audioContextRef.current) {
      audioContextRef.current.close();
      audioContextRef.current = null;
    }
    analyserRef.current = null;

    // Clear canvas
    const canvas = canvasRef.current;
    if (canvas) {
      const ctx = canvas.getContext("2d");
      if (ctx) {
        ctx.clearRect(0, 0, canvas.width, canvas.height);
      }
    }
  }, []);

  // Start/stop real-time waveform when recording state changes
  useEffect(() => {
    if (isRecording) {
      startRealTimeWaveform();
    } else {
      stopRealTimeWaveform();
    }

    return () => {
      stopRealTimeWaveform();
    };
  }, [isRecording, startRealTimeWaveform, stopRealTimeWaveform]);

  // Start recording
  const startRecording = async () => {
    // Check for the appropriate API key based on server setting
    const currentUseHyperwhisper = localStorage.getItem("use_hyperwhisper_server") !== "false";
    if (currentUseHyperwhisper) {
      const currentHyperwhisperKey = localStorage.getItem("hyperwhisper_api_key") || "";
      if (!currentHyperwhisperKey.trim()) {
        alert("Please enter your Hyperwhisper API key first");
        return;
      }
    } else {
      const currentApiKey = localStorage.getItem("deepgram_api_key") || "";
      if (!currentApiKey.trim()) {
        alert("Please enter your Deepgram API key first");
        return;
      }
    }

    try {
      setFinalText("");

      await invoke("set_auto_type_transcription", { enabled: autoTypeEnabled });

      await invoke("start_recording");
      setIsRecording(true);
    } catch (err) {
      alert(`Could not start recording: ${err}`);
    }
  };

  // Stop recording
  const stopRecording = async () => {
    try {
      await invoke<string>("stop_recording");
      setIsRecording(false);
    } catch (err) {
      alert(`Could not stop recording: ${err}`);
      setIsRecording(false);
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

  // Keep ref updated for D-Bus listener
  handleRecordRef.current = handleRecord;

  const handleDrag = () => getCurrentWindow().startDragging();

  return (
    <main
      className="flex flex-col h-screen w-screen relative bg-neutral-800/80 backdrop-blur-2xl rounded-2xl shadow-xl overflow-hidden"
      onMouseDown={handleDrag}
    >
      {/* Waveform area */}
      <div className="flex-1 flex items-center justify-center px-8 relative">
        {isRecording ? (
          <div className="flex items-center justify-center gap-[3px] h-[60px] w-full">
            {[...Array(80)].map((_, i) => (
              <div
                key={i}
                className={`w-[2px] bg-white/70 rounded-full sb${(i % 16) + 1}`}
                style={{ height: '100%' }}
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
      </div>

      {/* Bottom bar */}
      <div className="h-14 bg-neutral-900/80 flex items-center justify-between px-4">
        {/* Left: Recording status */}
        <div className="flex items-center gap-2">
          {isRecording ? (
            <>
              <div className="w-4 h-4 rounded-sm bg-red-500 animate-pulse" />
              <span className="text-white font-medium text-sm">Recording</span>
            </>
          ) : (
            <>
              <SettingsDialog disabled={isRecording} />
              <span className="text-white/60 text-sm">Ready</span>
            </>
          )}
        </div>

        {/* Right: Controls */}
        <div className="flex items-center gap-3">
          {/* Auto-type toggle */}
          <button
            onMouseDown={(e) => {
              e.stopPropagation();
              e.preventDefault();
              setAutoTypeEnabled(!autoTypeEnabled);
            }}
            disabled={isRecording}
            className={`flex items-center gap-1.5 text-xs transition-colors ${
              isRecording
                ? "opacity-50 cursor-not-allowed"
                : "hover:text-white/80"
            } ${autoTypeEnabled ? "text-white/80" : "text-white/40"}`}
            title={autoTypeEnabled ? "Auto-type enabled" : "Auto-type disabled"}
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="2" y="4" width="20" height="16" rx="2"/>
              <path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10"/>
            </svg>
            <div className={`w-6 h-3 rounded-full transition-colors ${autoTypeEnabled ? "bg-green-500" : "bg-white/20"}`}>
              <div className={`w-2.5 h-2.5 rounded-full bg-white mt-[1px] transition-transform ${autoTypeEnabled ? "translate-x-3" : "translate-x-0.5"}`} />
            </div>
          </button>

          {isRecording ? (
            <button
              onMouseDown={(e) => {
                e.stopPropagation();
                e.preventDefault();
                handleRecord();
              }}
              className="p-2 text-white/80 hover:text-white hover:bg-white/10 rounded-md transition-colors"
              title="Stop recording"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="currentColor" stroke="none">
                <rect x="6" y="6" width="12" height="12" rx="1" />
              </svg>
            </button>
          ) : (
            <button
              onMouseDown={(e) => {
                e.stopPropagation();
                e.preventDefault();
                handleRecord();
              }}
              className="p-2 rounded-md transition-colors text-white/80 hover:text-white hover:bg-white/10"
              title="Start recording"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect x="9" y="2" width="6" height="11" rx="3" />
                <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
                <line x1="12" y1="19" x2="12" y2="22" />
              </svg>
            </button>
          )}
        </div>
      </div>
    </main>
  );
}

export default App;
