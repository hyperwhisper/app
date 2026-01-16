<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="HyperWhisper Logo" width="128" height="128">
</p>

<h1 align="center">HyperWhisper</h1>

<p align="center">
  A cross-platform desktop speech-to-text application with real-time transcription
</p>

---

## About

HyperWhisper is a lightweight desktop application that provides real-time audio transcription using the Deepgram API. Record your voice, get instant transcriptions, and optionally auto-type the text directly into any application.

### Features

- Real-time speech-to-text transcription
- Auto-type transcribed text directly into any application
- Audio recording with waveform visualization
- Recordings saved locally as WAV files
- Support for multiple audio input devices
- Dark theme UI
- Global keyboard shortcut support via D-Bus
- Works with HyperWhisper server or with Deepgram APIs

## Installation

### Download

Download the latest release for your platform from the [Releases](https://github.com/hyperwhisper/app/releases) page.

**Linux:**

- `.deb` package for Debian/Ubuntu
- `.rpm` package for Fedora
- `.AppImage` for other distributions

```
nix build
```

### Requirements

- Linux with PipeWire/PulseAudio for audio capture
- For auto-type feature: `ydotool`, `wtype` (Wayland), or `xdotool` (X11)

## Usage

1. Launch HyperWhisper
2. Open Settings and configure your transcription service:
   - **Hyperwhisper**: Use the hosted service
   - **Deepgram**: Use your own Deepgram API key
3. Select your microphone
4. Click the record button or use the global shortcut
5. Speak and watch real-time transcription appear
6. Click stop to finish recording

### Global Shortcut

You can trigger recording from anywhere using:

```bash
hyperwhisper transcribe toggle
```

or via D-Bus

```sh
dbus-send --session --type=method_call \
  --dest=dev.hyperwhisper \
  /dev/hyperwhisper \
  dev.hyperwhisper.toggle_recording
```

Bind this command to a keyboard shortcut in your desktop environment for hands-free operation.

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Bun](https://bun.sh/) or Node.js
- Linux development libraries for Tauri

On Ubuntu/Debian:

```bash
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

### Setup

```bash
# Clone the repository
git clone https://github.com/hyperwhisper/app.git
cd app

# Install dependencies
bun install

# Run in development mode
bun tauri dev
```

### Logo

```
bun tauri icon logo.png
```

### Build

```bash
# Production build
bun tauri build
```

Build artifacts will be in `src-tauri/target/release/bundle/`.

### Project Structure

```
app/
├── src/                    # React frontend
│   ├── components/         # UI components
│   ├── hooks/              # React hooks
│   └── App.tsx             # Main application
├── src-tauri/              # Rust backend
│   ├── src/lib.rs          # Core application logic
│   └── icons/              # App icons
└── package.json
```

## Tech Stack

- **Frontend**: React 19, TypeScript, Tailwind CSS 4, shadcn/ui
- **Backend**: Rust, Tauri v2
- **Audio**: cpal (cross-platform audio)

## License

[GPLv3](./LICENSE)
