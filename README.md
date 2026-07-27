<p align="center">
  <img src="logo.png" alt="Omegawhisper Logo" width="128" height="128">
</p>

<h1 align="center">Omegawhisper</h1>

<p align="center">
  A fork of hyperwhisper that exists to make dictation good on macOS
</p>

---

> ### On Linux? Use [hyperwhisper](https://github.com/hyperwhisper/app) instead.
>
> This is a fork of [**hyperwhisper**](https://github.com/hyperwhisper/app), a desktop
> speech-to-text app with real-time transcription for Linux and macOS. Everything good
> here started there.
>
> The only reason this fork exists is to make the macOS experience better. All the work
> goes into the Mac side. The Linux code is inherited from upstream, left untouched, and
> **never tested here** - so on Linux you would be running an out-of-date copy of the
> original with no benefit. Go to upstream.
>
> On macOS, this fork adds background dictation: a global **F3** shortcut that types into
> whatever app you are using, a spectrogram indicator while you speak, and fully offline
> local transcription.

## About

Omegawhisper is a lightweight desktop application that transcribes your speech.
Record your voice, get instant transcriptions, and optionally auto-type the text
directly into any application.

Transcription can run three ways:

- **Local** - models run offline on your machine. Nothing leaves your computer.
- **Hyperwhisper hosted server** - the upstream project's service.
- **Deepgram** - your own Deepgram API key.

### Features

- Real-time speech-to-text transcription
- Auto-type transcribed text directly into any application
- Local offline transcription (Whisper, Parakeet, Moonshine) - no internet needed
- Audio recording with waveform visualization
- Recordings saved locally as WAV files
- Support for multiple audio input devices
- Dark theme UI
- Works with the Hyperwhisper hosted server or with Deepgram APIs

**macOS only:**

- Runs as a background menu-bar app - no Dock icon, no window in your way
- Global **F3** shortcut to dictate into whatever app you are using
- Spectrogram indicator window while recording
- Language menu: auto-detect, English, Bulgarian

**Linux only (inherited from upstream, untested here):**

- Global keyboard shortcut support via D-Bus

## Installation

### Download

Download the latest release for your platform from the [Releases](https://github.com/webtemp/omegawhisper/releases) page.

**macOS:** no prebuilt release yet - see [macOS — build from source](#macos--build-from-source) below.

**Linux:**

- `.deb` package for Debian/Ubuntu
- `.rpm` package for Fedora
- `.AppImage` for other distributions

```
nix build
```

### Requirements

- Linux with PipeWire/PulseAudio for audio capture
- For auto-type feature: `ydotool` (Wayland) or `xdotool` (X11)

- Steps to enable auto-type on Linux distributions

  - make sure `/dev/uinput` is owned by `root` user and `input` group

    ```sh
    sudo tee /etc/udev/rules.d/99-uinput.rules << 'EOF'
    KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
    EOF
    sudo udevadm trigger --name-match=uinput
    ```

  - create a `ydotoold` user service and enable it

    ```sh
    mkdir -p ~/.config/systemd/user/
    cat > ~/.config/systemd/user/ydotoold.service << 'EOF'
    [Unit]
    Description=ydotoold daemon

    [Service]
    ExecStart=/usr/bin/ydotoold
    Restart=always

    [Install]
    WantedBy=default.target
    EOF

    # Enable and start the service
    systemctl --user enable --now ydotoold.service
    ```

  - add your user to the input group

    ```sh
    sudo usermod -aG input $USER
    ```

- For Ubuntu/Debian:

  ```sh
  sudo apt install -y ydotool
  sudo dpkg -i omegawhisper_0.1.0_amd64.deb
  ```

- For Fedora:

  ```sh
  sudo dnf install omegawhisper-0.1.0-1.x86_64.rpm
  ```

- For NixOS:

  ```sh
  nix build
  ```

### macOS — build from source

Tested on Apple Silicon. There are no prebuilt macOS releases yet.

**1. Install the build tools**

```sh
# Xcode command line tools (C/C++ compiler and linker)
xcode-select --install

# Rust, via rustup (Homebrew's rust package is not recommended for Tauri)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Bun
brew tap oven-sh/bun
brew install bun

# CMake - required to compile whisper-rs-sys for local transcription.
# The build fails without it.
brew install cmake
```

**2. Clone and install dependencies**

```sh
git clone https://github.com/webtemp/omegawhisper.git
cd omegawhisper
bun install
```

`bun install` is not optional. The `tauri` command comes from `@tauri-apps/cli`,
which is a devDependency - without this step `bun tauri build` fails with
"command not found".

**3. Build**

```sh
bun tauri build --bundles app     # .app only, fastest
# or
bun tauri build                   # all bundle formats
```

The first build compiles whisper.cpp and ONNX Runtime from source and takes a
while. Later builds are much faster.

Output: `src-tauri/target/release/bundle/macos/Omegawhisper.app`

**4. Install**

```sh
cp -R src-tauri/target/release/bundle/macos/Omegawhisper.app /Applications/
open /Applications/Omegawhisper.app
```

When replacing an existing install, quit the app first and use `rsync` rather
than `rm -rf`. macOS App Management protection blocks deleting an `.app` folder
in `/Applications` and can leave it half-deleted:

```sh
rsync -a --delete src-tauri/target/release/bundle/macos/Omegawhisper.app/ /Applications/Omegawhisper.app/
```

**5. Grant permissions**

- **Accessibility** - required to type text into other apps.
  `Settings` -> `Privacy & Security` -> `Accessibility`, add
  `/Applications/Omegawhisper.app` and switch it on.
- **Microphone** - macOS asks the first time you record.

These builds are unsigned, so **every rebuild invalidates the Accessibility
grant**. Reset it and re-add the app after each build:

```sh
tccutil reset Accessibility dev.omegawhisper
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
```

## Usage

1. Launch Omegawhisper
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
omegawhisper transcribe toggle
```

or via D-Bus

```sh
dbus-send --session --type=method_call \
  --dest=dev.omegawhisper \
  /dev/omegawhisper \
  dev.omegawhisper.toggle_recording
```

Bind this command to a keyboard shortcut in your desktop environment for hands-free operation.

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Bun](https://bun.sh/) or Node.js
- [CMake](https://cmake.org/) - required to compile `whisper-rs-sys` for local
  transcription. The build fails without it on every platform.
- Linux: development libraries for Tauri
- macOS: Xcode command line tools (`xcode-select --install`)

### Setup

```sh
# Clone the repository
git clone https://github.com/webtemp/omegawhisper.git
cd omegawhisper

# Install dependencies
bun install

# Run in development mode
bun tauri dev
```

### Logo

```sh
bun tauri icon logo.png
```

### Build

```sh
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

- Copyright (C) 2026 Ameya Shenoy &lt;shenoy.ameya@gmail.com&gt;
- Copyright (C) 2026 Deyan Danailov &lt;webtemp@gmail.com&gt;

This is a modified fork of [hyperwhisper](https://github.com/hyperwhisper/app) ! 
Modifications by Deyan Danailov, mainly macOS improvements.
