#!/bin/bash
# One command from a clean Mac to a working Omegawhisper.
#
# Safe to run again. It only installs what is missing, and it never replaces a
# Rust or a Homebrew that already works on this machine.
set -e
set -o pipefail
# Stop on an unset variable. An empty path in one of the copy commands below
# would otherwise be read as "/" and do real damage.
set -u

cd "$(dirname "$0")/.."

APP_NAME="Omegawhisper.app"
BUILT_APP="src-tauri/target/release/bundle/macos/$APP_NAME"
INSTALLED_APP="/Applications/$APP_NAME"
BUNDLE_ID="dev.omegawhisper"
# Where the app looks for models. Overridable only so this can be tested
# without writing into the real one.
MODELS_DIR="${MODELS_DIR:-$HOME/Library/Application Support/omegawhisper/models}"

# Colour only when a person is watching, so piping this to a file stays readable.
if [ -t 1 ]; then
    # Fall back to no colour rather than stopping if TERM is not set.
    BOLD=$(tput bold 2>/dev/null || true);     GREEN=$(tput setaf 2 2>/dev/null || true)
    YELLOW=$(tput setaf 3 2>/dev/null || true); BLUE=$(tput setaf 4 2>/dev/null || true)
    RESET=$(tput sgr0 2>/dev/null || true)
else
    BOLD=""; GREEN=""; YELLOW=""; BLUE=""; RESET=""
fi

step() { printf "\n%s==> %s%s\n" "$BOLD$BLUE" "$1" "$RESET"; }
ok()   { printf "  %s+%s %s\n" "$GREEN" "$RESET" "$1"; }
info() { printf "    %s\n" "$1"; }
warn() { printf "  %s!%s %s\n" "$YELLOW" "$RESET" "$1"; }

# Rust lives here when rustup installed it.
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

# The files each model needs, as "name url" lines. These have to match
# AVAILABLE_MODELS in src-tauri/src/managers/model.rs, which is the real list -
# the app decides a model is ready by checking these files are on disk.
model_files() {
    HF="https://huggingface.co"
    case "$1" in
        whisper-turbo)
            echo "ggml-large-v3-turbo.bin $HF/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
            ;;
        whisper-small)
            echo "ggml-small.bin $HF/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            ;;
        parakeet-v3-int8)
            P="$HF/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
            echo "encoder-model.int8.onnx $P/encoder-model.int8.onnx"
            echo "decoder_joint-model.int8.onnx $P/decoder_joint-model.int8.onnx"
            echo "nemo128.onnx $P/nemo128.onnx"
            echo "vocab.txt $P/vocab.txt"
            echo "config.json $P/config.json"
            ;;
    esac
}

model_name() {
    case "$1" in
        whisper-turbo)    echo "Whisper Turbo" ;;
        whisper-small)    echo "Whisper Small" ;;
        parakeet-v3-int8) echo "Parakeet V3 (int8)" ;;
        *)                echo "$1" ;;
    esac
}

# Megabytes, only so the progress line has something to count towards.
model_size_mb() {
    case "$1" in
        whisper-turbo)    echo 1549 ;;
        whisper-small)    echo 465 ;;
        parakeet-v3-int8) echo 639 ;;
        *)                echo 0 ;;
    esac
}

model_is_downloaded() {
    files=$(model_files "$1")
    [ -n "$files" ] || return 1
    while read -r name _; do
        [ -f "$MODELS_DIR/$1/$name" ] || return 1
    done <<< "$files"
    return 0
}

download_model() {
    mkdir -p "$MODELS_DIR/$1"
    while read -r name url; do
        [ -f "$MODELS_DIR/$1/$name" ] && continue
        # Download under a temporary name and move it into place only once curl
        # is happy. The app calls a model ready as soon as the file exists, so a
        # half-finished download would look fine and then fail while you speak.
        curl -fL --retry 3 -o "$MODELS_DIR/$1/$name.part" "$url"
        mv "$MODELS_DIR/$1/$name.part" "$MODELS_DIR/$1/$name"
    done <<< "$(model_files "$1")"
}

# Homebrew is in a different place on Apple Silicon and on Intel Macs.
load_homebrew() {
    command -v brew >/dev/null 2>&1 && return 0
    for brew_bin in /opt/homebrew/bin/brew /usr/local/bin/brew; do
        if [ -x "$brew_bin" ]; then
            eval "$("$brew_bin" shellenv)"
            return 0
        fi
    done
    return 1
}

printf "\n%sOmegawhisper installer%s\n" "$BOLD" "$RESET"
printf "This builds the app from source and installs it into /Applications.\n"

# Say up front what is missing, so nobody is surprised by what gets installed.
load_homebrew || true
MISSING=""
xcode-select -p          >/dev/null 2>&1 || MISSING="$MISSING\n    - Xcode Command Line Tools"
command -v brew          >/dev/null 2>&1 || MISSING="$MISSING\n    - Homebrew"
{ command -v cargo >/dev/null 2>&1 || [ -x "$CARGO_BIN/cargo" ]; } || MISSING="$MISSING\n    - Rust"
command -v bun           >/dev/null 2>&1 || MISSING="$MISSING\n    - Bun"
command -v cmake         >/dev/null 2>&1 || MISSING="$MISSING\n    - CMake"

if [ -n "$MISSING" ]; then
    printf "\nIt needs to install:%b\n" "$MISSING"
    printf "\nYou may be asked for your Mac password, and for permission to install.\n"
else
    printf "\nEverything it needs is already installed.\n"
fi

# Asked now rather than later so the rest of the install can be left alone.
MODEL_ID="whisper-turbo"
if [ "${1:-}" != "-y" ] && [ -t 0 ]; then
    printf "
%sWhich model should it download for working offline?%s
  1) Whisper Turbo   1.6 GB   any language, best all-rounder (default)
  2) Whisper Small   488 MB   any language, quicker and less accurate
  3) Parakeet V3     670 MB   English only, fast and very accurate
  4) None            skip it, or pick one later in Settings

Choose 1-4, or press Enter for 1: " "$BOLD" "$RESET"
    read -r MODEL_CHOICE
    case "$MODEL_CHOICE" in
        2) MODEL_ID="whisper-small" ;;
        3) MODEL_ID="parakeet-v3-int8" ;;
        4) MODEL_ID="none" ;;
        *) MODEL_ID="whisper-turbo" ;;
    esac

    printf "\nPress Enter to start, or Ctrl-C to stop. "
    read -r _
fi

# Started before anything else, because on a Mac with the dependencies already
# cached the build is under a minute and this download is the slowest thing here.
MODEL_PID=""
MODEL_LOG=""
if [ "$MODEL_ID" != none ] && ! model_is_downloaded "$MODEL_ID"; then
    MODEL_LOG=$(mktemp -t omegawhisper-model)
    download_model "$MODEL_ID" > "$MODEL_LOG" 2>&1 &
    MODEL_PID=$!
    printf "\n%s downloading in the background while everything else installs.\n" \
        "$(model_name "$MODEL_ID")"
fi

step "1/8  Xcode Command Line Tools"
if xcode-select -p >/dev/null 2>&1; then
    ok "already installed"
else
    info "A macOS window will open now. Click Install, agree, and wait for it."
    info "This one cannot be automated - come back here when it finishes."
    xcode-select --install >/dev/null 2>&1 || true
    printf "    waiting"
    until xcode-select -p >/dev/null 2>&1; do printf "."; sleep 5; done
    printf "\n"
    ok "installed"
fi

step "2/8  Homebrew"
if load_homebrew; then
    ok "already installed"
else
    info "Homebrew will ask for your Mac password."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    load_homebrew
    ok "installed"
fi

step "3/8  Rust"
if command -v cargo >/dev/null 2>&1; then
    # Someone may already have Rust from Homebrew. Installing rustup next to it
    # would change which cargo wins, on a machine that was working fine.
    ok "already installed ($(command -v cargo))"
elif [ -x "$CARGO_BIN/cargo" ]; then
    # Rust is here, but this terminal was opened before it and never saw it.
    PATH="$PATH:$CARGO_BIN"
    ok "already installed, and now visible to this terminal"
else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    PATH="$PATH:$CARGO_BIN"
    ok "installed"
fi
export PATH

step "4/8  Bun and CMake"
if command -v bun >/dev/null 2>&1; then
    ok "Bun already installed"
else
    brew install bun
    ok "Bun installed"
fi
if command -v cmake >/dev/null 2>&1; then
    ok "CMake already installed"
else
    # Without CMake the build dies deep inside whisper-rs-sys, minutes in.
    brew install cmake
    ok "CMake installed"
fi

step "5/8  Project dependencies"
bun install
ok "installed, including the Tauri command line tool"

step "6/8  Building the app"
info "The first build has to compile the dependencies, so it is slower than"
info "later ones. After that a rebuild is well under a minute."
bun run tauri build --bundles app
ok "built"

step "7/8  Installing into /Applications"
REPLACED=no
if [ -d "$INSTALLED_APP" ]; then
    REPLACED=yes
    pkill -x omegawhisper 2>/dev/null || true
    sleep 1
    # Copy over the top rather than deleting first: macOS blocks removing an app
    # from /Applications and can leave half of it behind.
    rsync -a --delete "$BUILT_APP/" "$INSTALLED_APP/"
else
    cp -R "$BUILT_APP" /Applications/
fi
ok "installed at $INSTALLED_APP"

if [ "$REPLACED" = yes ]; then
    # This build is unsigned, so macOS counts it as a different app and the old
    # permission is already dead - while the switch still looks turned on. Clear
    # it so the list tells the truth.
    tccutil reset Accessibility "$BUNDLE_ID" >/dev/null 2>&1 || true
    warn "Replaced an older build, so the Accessibility permission was reset."
fi

step "8/8  Offline model"
if [ "$MODEL_ID" = none ]; then
    ok "skipped - you can download one in Settings whenever you want"
elif [ -z "$MODEL_PID" ]; then
    ok "$(model_name "$MODEL_ID") was already downloaded"
else
    info "Everything else is done. Waiting for the download to finish."
    TOTAL_MB=$(model_size_mb "$MODEL_ID")
    # Counted off the files themselves, because curl is writing to a log rather
    # than the screen and a silent wait looks like a hang.
    while kill -0 "$MODEL_PID" 2>/dev/null; do
        HAVE_MB=$(( $(du -sk "$MODELS_DIR/$MODEL_ID" 2>/dev/null | cut -f1 || echo 0) / 1024 ))
        if [ -t 1 ] && [ "$TOTAL_MB" -gt 0 ]; then
            # The sizes are close estimates, so cap it rather than show 103%.
            PCT=$(( HAVE_MB * 100 / TOTAL_MB ))
            if [ "$PCT" -gt 100 ]; then PCT=100; fi
            printf "\r    %s MB of %s MB   (%s%%)   " "$HAVE_MB" "$TOTAL_MB" "$PCT"
        fi
        sleep 2
    done
    [ -t 1 ] && printf "\r%*s\r" 50 ""
    if wait "$MODEL_PID"; then
        ok "$(model_name "$MODEL_ID") downloaded"
    else
        # Not fatal. The app still works against the hosted server without it.
        MODEL_ID=none
        warn "The download failed. What went wrong is in $MODEL_LOG"
        warn "Everything else is fine - download a model in Settings instead."
    fi
fi

open -a "$INSTALLED_APP"
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"

if [ "$MODEL_ID" = none ]; then
    MODEL_ADVICE="open Settings, and pick a backend. To work offline, download
   a local model there - ${BOLD}Whisper Turbo${RESET} is the best one to start with."
else
    MODEL_ADVICE="open Settings, choose ${BOLD}Local${RESET}, and pick ${BOLD}$(model_name "$MODEL_ID")${RESET}.
   It is already downloaded, so there is nothing left to wait for."
fi

printf "\n%s%s\n" "$BOLD" "The build is done. Two things left, both by hand.$RESET"
printf "
%s1. Turn on Accessibility. This is what lets the app type for you.%s
   System Settings just opened on the right page.
   - Click the + button
   - Choose %s
   - Make sure its switch is ON

   Without this the app records you, understands you, and then types
   nothing at all, with no error message.

%s2. Choose how it transcribes.%s
   Click the Omegawhisper icon in the menu bar at the top of your screen,
   %s

%sThen press F3 anywhere, speak, and press F3 again.%s
The text goes into whatever app you are looking at. That is the whole app.

%sNote:%s the first time you record, macOS asks for the microphone. Say yes.
%sNote:%s this build is unsigned, so every time you rebuild you have to add
      the app to Accessibility again.
" "$BOLD" "$RESET" "$INSTALLED_APP" \
  "$BOLD" "$RESET" "$MODEL_ADVICE" \
  "$BOLD$GREEN" "$RESET" \
  "$YELLOW" "$RESET" "$YELLOW" "$RESET"
