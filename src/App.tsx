import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

type Theme = "light" | "dark" | "system";
type SttService = "deepgram" | "whisper";

interface TranscriptionEvent {
  text: string;
  is_final: boolean;
}

interface LlmResponseEvent {
  text: string;
  is_error: boolean;
}

interface DownloadProgressEvent {
  downloaded: number;
  total: number;
  percent: number;
}

const DEFAULT_PROMPT = "You are a helpful assistant. Process the following transcription and provide a refined response:";

function App() {
  const [finalText, setFinalText] = useState("");
  const [theme, setTheme] = useState<Theme>("system");
  const [isRecording, setIsRecording] = useState(false);
  const [apiKey, setApiKey] = useState(() => localStorage.getItem("deepgram_api_key") || "");

  // STT service state
  const [sttService, setSttService] = useState<SttService>(() =>
    (localStorage.getItem("stt_service") as SttService) || "deepgram"
  );
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(null);
  const [isDownloading, setIsDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState(0);

  // LLM state
  const [llmPrompt] = useState(() => localStorage.getItem("llm_prompt") || DEFAULT_PROMPT);
  const [openRouterApiKey, setOpenRouterApiKey] = useState(() => localStorage.getItem("openrouter_api_key") || "");
  const [typeOutput] = useState<"transcription" | "llm">(() =>
    (localStorage.getItem("type_output") as "transcription" | "llm") || "transcription"
  );
  const [llmEnabled, setLlmEnabled] = useState(() => localStorage.getItem("llm_enabled") !== "false");
  const [realTimeTypingEnabled, setRealTimeTypingEnabled] = useState(() =>
    localStorage.getItem("realtime_typing_enabled") !== "false"
  );

  // Settings panel visibility
  const [showSettings, setShowSettings] = useState(false);

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

  // Save LLM prompt and OpenRouter API key to localStorage
  useEffect(() => {
    localStorage.setItem("llm_prompt", llmPrompt);
    invoke("set_llm_prompt", { prompt: llmPrompt });
  }, [llmPrompt]);

  useEffect(() => {
    localStorage.setItem("openrouter_api_key", openRouterApiKey);
    if (openRouterApiKey.trim()) {
      invoke("set_openrouter_api_key", { apiKey: openRouterApiKey });
    }
  }, [openRouterApiKey]);

  useEffect(() => {
    localStorage.setItem("type_output", typeOutput);
  }, [typeOutput]);

  useEffect(() => {
    localStorage.setItem("llm_enabled", String(llmEnabled));
  }, [llmEnabled]);

  useEffect(() => {
    localStorage.setItem("realtime_typing_enabled", String(realTimeTypingEnabled));
  }, [realTimeTypingEnabled]);

  // Save STT service to localStorage and sync with backend
  useEffect(() => {
    localStorage.setItem("stt_service", sttService);
    invoke("set_stt_service", { service: sttService });
  }, [sttService]);

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

  // Listen for download events
  useEffect(() => {
    const unlistenProgress = listen<DownloadProgressEvent>("download-progress", (event) => {
      setDownloadProgress(event.payload.percent);
    });

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

  // Track if we need to process after Whisper completes
  const pendingWhisperProcessingRef = useRef(false);

  // Listen for whisper processing events
  useEffect(() => {
    const unlisten = listen<boolean>("whisper-processing", (event) => {
      // When Whisper processing completes, handle typing and LLM
      if (!event.payload && pendingWhisperProcessingRef.current) {
        pendingWhisperProcessingRef.current = false;

        // Small delay to ensure state is updated
        setTimeout(async () => {
          const textToType = finalTextRef.current;
          if (textToType) {
            // Type transcription if that's what user selected
            if (localStorage.getItem("type_output") === "transcription") {
              try {
                await invoke("type_text", { text: textToType });
              } catch (err) {
                console.error("Failed to type text:", err);
              }
            }
            // Process with LLM via Rust backend (if enabled)
            const storedOpenRouterKey = localStorage.getItem("openrouter_api_key");
            const isLlmEnabled = localStorage.getItem("llm_enabled") !== "false";
            if (isLlmEnabled && storedOpenRouterKey?.trim()) {
              try {
                await invoke("process_with_llm", {
                  transcription: textToType,
                  shouldType: localStorage.getItem("type_output") === "llm"
                });
              } catch (err) {
                console.error("Failed to process with LLM:", err);
              }
            }
          }
        }, 100);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
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

  // Listen for LLM events from backend
  useEffect(() => {
    const unlistenResponse = listen<LlmResponseEvent>("llm-response", () => {
      // LLM processing complete
    });

    return () => {
      unlistenResponse.then((fn) => fn());
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

  // Theme handling
  useEffect(() => {
    const root = document.documentElement;
    let intervalId: number | undefined;

    const applyTheme = async () => {
      if (theme === "system") {
        const systemTheme = await invoke<string>("get_system_theme");
        root.setAttribute("data-theme", systemTheme);
      } else {
        root.setAttribute("data-theme", theme);
      }
    };

    applyTheme();

    if (theme === "system") {
      intervalId = window.setInterval(applyTheme, 5000);
    }

    return () => {
      if (intervalId) clearInterval(intervalId);
    };
  }, [theme]);

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
        if (!canvas || !analyserRef.current) return;

        const ctx = canvas.getContext("2d");
        if (!ctx) return;

        analyserRef.current.getByteFrequencyData(dataArray);

        const width = canvas.width;
        const height = canvas.height;

        ctx.clearRect(0, 0, width, height);

        const isDark = document.documentElement.getAttribute("data-theme") === "dark";
        const barColor = isDark ? "#24c8db" : "#396cd8";

        // Draw waveform as bars
        const barWidth = 3;
        const gap = 2;
        const totalBars = Math.floor(width / (barWidth + gap));
        const step = Math.floor(dataArray.length / totalBars);

        for (let i = 0; i < totalBars; i++) {
          const dataIndex = i * step;
          const value = dataArray[dataIndex] || 0;
          const barHeight = (value / 255) * height * 0.8;
          const x = i * (barWidth + gap);
          const y = (height - barHeight) / 2;

          ctx.fillStyle = barColor;
          ctx.fillRect(x, y, barWidth, barHeight);
        }

        if (isRecording) {
          animationRef.current = requestAnimationFrame(draw);
        }
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
      microphoneRef.current.getTracks().forEach(track => track.stop());
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

  // Download whisper model
  const downloadWhisperModel = async () => {
    setIsDownloading(true);
    setDownloadProgress(0);
    try {
      await invoke("download_whisper_model");
    } catch (err: any) {
      setIsDownloading(false);
      alert(`Could not start download: ${err}`);
    }
  };

  // Start recording
  const startRecording = async () => {
    if (sttService === "deepgram" && !apiKey.trim()) {
      alert("Please enter your Deepgram API key first");
      return;
    }

    if (sttService === "whisper" && !whisperModelExists) {
      alert("Please download the Whisper model first");
      return;
    }

    try {
      setFinalText("");

      // Enable real-time typing for Deepgram when enabled and typeOutput is "transcription"
      const shouldAutoType = sttService === "deepgram" && typeOutput === "transcription" && realTimeTypingEnabled;
      await invoke("set_auto_type_transcription", { enabled: shouldAutoType });

      await invoke("start_recording");
      setIsRecording(true);
    } catch (err: any) {
      alert(`Could not start recording: ${err}`);
    }
  };

  // Stop recording
  const stopRecording = async () => {
    try {
      await invoke<string>("stop_recording");
      setIsRecording(false);

      // For Whisper mode, set flag to process when transcription completes
      if (sttService === "whisper") {
        pendingWhisperProcessingRef.current = true;
      } else {
        // For Deepgram, wait briefly for any final transcription events, then process
        setTimeout(async () => {
          const textToType = finalTextRef.current;
          if (textToType) {
            // Type transcription after stop if real-time typing was disabled
            if (typeOutput === "transcription" && !realTimeTypingEnabled) {
              try {
                await invoke("type_text", { text: textToType });
              } catch (err) {
                console.error("Failed to type text:", err);
              }
            }
            // Process with LLM via Rust backend (will type if typeOutput is "llm")
            if (llmEnabled && openRouterApiKey.trim()) {
              try {
                await invoke("process_with_llm", {
                  transcription: textToType,
                  shouldType: typeOutput === "llm"
                });
              } catch (err) {
                console.error("Failed to process with LLM:", err);
              }
            }
          }
        }, 200);
      }
    } catch (err: any) {
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

  function cycleTheme() {
    setTheme((prev) => {
      if (prev === "system") return "light";
      if (prev === "light") return "dark";
      return "system";
    });
  }

  const handleDrag = () => getCurrentWindow().startDragging();

  return (
    <main className="container" onContextMenu={(e) => { e.preventDefault(); setShowSettings(!showSettings); }}>
      {/* Drag handle area */}
      <div className="drag-area" onMouseDown={handleDrag} />

      {/* Title */}
      <div className="title-bar">
        <span className="title-text">hyperwhisper</span>
      </div>

      {/* Waveform canvas */}
      <canvas
        ref={canvasRef}
        width={600}
        height={40}
        className="waveform-canvas"
      />

      {/* Controls */}
      <div className="controls">
        <button
          className={`record-btn ${isRecording ? "recording" : ""}`}
          onClick={isRecording ? stopRecording : startRecording}
        >
          <div className="record-icon">
            {isRecording ? (
              <div className="stop-icon" />
            ) : (
              <div className="mic-icon">
                <div className="mic-body" />
                <div className="mic-stand" />
              </div>
            )}
          </div>
        </button>
      </div>

      {/* Settings panel - accessible via right-click */}
      {showSettings && !isRecording && (
        <div className="settings-panel">
          <div className="settings-section">
            <label>STT Service</label>
            <div className="toggle-group">
              <button
                className={`toggle-btn ${sttService === "deepgram" ? "active" : ""}`}
                onClick={() => setSttService("deepgram")}
              >
                Deepgram
              </button>
              <button
                className={`toggle-btn ${sttService === "whisper" ? "active" : ""}`}
                onClick={() => setSttService("whisper")}
              >
                Whisper
              </button>
            </div>
          </div>

          {sttService === "whisper" && (
            <div className="settings-section">
              {whisperModelExists === null ? (
                <span>Checking model...</span>
              ) : whisperModelExists ? (
                <span className="status-ready">Model ready</span>
              ) : isDownloading ? (
                <div className="download-progress">
                  <div className="progress-bar">
                    <div className="progress-fill" style={{ width: `${downloadProgress}%` }} />
                  </div>
                  <span>{downloadProgress.toFixed(1)}%</span>
                </div>
              ) : (
                <button className="download-btn" onClick={downloadWhisperModel}>
                  Download Model
                </button>
              )}
            </div>
          )}

          <div className="settings-section">
            <label>Deepgram API Key</label>
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="Enter API key"
              className="input-field"
            />
          </div>

          <div className="settings-section">
            <label>OpenRouter API Key</label>
            <input
              type="password"
              value={openRouterApiKey}
              onChange={(e) => setOpenRouterApiKey(e.target.value)}
              placeholder="Enter API key"
              className="input-field"
            />
          </div>

          <div className="settings-section">
            <label>Theme</label>
            <button className="theme-btn" onClick={cycleTheme}>
              {theme === "system" ? "System" : theme === "light" ? "Light" : "Dark"}
            </button>
          </div>

          <div className="settings-section">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={realTimeTypingEnabled}
                onChange={(e) => setRealTimeTypingEnabled(e.target.checked)}
              />
              Real-time typing
            </label>
          </div>

          <div className="settings-section">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={llmEnabled}
                onChange={(e) => setLlmEnabled(e.target.checked)}
              />
              LLM processing
            </label>
          </div>
        </div>
      )}
    </main>
  );
}

export default App;
