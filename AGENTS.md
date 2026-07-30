# AGENTS.md/CLAUDE.md

## Critical Rules
1. **UI**: shadcn/ui (the React version) with Tailwind CSS 4, generated into `src/components/ui/`. This is a React project — never shadcn-vue.

## What this is
**Omegawhisper** — desktop speech-to-text, Tauri v2 + React 19. A **macOS-first fork** of [hyperwhisper](https://github.com/hyperwhisper/app). The Linux code is inherited from upstream and never tested here; ignore it unless asked.

**Local only.** Models (Whisper / Parakeet / Moonshine via `transcribe-rs`) run on this machine, buffer the whole recording, and transcribe once you stop. Nothing is sent anywhere. The hosted server and Deepgram were removed in 0.2.1.

**No main window.** The app is a menu-bar agent: Rust does the work, and the only windows are `indicator` (the spectrogram, shown while dictating) and `settings` (opened from the tray).

## Commands
```bash
bun run tauri dev    # dev server + app
bun run tauri build  # production build
bun run dev          # frontend only
```
Vite port **1420**, strict. `nix-shell` or `flake.nix` gives a Nix dev env on Linux.

## One flow, all in Rust
```
F3 anywhere (macOS; Linux: D-Bus / `omegawhisper transcribe toggle`)
                      |
      toggle_recording() -> start_recording_internal()
                      |
    audio capture thread (cpal) + transcription thread
                      |
    16 kHz -> buffer, transcribe once the recording stops,
    then type the text into whatever app has focus
                      |
    events to the indicator window; WAV saved to
    ~/Library/Application Support/omegawhisper/recordings/
```

## Where things live
`src/`
- `main.tsx` — routes on `window.location.pathname`: `/settings`, `/indicator`
- `components/indicator.tsx` — spectrogram, live numbers, errors and startup warnings; drawn from the `mic-level` events Rust sends. It must never open the microphone itself
- `components/settings-page.tsx` — dictation key, microphone, models
- `lib/browser-settings.ts` — hands the settings the old main window kept in browser storage to Rust, once

`src-tauri/src/`
- `lib.rs` — `AudioState`, the event payloads, `run()` and the command list
- `main.rs` — `run()`, or the `transcribe toggle` CLI subcommand
- `settings.rs` — `Prefs`, the settings file, the one-time copy out of browser storage
- `recording.rs` — start/stop/toggle, the capture stream, the transcription thread
- `analysis.rs` — loudness, frequency bands, pitch, trimming, WAV bytes
- `microphone.rs` — listing input devices and picking one
- `indicator.rs` — showing, placing and hiding the indicator window
- `chime.rs` — the two sounds, worked out sample by sample through cpal
- `tray.rs` — the menu-bar icon and its frames; `shortcut.rs` — the dictation key
- `typing.rs` — putting text into other apps; `storage.rs` — the data folder and the log
- `models.rs` — the model list and download/delete; `tests.rs` — every test
- `managers/model.rs` — `AVAILABLE_MODELS`, download, delete, disk status
- `managers/transcription.rs` — loads a model, runs `transcribe-rs`
- `resampler.rs` — resample the microphone's rate down to 16 kHz

`src-tauri/icons/` — all committed, none generated. `icon.icns`/`icon.ico` and the sized PNGs are the app icon; `tray/` holds the menu-bar frames (`key-up`/`mid`/`down`, plus an unused `switch-*` set) as SVG source beside the 36x36 PNG that is compiled in. `TRAY_ICON` in `tray.rs` picks the set; `watch_tray_icon` plays the frames off the recording flag.

Read these in the code, not a copy here: `AudioState` at the top of `lib.rs`, `Prefs` in `settings.rs`, and the commands in `generate_handler!`.

## Notes
- Rust owns every setting. They live in `tray-prefs.json` next to the models and are saved the moment they change. The windows read them with `get_settings` and never keep their own copy — `localStorage` is not used for settings at all.
- Two threads: capture (cpal; stereo to mono by averaging; F32/I16/U16) and transcription. Local transcription runs *after* the stop, which is why the app looks frozen for a moment. Linux adds a D-Bus thread.
- macOS builds add `whisper-metal` and `ort-coreml` so models run on the GPU; without them it falls back to the CPU and is far slower.
- Typing into other apps uses `core-graphics` Unicode key events on macOS, `ydotool`/`wtype`/`xdotool` on Linux.
- `list_audio_devices` asks cpal, so the list is the same set of devices the recording can actually open. A chosen microphone that is unplugged falls back to the system default and says so in the log.
- Silence fed to Whisper makes it invent text. `holds_speech` is what stops that: a recording with no speech in it never reaches the model. `trim_quiet_edges` cuts the quiet start and end. Nothing touches pauses in the middle. Energy-based voice detection was removed in 0.2.1 — it fed the model 3.2x the audio, chopped into fragments, and filtered nothing.
- Sample rate comes from the input device, not hardcoded. 300 ms flush delay on `stop_recording`.
