import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Button } from "@/components/ui/button";
import { SettingsDialog } from "@/components/settings-dialog";

type SttService = "deepgram" | "whisper";

interface TranscriptionEvent {
  text: string;
  is_final: boolean;
}

interface LlmResponseEvent {
  text: string;
  is_error: boolean;
}

const DEFAULT_PROMPT =
  "You are a helpful assistant. Process the following transcription and provide a refined response:";

function App() {
  const [finalText, setFinalText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [apiKey] = useState(
    () => localStorage.getItem("deepgram_api_key") || ""
  );

  // STT service state
  const [sttService] = useState<SttService>(
    () => (localStorage.getItem("stt_service") as SttService) || "deepgram"
  );
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(
    null
  );

  // Audio device state
  const [selectedDeviceId] = useState<number | null>(() => {
    const stored = localStorage.getItem("selected_audio_device_id");
    return stored ? parseInt(stored, 10) : null;
  });

  // LLM state
  const [llmPrompt] = useState(
    () => localStorage.getItem("llm_prompt") || DEFAULT_PROMPT
  );
  const [openRouterApiKey] = useState(
    () => localStorage.getItem("openrouter_api_key") || ""
  );
  const [typeOutput] = useState<"transcription" | "llm">(
    () =>
      (localStorage.getItem("type_output") as "transcription" | "llm") ||
      "transcription"
  );
  const [llmEnabled] = useState(
    () => localStorage.getItem("llm_enabled") !== "false"
  );
  const [realTimeTypingEnabled] = useState(
    () => localStorage.getItem("realtime_typing_enabled") !== "false"
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
    localStorage.setItem(
      "realtime_typing_enabled",
      String(realTimeTypingEnabled)
    );
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

  // Track if we need to process after Whisper completes
  const pendingWhisperProcessingRef = useRef(false);

  // Listen for whisper processing events
  useEffect(() => {
    const unlisten = listen<boolean>("whisper-processing", (event) => {
      // event.payload is true when processing starts, false when done
      setIsProcessing(event.payload);

      if (!event.payload && pendingWhisperProcessingRef.current) {
        pendingWhisperProcessingRef.current = false;

        setTimeout(async () => {
          const textToType = finalTextRef.current;
          if (textToType) {
            if (localStorage.getItem("type_output") === "transcription") {
              try {
                await invoke("type_text", { text: textToType });
              } catch (err) {
                console.error("Failed to type text:", err);
              }
            }
            const storedOpenRouterKey =
              localStorage.getItem("openrouter_api_key");
            const isLlmEnabled =
              localStorage.getItem("llm_enabled") !== "false";
            if (isLlmEnabled && storedOpenRouterKey?.trim()) {
              try {
                await invoke("process_with_llm", {
                  transcription: textToType,
                  shouldType: localStorage.getItem("type_output") === "llm",
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

      const shouldAutoType =
        sttService === "deepgram" &&
        typeOutput === "transcription" &&
        realTimeTypingEnabled;
      await invoke("set_auto_type_transcription", { enabled: shouldAutoType });

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

      if (sttService === "whisper") {
        pendingWhisperProcessingRef.current = true;
      } else {
        setTimeout(async () => {
          const textToType = finalTextRef.current;
          if (textToType) {
            if (typeOutput === "transcription" && !realTimeTypingEnabled) {
              try {
                await invoke("type_text", { text: textToType });
              } catch (err) {
                console.error("Failed to type text:", err);
              }
            }
            if (llmEnabled && openRouterApiKey.trim()) {
              try {
                await invoke("process_with_llm", {
                  transcription: textToType,
                  shouldType: typeOutput === "llm",
                });
              } catch (err) {
                console.error("Failed to process with LLM:", err);
              }
            }
          }
        }, 200);
      }
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
      className="flex flex-col h-screen w-screen relative bg-neutral-800/80 backdrop-blur-2xl rounded-2xl border border-white/10 shadow-xl overflow-hidden"
      onMouseDown={handleDrag}
    >
      {/* Waveform area */}
      <div className="flex-1 flex items-center justify-center px-8">
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
        ) : isProcessing ? (
          <div className="px-4 w-full max-h-[100px] overflow-y-auto">
            <p className="text-sm text-white/50 text-center leading-relaxed animate-pulse">
              {finalText || "Processing..."}
            </p>
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
      </div>

      {/* Bottom bar */}
      <div className="h-14 bg-neutral-900/80 border-t border-white/10 flex items-center justify-between px-4">
        {/* Left: Recording status */}
        <div className="flex items-center gap-2">
          {isRecording ? (
            <>
              <div className="w-4 h-4 rounded-sm bg-red-500 animate-pulse" />
              <span className="text-white font-medium text-sm">Recording</span>
            </>
          ) : isProcessing ? (
            <>
              <div className="w-4 h-4 rounded-sm bg-yellow-500 animate-pulse" />
              <span className="text-white font-medium text-sm">Processing</span>
            </>
          ) : (
            <>
              <SettingsDialog disabled={isRecording || isProcessing} />
              <span className="text-white/60 text-sm">Ready</span>
            </>
          )}
        </div>

        {/* Right: Controls */}
        <div className="flex items-center gap-2">
          {isRecording ? (
            <Button
              onClick={handleRecord}
              variant="ghost"
              size="sm"
              className="text-white/80 hover:text-white hover:bg-white/10"
            >
              Stop
            </Button>
          ) : (
            <Button
              onClick={handleRecord}
              variant="ghost"
              size="sm"
              className="text-white/80 hover:text-white hover:bg-white/10"
              disabled={isProcessing}
            >
              Record
            </Button>
          )}
        </div>
      </div>
    </main>
  );
}

export default App;
