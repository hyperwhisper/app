let pkgs = import <nixpkgs> { };
in pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    gobject-introspection
    cargo
    cargo-tauri # Optional, Only needed if Tauri doesn't work through the traditional way.
    # nodejs # Optional, this is for if you have a js frontend
    bun
    rustc

    # Required for whisper-rs bindgen
    clang
    llvmPackages.libclang
    cmake

    # For typing text (Wayland/X11)
    wtype      # Wayland (requires compositor support)
    ydotool    # Works on both via uinput
  ];

  buildInputs = with pkgs; [
    at-spi2-atk
    atkmm
    cairo
    gdk-pixbuf
    glib
    gtk3
    harfbuzz
    librsvg
    libsoup_3
    pango
    webkitgtk_4_1
    openssl

    # Audio libraries for cpal (works with PipeWire via ALSA emulation)
    alsa-lib
    libxkbcommon

    # Video/Audio data composition framework tools like "gst-inspect", "gst-launch" ...
    gst_all_1.gstreamer
    # Common plugins like "filesrc" to combine within e.g. gst-launch
    gst_all_1.gst-plugins-base
    # Specialized plugins separated by quality
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-plugins-ugly
    # Plugins to reuse ffmpeg to play almost every video format
    gst_all_1.gst-libav
    # Support the Video Audio (Hardware) Acceleration API
    gst_all_1.gst-vaapi
  ];

  # Set ALSA library path for cpal
  LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath [pkgs.alsa-lib]}";

  # Set LIBCLANG_PATH for whisper-rs bindgen
  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
}
