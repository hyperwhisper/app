import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SettingsDialog } from "@/components/settings-dialog";
import { Mic, Square } from "lucide-react";

type SttService = "deepgram" | "whisper";

interface TranscriptionEvent {
  text: string;
  is_final: boolean;
}

interface LlmResponseEvent {
  text: string;
  is_error: boolean;
}

interface WpDevice {
  id: number;
  name: string;
  is_default: boolean;
}

const DEFAULT_PROMPT =
  "You are a helpful assistant. Process the following transcription and provide a refined response:";

function App() {
  const [finalText, setFinalText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
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
  const [audioDevices, setAudioDevices] = useState<WpDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState<number | null>(
    () => {
      const stored = localStorage.getItem("selected_audio_device_id");
      return stored ? parseInt(stored, 10) : null;
    }
  );

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
        if (!canvas || !analyserRef.current) return;

        const ctx = canvas.getContext("2d");
        if (!ctx) return;

        analyserRef.current.getByteFrequencyData(dataArray);

        const width = canvas.width;
        const height = canvas.height;

        ctx.clearRect(0, 0, width, height);

        const isDark = document.documentElement.classList.contains("dark");
        const barColor = isDark ? "hsl(186, 100%, 50%)" : "hsl(221, 83%, 53%)";

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
    <main className="flex flex-col items-center justify-center h-screen w-screen relative bg-white/10 backdrop-blur-2xl backdrop-saturate-150 rounded-2xl border border-white/20 shadow-xl">
      {/* Drag handle area */}
      <div
        className="absolute top-0 left-0 right-0 h-5 cursor-move z-50"
        onMouseDown={handleDrag}
      />

      {/* Header with settings button and title */}
      <div className="absolute top-2 left-0 right-0 flex items-center justify-between px-3">
        <SettingsDialog disabled={isRecording} />
        <span className="text-xs text-muted-foreground font-medium tracking-wide">
          hyperwhisper
        </span>
        <div className="w-8" /> {/* Spacer for balance */}
      </div>

      {/* Audio device selector */}
      <div className="mb-3 w-full max-w-[280px]">
        <Select
          value={selectedDeviceId?.toString() ?? "auto"}
          onValueChange={(v) =>
            setSelectedDeviceId(v === "auto" ? null : parseInt(v, 10))
          }
          disabled={isRecording}
        >
          <SelectTrigger className="h-8 text-xs">
            <SelectValue placeholder="Auto" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">Auto</SelectItem>
            {audioDevices.map((device) => (
              <SelectItem key={device.id} value={device.id.toString()}>
                {device.name}
                {device.is_default ? " *" : ""}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {/* Waveform canvas */}
      <canvas
        ref={canvasRef}
        width={600}
        height={40}
        className="w-[600px] h-[40px] mb-3"
      />

      {/* Record button */}
      <Button
        onClick={handleRecord}
        size="lg"
        className={cn(
          "h-14 w-14 rounded-full transition-all duration-200",
          isRecording
            ? "bg-destructive hover:bg-destructive/90 animate-pulse"
            : "bg-primary hover:bg-primary/90"
        )}
      >
        {isRecording ? (
          <Square className="h-5 w-5 fill-current" />
        ) : (
          <Mic className="h-5 w-5" />
        )}
      </Button>

      {/* Transcription text */}
      {finalText && !isRecording && (
        <div className="mt-4 px-4 w-full max-w-[700px] max-h-[80px] overflow-y-auto">
          <p className="text-sm text-foreground/90 text-center leading-relaxed">
            {finalText}
          </p>
        </div>
      )}
    </main>
  );
}

export default App;
