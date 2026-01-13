# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Critical Rules

1. **No command execution**: Never execute `cargo` or `rustc` commands directly. Always ask the user to run these commands manually.

2. **UI components**: Always use shadcn-vue components with Tailwind CSS 4 for creating UI.


## Project Overview

**HyperWhisper** is a cross-platform desktop speech-to-text application built with Tauri v2 + React. It provides real-time audio transcription using the Deepgram API, with the ability to record, save, and playback audio recordings.

## Development Commands

```bash
# Enter Nix development environment (optional but recommended on Linux)
nix-shell

# Development mode - runs frontend dev server + Tauri
bun run tauri dev

# Production build
bun run tauri build

# Frontend only (Vite dev server on port 1420)
bun run dev

# Build frontend only
bun run build
```

The dev server runs on port **1420** (strict - cannot be changed). The frontend is rebuilt automatically during `tauri dev`.

## Architecture

### High-Level Data Flow

```
User clicks "Record"
    ↓
Frontend invokes "start_recording" (Tauri command)
    ↓
Rust spawns audio capture thread (via cpal)
Rust spawns WebSocket thread for Deepgram
    ↓
Audio chunks → channel → WebSocket → Deepgram API
    ↓
Deepgram responses → "transcription" event → Frontend
    ↓
Frontend displays text (final + interim results)
    ↓
User clicks "Stop"
    ↓
Rust saves WAV file to ~/.local/share/hyperwhisper/recordings/
Rust returns base64 data URL to frontend
    ↓
Frontend displays waveform + enables audio playback
```

### Frontend (React + TypeScript)

**Location:** `src/`

- **App.tsx** (356 lines) - Single-page app with centralized state management
  - Recording state, transcription text, theme, audio playback
  - WebSocket event listener for `transcription` events from Rust
  - Audio waveform visualization using Canvas API
  - API key persistence via localStorage

**Key React patterns:**
- State is managed in a single component (no Redux/Context)
- Tauri `invoke()` for calling Rust commands
- Tauri `listen()` for receiving Rust events
- Audio playback uses HTML5 `<audio>` element with ref

### Backend (Rust - Tauri)

**Location:** `src-tauri/src/`

- **lib.rs** (486 lines) - All core application logic
- **main.rs** - Entry point that calls `hyperwhisper_lib::run()`

**AudioState** - Shared application state using `Arc<Mutex<>>`:
- `is_recording` - Recording status flag
- `recorded_samples` - Audio samples (f32)
- `sample_rate` - Captured audio sample rate
- `stop_signal` - Channel for signaling threads to stop
- `api_key` - Deepgram API key

**Tauri Commands:**
| Command | Description |
|---------|-------------|
| `start_recording` | Spawns audio capture + WebSocket threads |
| `stop_recording` | Stops recording, saves WAV, returns base64 |
| `is_recording` | Returns current recording state |
| `set_api_key` | Stores Deepgram API key in state |
| `get_system_theme` | Detects system theme (Linux/gsettings) |

**Threading Model:**
1. **Audio capture thread** - Uses cpal to capture audio from default input device
   - Converts stereo to mono (averages channels)
   - Supports F32, I16, U16 sample formats
   - Sends audio chunks via channel to WebSocket thread
   - Stores samples for WAV file generation

2. **WebSocket thread** - Connects to Deepgram API
   - Receives audio chunks via channel
   - Converts to linear16 PCM format
   - Sends binary audio to Deepgram
   - Emits `transcription` events for each result
   - Reads with timeout to check stop signal

**Audio Processing:**
- `to_wav_bytes()` - Generates WAV file with proper headers
- `samples_to_linear16()` - Converts f32 to i16 PCM for Deepgram
- Recordings saved to `~/.local/share/hyperwhisper/recordings/`
- Filename format: `YYYY-MM-DD_HH-MM-SS.wav`

**Deepgram Integration:**
- WebSocket URL: `wss://api.deepgram.com/v1/listen`
- Model: `nova-3` with `smart_format=true`
- Parameters: `interim_results=true`, `encoding=linear16`, `channels=1`
- Uses `tungstenite` crate with TLS (`native-tls`)

## Key Dependencies

### Frontend (package.json)
- `@tauri-apps/api` v2 - Tauri API for commands/events
- `react` v19.1.0 - UI framework
- `vite` v7.0.4 - Build tool

### Backend (Cargo.toml)
- `tauri` v2 - Desktop framework
- `cpal` v0.15 - Cross-platform audio capture
- `tungstenite` v0.21 - WebSocket client
- `native-tls` - TLS for WebSocket connections
- `serde`/`serde_json` - JSON serialization
- `base64` - Audio encoding for frontend
- `chrono` - Timestamps for filenames
- `dirs` - Cross-platform paths

## Configuration Files

- **tauri.conf.json** - Tauri config (dev URL: `localhost:1420`)
- **vite.config.ts** - Vite config (port 1420, strict)
- **capabilities/default.json** - Tauri permissions
- **shell.nix** - Nix development environment

## Important Notes

- **Sample rate** is determined by the default audio input device (not hardcoded)
- **Mono conversion** averages all channels - stereo becomes mono
- **300ms flush delay** on stop_recording to ensure buffers flush
- **System theme detection** only works on Linux via `gsettings`
- **API key** is stored in-memory only (not persisted in Rust) - frontend syncs via localStorage
