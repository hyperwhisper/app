FROM rust:1.91.1-trixie

# Install system dependencies for Tauri
RUN set -ex \
  && apt update \
  && apt install -y \
    build-essential \
    pkg-config \
    cmake \
    libwebkit2gtk-4.1-dev \
    libgtk-3-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    libsoup-3.0-dev \
    libjavascriptcoregtk-4.1-dev \
    libasound2-dev \
    libssl-dev \
    file \
    patchelf \
    squashfs-tools \
    fuse3 \
    libfuse2t64 \
    xdg-utils \
    clang \
    libclang-dev \
    curl \
    unzip \
    libxkbcommon-dev \
    libatk1.0-dev \
    libatk-bridge2.0-dev \
    libgirepository1.0-dev \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    gstreamer1.0-vaapi \
    wtype \
    libvulkan-dev \
    vulkan-tools \
    glslc \
    spirv-tools \
  && rm -rf /var/lib/apt/lists/*

# Install bun
RUN curl -fsSL https://bun.com/install | bash -s "bun-v1.3.5"
ENV PATH="/root/.bun/bin:${PATH}"
