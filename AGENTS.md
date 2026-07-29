# AGENTS.md/CLAUDE.md

## Critical Rules
1. **UI**: shadcn/ui (the React version) with Tailwind CSS 4, generated into `src/components/ui/`. This is a React project — never shadcn-vue.

## What this is
**Omegawhisper** — desktop speech-to-text, Tauri v2 + React 19. A **macOS-first fork** of [hyperwhisper](https://github.com/hyperwhisper/app). The Linux code is inherited from upstream and never tested here; ignore it unless asked.

Three backends. The **hyperwhisper hosted server** (default) and **Deepgram** stream over WebSocket and return interim and final results while you speak. **Local models** (Whisper / Parakeet / Moonshine via `transcribe-rs`) run offline, buffer everything, and transcribe once after you stop.

## Commands
```bash
bun run tauri dev    # dev server + app
bun run tauri build  # production build
bun run dev          # frontend only
```
Vite port **1420**, strict. `nix-shell` or `flake.nix` gives a Nix dev env on Linux.

## Two entry points, one flow
Clicking Record and pressing F3 both call `handleRecord()` in `App.tsx`, so there is one recording code path.

```
Click Record  |  F3 anywhere (macOS; Linux: D-Bus / `omegawhisper transcribe toggle`)
                 -> Rust emits "recording-toggled"
         \              /
      App.tsx handleRecord() -> invoke("start_recording")
                      |
    audio capture thread (cpal) + transcription thread
                      |
    LOCAL                          STREAMING
    16 kHz -> VAD -> buffer        chunks -> WebSocket
    transcribe on stop             interim + final stream back
    auto-type at the end           auto-type per final segment
                      |
    "transcription" event -> UI; WAV saved to
    ~/.local/share/omegawhisper/recordings/ + base64 URL for playback
```

## Where things live
`src/`
- `main.tsx` — routes on `window.location.pathname`: `/settings`, `/indicator`, else the main app
- `App.tsx` — main window: recording state, text, waveform, playback
- `components/indicator.tsx` — spectrogram shown while recording; opens its own mic via `getUserMedia`, driven by the `indicator-active` event
- `components/settings-page.tsx` — backend choice, models, audio device, trial key
- `hooks/use-trial-key.ts`, `components/ui/` (shadcn), `theme-provider.tsx` (dark default)

`src-tauri/src/`
- `lib.rs` — commands, recording threads, typing into other apps, tray, windows
- `main.rs` — `run()`, or the `transcribe toggle` CLI subcommand
- `managers/model.rs` — `AVAILABLE_MODELS`, download, delete, disk status
- `managers/transcription.rs` — loads a model, runs `transcribe-rs`
- `audio/vad.rs` — energy-based voice detection; `resampler.rs` — resample to 16 kHz

Read these in the code, not a copy here: `AudioState` (18 fields) at the top of `lib.rs`, and the 33 commands in its `generate_handler!`.

## Notes
- All frontend state sits in one component (no Redux/Context). Settings live in `localStorage` and are pushed to Rust at startup; Rust keeps them in memory only.
- Two threads: capture (cpal; stereo to mono by averaging; F32/I16/U16) and transcription. Local transcription runs *after* the stop, which is why the app looks frozen for a moment. Linux adds a D-Bus thread.
- macOS builds add `whisper-metal` and `ort-coreml` so models run on the GPU; without them it falls back to the CPU and is far slower.
- Typing into other apps uses `core-graphics` Unicode key events on macOS, `ydotool`/`wtype`/`xdotool` on Linux.
- `list_audio_devices` shells out to `wpctl` (WirePlumber, Linux-only) — device picking does not work on macOS.
- Keep VAD on. Silence fed to Whisper makes it invent text.
- Sample rate comes from the input device, not hardcoded. 300 ms flush delay on `stop_recording`.
