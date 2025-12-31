import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

type Theme = "light" | "dark" | "system";

interface TranscriptionEvent {
  text: string;
  is_final: boolean;
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
  }, [llmPrompt]);

  useEffect(() => {
    localStorage.setItem("openrouter_api_key", openRouterApiKey);
  }, [openRouterApiKey]);

  useEffect(() => {
    localStorage.setItem("type_output", typeOutput);
  }, [typeOutput]);

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

  // Call OpenRouter API with transcription
  const processWithLlm = async (transcription: string, shouldType: boolean) => {
    if (!openRouterApiKey.trim() || !transcription.trim()) {
      return;
    }

    setIsProcessingLlm(true);
    setLlmResponse("");

    try {
      const response = await fetch("https://openrouter.ai/api/v1/chat/completions", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Authorization": `Bearer ${openRouterApiKey}`,
        },
        body: JSON.stringify({
          model: "openai/gpt-4.1-nano",
          messages: [
            { role: "system", content: llmPrompt },
            { role: "user", content: transcription }
          ]
        })
      });

      if (!response.ok) {
        const error = await response.text();
        throw new Error(`API error: ${error}`);
      }

      const data = await response.json();
      const llmText = data.choices?.[0]?.message?.content || "";
      setLlmResponse(llmText);

      // Type LLM response if that's what user selected
      if (shouldType && llmText) {
        try {
          await invoke("type_text", { text: llmText });
        } catch (err) {
          console.error("Failed to type LLM response:", err);
        }
      }
    } catch (err: any) {
      console.error("LLM processing error:", err);
      setLlmResponse(`Error: ${err.message}`);
    } finally {
      setIsProcessingLlm(false);
    }
  };

  // Start recording
  const startRecording = async () => {
    if (!apiKey.trim()) {
      alert("Please enter your Deepgram API key first");
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

      // Wait briefly for any final transcription events, then process
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
          // Process with LLM (will type if typeOutput is "llm")
          processWithLlm(textToType, typeOutput === "llm");
        }
      }, 200);
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
          ) : (
            <span className="placeholder">
              {isRecording ? "Listening..." : "Transcription will appear here..."}
            </span>
          )}
        </div>
        {finalText && !isRecording && (
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
