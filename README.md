<p align="center">
  <img src="logo.png" alt="Omegawhisper Logo" width="128" height="128">
</p>

<h1 align="center">Omegawhisper</h1>

<p align="center">
  Press one key anywhere on your Mac, speak, and the text is typed into whatever app you are in.
</p>

---

> ### On Linux? Use [hyperwhisper](https://github.com/hyperwhisper/app) instead.
>
> This is a fork of [**hyperwhisper**](https://github.com/hyperwhisper/app). Everything
> good here started there. The only reason this fork exists is to make dictation good on
> macOS, so all the work goes into the Mac side. The Linux code is inherited from
> upstream, left untouched, and **never tested here** — on Linux you would be running an
> out-of-date copy of the original with no benefit. Go to upstream.

## What it does

Omegawhisper sits in your menu bar. It has no Dock icon and no window in your way.
Press **F3**, speak, press **F3** again. A small spectrogram shows it is listening, and
the text is typed into the app you were already using. F3 is only the default — pick any
key in Settings.

Transcription runs three ways:

| Backend | Where it runs | Speed |
|---|---|---|
| **Local** (Whisper, Parakeet, Moonshine) | Your Mac, on the GPU. Offline, nothing leaves the machine | Transcribes after you stop |
| **Hyperwhisper server** | The upstream project's hosted service | Text appears while you speak |
| **Deepgram** | Deepgram, with your own API key | Text appears while you speak |

### Features

- One global shortcut, works in any app. **F3** by default, changeable in Settings
- Types into other apps with Unicode key events
- Local models run on the Mac GPU (Metal + CoreML)
- Spectrogram indicator window while you speak
- Recordings saved as WAV, and deletable from the menu bar
- Silence is never sent to the model, so it cannot invent text from a quiet room
- Dark theme

## Install (macOS)

There are no prebuilt macOS releases. You build it yourself. Tested on Apple Silicon.

### The short way

```sh
git clone https://github.com/webtemp/omegawhisper.git
cd omegawhisper
./scripts/install.sh
```

That does steps 1 to 4 below for you. It installs only what is missing, never replaces a
Rust or a Homebrew you already have, and stops twice to tell you what to click. You still
have to grant Accessibility yourself (step 5) — macOS does not let any script do that.

It also asks which offline model to download, defaulting to Whisper Turbo, and fetches it
in the background while everything else installs. Choose **None** to skip it and pick one
in Settings later. The model is usually the slowest part, so it waits for it at the end.

Or do it by hand, below. Six steps. Step 5 grants macOS permissions — the app cannot type
anything until you do it, so do not stop after the build.

### 1. Install the build tools

You need [Homebrew](https://brew.sh) first, since two of these come from it:

```sh
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

Then:

```sh
xcode-select --install                                          # C/C++ compiler and linker
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh  # Rust (not Homebrew's rust)
brew install bun                                                # Bun
brew install cmake                                              # builds whisper.cpp
```

CMake is not optional. Without it the build fails while compiling `whisper-rs-sys`.

> [!IMPORTANT]
> **After installing Rust, open a new terminal.** Rustup does not add `cargo` to the
> terminal you are sitting in — it writes a line into your shell startup files, and only
> terminals opened afterwards read those. Carry on in the same window and the build fails
> with `failed to run 'cargo metadata'` and `No such file or directory (os error 2)`, which
> never mentions Rust. To fix the window you already have without opening a new one:
>
> ```sh
> source "$HOME/.cargo/env"
> ```

### 2. Get the code and the Tauri CLI

```sh
git clone https://github.com/webtemp/omegawhisper.git
cd omegawhisper
bun install
```

`bun install` is what installs the Tauri CLI — it is a devDependency, not something you
install globally. Check it worked before going on:

```sh
bun run tauri --version     # should print: tauri-cli 2.x.x
```

If that says "command not found", install it into the project by hand and try again:

```sh
bun add -D @tauri-apps/cli
```

### 3. Build

```sh
bun run tauri build --bundles app
```

The first build compiles whisper.cpp and ONNX Runtime from source and takes a while.
Later builds are much faster.

Result: `src-tauri/target/release/bundle/macos/Omegawhisper.app`

### 4. Copy it to Applications

```sh
cp -R src-tauri/target/release/bundle/macos/Omegawhisper.app /Applications/
open /Applications/Omegawhisper.app
```

**Replacing an existing install?** Quit the app first and use `rsync`, not `rm -rf`.
macOS App Management protection blocks deleting an `.app` folder in `/Applications` and
can leave it half-deleted:

```sh
rsync -a --delete src-tauri/target/release/bundle/macos/Omegawhisper.app/ /Applications/Omegawhisper.app/
```

### 5. Grant permissions — do not skip this

> [!IMPORTANT]
> **Without Accessibility, the app does nothing useful.** It will record you, transcribe
> you, and then fail to type a single character — with no error message. If you only do
> one thing from this whole page, do this.

**Accessibility** is what lets the app type into other apps. Turn it on:

```sh
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
```

Then add `/Applications/Omegawhisper.app` to the list and switch the toggle on.

**Microphone** needs nothing from you now — macOS asks the first time you record. Say yes.

#### After every rebuild, do it again

These builds are unsigned, so macOS treats each new build as a different app and
**throws the Accessibility grant away**. The nasty part: the toggle still looks switched
on, so it appears fine while typing silently fails.

Every time you rebuild, run this and add the app back:

```sh
tccutil reset Accessibility dev.omegawhisper
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
```

### 6. Pick a backend

Open **Settings** from the menu-bar icon.

To run offline, download a local model there. **Whisper Turbo** is the one to start with:
it is the best mix of speed and accuracy on the Mac GPU, and it handles any language.
Whisper Small is smaller and quicker if you are short of disk space; Whisper Large is more
accurate but noticeably slower. Parakeet and Moonshine are English only.

Models are 80 MB to 1.6 GB, so the first download takes a moment.

The microphone is always the system default input. The device list in Settings is Linux
code and does nothing on macOS; change the input in System Settings → Sound.

## Using it

Press **F3** to start, speak, press **F3** to stop. That is the whole app.

To use a different key, open **Settings** → **Dictation key** → **Change**, then press the
combination you want. If it is already taken by another app it says so and keeps the old
one, so you can never end up with no shortcut.

The menu-bar icon has:

| Item | What it does |
|---|---|
| Open window | Shows the main window: text, waveform, playback |
| Hide window | Puts it away again, back to menu bar only |
| Recordings → Open Folder | `~/Library/Application Support/omegawhisper/recordings` |
| Recordings → Delete Recordings | Deletes every saved WAV. Asks first |
| Show debug stats | Live microphone numbers, and a line of numbers under each result. Also in Settings |
| Settings | Dictation key, backend, model, trial key |
| Quit | Quits |

## Troubleshooting

**Nothing happens when I press the key.** Another app has taken it, or Accessibility is
off. Open the main window — startup problems appear there as a message. Pick a different
key in Settings → Dictation key.

**Text is transcribed but never typed.** Accessibility. If you rebuilt the app, the grant
is gone even though the checkbox still looks on: reset it (step 5).

**The build says `failed to run 'cargo metadata'` / `No such file or directory (os error 2)`.**
That means the build cannot find `cargo`. Two different causes:

```sh
which cargo || ls ~/.cargo/bin/cargo
```

Nothing found at all — Rust is not installed, do step 1. Found in `~/.cargo/bin` but `which`
says nothing — it is installed and your terminal is just too old to see it, so open a new
one or run `source "$HOME/.cargo/env"`.

**The app freezes for a second after I stop.** Expected with local models. They transcribe
after the recording ends, not during.

**Whisper writes text I never said.** Recordings with no speech in them are refused before
they reach the model, so this should not happen. If it does, the log line for that
dictation shows the loudness it measured.

**Anything else.** The log is at
`~/Library/Application Support/omegawhisper/omegawhisper.log`. Switch on **Show debug
stats**, in the menu bar or in Settings, to get live microphone numbers and a line of
numbers per dictation.

## Development

```sh
bun install
bun run tauri dev # dev server + app
bun run test      # Rust tests + frontend tests
bun run test:rust # Rust only
bun run test:web  # frontend only
bun run dev       # frontend only, port 1420
```

```
src/                       React 19 frontend (two windows, no main window)
  components/indicator.tsx spectrogram, errors, startup warnings
  components/settings-page.tsx  dictation key, microphone, models
src-tauri/src/
  lib.rs                   app state, events, run(), the command list
  recording.rs             start/stop, capture stream, transcription thread
  analysis.rs              loudness, frequency bands, pitch, trimming, WAV
  settings.rs              the settings file; microphone.rs  input devices
  indicator.rs  chime.rs  tray.rs  shortcut.rs  typing.rs  storage.rs
  managers/model.rs        model list, download, delete
  managers/transcription.rs  loads a model, runs transcribe-rs
src-tauri/icons/           app icon and menu-bar frames, all committed
```

**Tech stack:** React 19, TypeScript, Tailwind CSS 4, shadcn/ui, Rust, Tauri v2, cpal.

## Linux

<details>
<summary>Inherited from upstream and never tested here — expand only if you know what you are doing</summary>

Really, use [hyperwhisper](https://github.com/hyperwhisper/app). Nothing below has been
run since the fork.

### Requirements

- PipeWire or PulseAudio for audio capture
- `ydotool` (Wayland) or `xdotool` (X11) for auto-type

### Enabling auto-type

Make sure `/dev/uinput` is owned by the `root` user and the `input` group:

```sh
sudo tee /etc/udev/rules.d/99-uinput.rules << 'EOF'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
EOF
sudo udevadm trigger --name-match=uinput
```

Create a `ydotoold` user service and enable it:

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

systemctl --user enable --now ydotoold.service
```

Add your user to the `input` group:

```sh
sudo usermod -aG input $USER
```

### Building

```sh
bun install
bun run tauri build # .deb, .rpm, .AppImage in src-tauri/target/release/bundle/
nix build           # NixOS
```

`nix-shell` or `flake.nix` gives a dev environment.

### Global shortcut

There is no built-in shortcut on Linux. Bind this to a key in your desktop environment:

```sh
omegawhisper transcribe toggle
```

Or over D-Bus:

```sh
dbus-send --session --type=method_call \
  --dest=dev.omegawhisper \
  /dev/omegawhisper \
  dev.omegawhisper.toggle_recording
```

</details>

## License

[GPLv3](./LICENSE)

- Copyright (C) 2026 Ameya Shenoy &lt;shenoy.ameya@gmail.com&gt;
- Copyright (C) 2026 Deyan Danailov &lt;webtemp@gmail.com&gt;

A modified fork of [hyperwhisper](https://github.com/hyperwhisper/app).
Modifications by Deyan Danailov, mainly macOS improvements.
