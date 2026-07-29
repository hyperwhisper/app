#!/bin/bash
# One command from a clean Mac to a working Omegawhisper.
#
# Safe to run again. It only installs what is missing, and it never replaces a
# Rust or a Homebrew that already works on this machine.
set -e
set -o pipefail

cd "$(dirname "$0")/.."

APP_NAME="Omegawhisper.app"
BUILT_APP="src-tauri/target/release/bundle/macos/$APP_NAME"
INSTALLED_APP="/Applications/$APP_NAME"
BUNDLE_ID="dev.omegawhisper"

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

if [ "$1" != "-y" ] && [ -t 0 ]; then
    printf "\nPress Enter to start, or Ctrl-C to stop. "
    read -r _
fi

step "1/7  Xcode Command Line Tools"
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

step "2/7  Homebrew"
if load_homebrew; then
    ok "already installed"
else
    info "Homebrew will ask for your Mac password."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    load_homebrew
    ok "installed"
fi

step "3/7  Rust"
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

step "4/7  Bun and CMake"
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

step "5/7  Project dependencies"
bun install
ok "installed, including the Tauri command line tool"

step "6/7  Building the app"
info "The first build compiles whisper.cpp and ONNX Runtime from source."
info "Expect somewhere between 10 and 30 minutes. Later builds are much faster."
bun run tauri build --bundles app
ok "built"

step "7/7  Installing into /Applications"
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

open -a "$INSTALLED_APP"
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"

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
   open Settings, and pick a backend. To work offline, download a local
   model there - %sWhisper Turbo%s is the best one to start with.

%sThen press F3 anywhere, speak, and press F3 again.%s
The text goes into whatever app you are looking at. That is the whole app.

%sNote:%s the first time you record, macOS asks for the microphone. Say yes.
%sNote:%s this build is unsigned, so every time you rebuild you have to add
      the app to Accessibility again.
" "$BOLD" "$RESET" "$INSTALLED_APP" \
  "$BOLD" "$RESET" "$BOLD" "$RESET" \
  "$BOLD$GREEN" "$RESET" \
  "$YELLOW" "$RESET" "$YELLOW" "$RESET"
