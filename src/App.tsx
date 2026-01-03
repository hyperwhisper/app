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

// Icons
const MicIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
    <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
    <line x1="12" x2="12" y1="19" y2="22" />
  </svg>
);

const StopIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="currentColor">
    <rect x="6" y="6" width="12" height="12" rx="1" />
  </svg>
);

const PlayIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="currentColor">
    <polygon points="6,4 20,12 6,20" />
  </svg>
);

const PauseIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="currentColor">
    <rect x="6" y="4" width="4" height="16" rx="1" />
    <rect x="14" y="4" width="4" height="16" rx="1" />
  </svg>
);

const MinimizeIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
    <rect x="1" y="5.5" width="10" height="1" />
  </svg>
);

const MaximizeIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1">
    <rect x="1.5" y="1.5" width="9" height="9" />
  </svg>
);

const CloseIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
    <path d="M1.5 1.5L10.5 10.5M10.5 1.5L1.5 10.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
  </svg>
);

const DEFAULT_PROMPT = "You are a helpful assistant. Process the following transcription and provide a refined response:";

function App() {
  const [finalText, setFinalText] = useState("");
  const [interimText, setInterimText] = useState("");
  const [theme, setTheme] = useState<Theme>("system");
  const [isRecording, setIsRecording] = useState(false);
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
  const [audioFilePath, setAudioFilePath] = useState<string | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [waveformData, setWaveformData] = useState<number[]>([]);
  const [apiKey, setApiKey] = useState(() => localStorage.getItem("deepgram_api_key") || "");
  const [focusedApp, setFocusedApp] = useState<string | null>(null);

  // STT service state
  const [sttService, setSttService] = useState<SttService>(() =>
    (localStorage.getItem("stt_service") as SttService) || "deepgram"
  );
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(null);
  const [isDownloading, setIsDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [isWhisperProcessing, setIsWhisperProcessing] = useState(false);

  // LLM state
  const [llmPrompt, setLlmPrompt] = useState(() => localStorage.getItem("llm_prompt") || DEFAULT_PROMPT);
  const [llmResponse, setLlmResponse] = useState("");
  const [isProcessingLlm, setIsProcessingLlm] = useState(false);
  const [openRouterApiKey, setOpenRouterApiKey] = useState(() => localStorage.getItem("openrouter_api_key") || "");
  const [typeOutput, setTypeOutput] = useState<"transcription" | "llm">(() =>
    (localStorage.getItem("type_output") as "transcription" | "llm") || "transcription"
  );

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const animationRef = useRef<number | null>(null);
  const finalTextRef = useRef<string>("");

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
      setIsWhisperProcessing(event.payload);

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
            // Process with LLM via Rust backend
            const storedOpenRouterKey = localStorage.getItem("openrouter_api_key");
            if (storedOpenRouterKey?.trim()) {
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

  // Poll for focused window
  useEffect(() => {
    const updateFocusedApp = async () => {
      try {
        const app = await invoke<string | null>("get_focused_window");
        setFocusedApp(app);
      } catch {
        setFocusedApp(null);
      }
    };

    updateFocusedApp();
    const interval = setInterval(updateFocusedApp, 500);

    return () => clearInterval(interval);
  }, []);

  // Listen for transcription events from backend
  useEffect(() => {
    const unlisten = listen<TranscriptionEvent>("transcription", (event) => {
      const { text, is_final } = event.payload;

      if (is_final) {
        if (text) {
          setFinalText((prev) => prev + (prev ? " " : "") + text);
        }
        setInterimText("");
      } else {
        setInterimText(text);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Listen for LLM events from backend
  useEffect(() => {
    const unlistenProcessing = listen<boolean>("llm-processing", (event) => {
      setIsProcessingLlm(event.payload);
      if (event.payload) {
        setLlmResponse("");
      }
    });

    const unlistenResponse = listen<LlmResponseEvent>("llm-response", (event) => {
      setLlmResponse(event.payload.text);
    });

    return () => {
      unlistenProcessing.then((fn) => fn());
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

  // Handle audio playback events
  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;

    const handleEnded = () => setIsPlaying(false);
    const handlePlay = () => setIsPlaying(true);
    const handlePause = () => setIsPlaying(false);

    audio.addEventListener("ended", handleEnded);
    audio.addEventListener("play", handlePlay);
    audio.addEventListener("pause", handlePause);

    return () => {
      audio.removeEventListener("ended", handleEnded);
      audio.removeEventListener("play", handlePlay);
      audio.removeEventListener("pause", handlePause);
    };
  }, [audioUrl]);

  // Generate waveform data from audio URL
  const generateWaveform = useCallback(async (url: string) => {
    try {
      const response = await fetch(url);
      const arrayBuffer = await response.arrayBuffer();
      const audioContext = new AudioContext();
      const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
      const channelData = audioBuffer.getChannelData(0);

      const samples: number[] = [];
      const step = Math.ceil(channelData.length / 200);
      for (let i = 0; i < channelData.length; i += step) {
        samples.push(Math.abs(channelData[i]));
      }
      setWaveformData(samples);
      audioContext.close();
    } catch (err) {
      console.error("Error generating waveform:", err);
    }
  }, []);

  // Draw waveform on canvas
  const drawWaveform = useCallback((progress: number = 0) => {
    const canvas = canvasRef.current;
    if (!canvas || waveformData.length === 0) return;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const width = canvas.width;
    const height = canvas.height;
    const centerY = height / 2;
    const barWidth = width / waveformData.length;
    const maxAmplitude = Math.max(...waveformData, 0.1);

    ctx.clearRect(0, 0, width, height);

    const isDark = document.documentElement.getAttribute("data-theme") === "dark";
    const barColor = isDark ? "#24c8db" : "#396cd8";
    const playedColor = isDark ? "#535bf2" : "#24c8db";

    waveformData.forEach((amplitude, index) => {
      const barHeight = (amplitude / maxAmplitude) * (height * 0.8);
      const x = index * barWidth;
      const isPlayed = index / waveformData.length < progress;

      ctx.fillStyle = isPlayed ? playedColor : barColor;
      ctx.fillRect(x, centerY - barHeight / 2, barWidth - 1, barHeight);
    });
  }, [waveformData]);

  // Animation loop for playback progress
  const animatePlayback = useCallback(() => {
    const audio = audioRef.current;
    if (!audio || !isPlaying) {
      if (animationRef.current) {
        cancelAnimationFrame(animationRef.current);
        animationRef.current = null;
      }
      return;
    }

    const progress = audio.currentTime / audio.duration;
    drawWaveform(progress);

    animationRef.current = requestAnimationFrame(animatePlayback);
  }, [isPlaying, drawWaveform]);

  useEffect(() => {
    if (isPlaying) {
      animatePlayback();
    } else if (audioUrl && audioRef.current) {
      const progress = audioRef.current.currentTime / audioRef.current.duration;
      drawWaveform(isNaN(progress) ? 0 : progress);
    }
  }, [isPlaying, audioUrl, animatePlayback, drawWaveform]);

  useEffect(() => {
    if (waveformData.length > 0 && !isPlaying) {
      drawWaveform(0);
    }
  }, [waveformData, isPlaying, drawWaveform]);

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
      setInterimText("");
      setAudioUrl(null);
      setAudioFilePath(null);

      await invoke("start_recording");
      setIsRecording(true);
    } catch (err: any) {
      alert(`Could not start recording: ${err}`);
    }
  };

  // Stop recording
  const stopRecording = async () => {
    try {
      const response = await invoke<string>("stop_recording");
      const { dataUrl, filePath } = JSON.parse(response);
      setAudioUrl(dataUrl);
      setAudioFilePath(filePath);
      setIsRecording(false);
      setInterimText("");
      await generateWaveform(dataUrl);

      // For Whisper mode, set flag to process when transcription completes
      if (sttService === "whisper") {
        pendingWhisperProcessingRef.current = true;
      } else {
        // For Deepgram, wait briefly for any final transcription events, then process
        setTimeout(async () => {
          const textToType = finalTextRef.current;
          if (textToType) {
            // Type transcription if that's what user selected
            if (typeOutput === "transcription") {
              try {
                await invoke("type_text", { text: textToType });
              } catch (err) {
                console.error("Failed to type text:", err);
              }
            }
            // Process with LLM via Rust backend (will type if typeOutput is "llm")
            if (openRouterApiKey.trim()) {
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

  // Toggle playback
  const handlePlayback = () => {
    const audio = audioRef.current;
    if (!audio) return;

    if (isPlaying) {
      audio.pause();
    } else {
      audio.play();
    }
  };

  function handleCopy() {
    navigator.clipboard.writeText(finalText);
  }

  function cycleTheme() {
    setTheme((prev) => {
      if (prev === "system") return "light";
      if (prev === "light") return "dark";
      return "system";
    });
  }

  function getThemeLabel() {
    if (theme === "system") return "System";
    if (theme === "light") return "Light";
    return "Dark";
  }

  const handleMinimize = () => getCurrentWindow().minimize();
  const handleMaximize = () => getCurrentWindow().toggleMaximize();
  const handleClose = () => getCurrentWindow().close();
  const handleDrag = () => getCurrentWindow().startDragging();

  return (
    <main className="container">
      <div className="titlebar" onMouseDown={handleDrag}>
        <span className="titlebar-title">hyperwhisper</span>
        <div className="titlebar-controls" onMouseDown={(e) => e.stopPropagation()}>
          <button className="theme-toggle" onClick={cycleTheme}>
            {getThemeLabel()}
          </button>
          <button className="titlebar-btn" onClick={handleMinimize}>
            <MinimizeIcon />
          </button>
          <button className="titlebar-btn" onClick={handleMaximize}>
            <MaximizeIcon />
          </button>
          <button className="titlebar-btn titlebar-btn-close" onClick={handleClose}>
            <CloseIcon />
          </button>
        </div>
      </div>

      <button
        className={`record-btn ${isRecording ? "recording" : ""}`}
        onClick={handleRecord}
      >
        {isRecording ? <StopIcon /> : <MicIcon />}
      </button>

      {focusedApp && (
        <div className="focused-app">
          {focusedApp}
        </div>
      )}

      <div className="output-toggle">
        <button
          className={`toggle-btn ${typeOutput === "transcription" ? "active" : ""}`}
          onClick={() => setTypeOutput("transcription")}
        >
          Transcription
        </button>
        <button
          className={`toggle-btn ${typeOutput === "llm" ? "active" : ""}`}
          onClick={() => setTypeOutput("llm")}
        >
          LLM Response
        </button>
      </div>

      <div className="stt-service-container">
        <label className="section-label">STT Service</label>
        <div className="stt-toggle">
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
            Whisper (Local)
          </button>
        </div>

        {sttService === "whisper" && (
          <div className="whisper-status">
            {whisperModelExists === null ? (
              <span className="status-text">Checking model...</span>
            ) : whisperModelExists ? (
              <span className="status-text status-ready">Model ready</span>
            ) : isDownloading ? (
              <div className="download-progress">
                <div className="progress-bar">
                  <div
                    className="progress-fill"
                    style={{ width: `${downloadProgress}%` }}
                  />
                </div>
                <span className="progress-text">{downloadProgress.toFixed(1)}%</span>
              </div>
            ) : (
              <button className="download-btn" onClick={downloadWhisperModel}>
                Download Whisper Model (~142MB)
              </button>
            )}
          </div>
        )}
      </div>

      {audioUrl && !isRecording && (
        <div className="waveform-container">
          <audio ref={audioRef} src={audioUrl} />
          <div className="waveform-controls">
            <canvas
              ref={canvasRef}
              width={400}
              height={60}
              className="waveform-canvas"
            />
            <button className="playback-btn" onClick={handlePlayback}>
              {isPlaying ? <PauseIcon /> : <PlayIcon />}
            </button>
          </div>
          {audioFilePath && (
            <div className="file-path">{audioFilePath}</div>
          )}
        </div>
      )}

      <div className="text-box-container">
        <div className="text-box" spellCheck={false}>
          {finalText || interimText ? (
            <>
              <span>{finalText}</span>
              {interimText && <span className="interim-text">{finalText ? " " : ""}{interimText}</span>}
            </>
          ) : isWhisperProcessing ? (
            <div className="loading-indicator">
              <div className="loading-dots">
                <span></span>
                <span></span>
                <span></span>
              </div>
              <span className="loading-text">Processing with Whisper...</span>
            </div>
          ) : (
            <span className="placeholder">
              {isRecording
                ? (sttService === "whisper" ? "Recording... (transcription after stop)" : "Listening...")
                : "Transcription will appear here..."}
            </span>
          )}
        </div>
        {finalText && !isRecording && !isWhisperProcessing && (
          <button className="copy-btn" onClick={handleCopy}>
            Copy
          </button>
        )}
      </div>

      <div className="text-box-container">
        <label className="section-label">LLM Response</label>
        <div className="text-box llm-response-box" spellCheck={false}>
          {isProcessingLlm ? (
            <div className="loading-indicator">
              <div className="loading-dots">
                <span></span>
                <span></span>
                <span></span>
              </div>
              <span className="loading-text">Processing with LLM...</span>
            </div>
          ) : llmResponse ? (
            <span>{llmResponse}</span>
          ) : (
            <span className="placeholder">
              LLM response will appear here...
            </span>
          )}
        </div>
        {llmResponse && !isProcessingLlm && (
          <button className="copy-btn" onClick={() => navigator.clipboard.writeText(llmResponse)}>
            Copy
          </button>
        )}
      </div>

      <div className="prompt-container">
        <label htmlFor="llm-prompt">LLM Prompt</label>
        <textarea
          id="llm-prompt"
          value={llmPrompt}
          onChange={(e) => setLlmPrompt(e.target.value)}
          placeholder="Enter your custom prompt..."
          className="prompt-input"
          rows={3}
        />
      </div>

      <div className="api-key-container">
        <label htmlFor="api-key">Deepgram API Key</label>
        <input
          id="api-key"
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder="Enter your Deepgram API key"
          className="api-key-input"
        />
      </div>

      <div className="api-key-container">
        <label htmlFor="openrouter-api-key">OpenRouter API Key</label>
        <input
          id="openrouter-api-key"
          type="password"
          value={openRouterApiKey}
          onChange={(e) => setOpenRouterApiKey(e.target.value)}
          placeholder="Enter your OpenRouter API key"
          className="api-key-input"
        />
      </div>
    </main>
  );
}

export default App;
