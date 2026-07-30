{
  description = "Omegawhisper - Type 3x faster, without lifting a finger";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
        };

        # Runtime dependencies
        runtimeDeps = with pkgs; [
          webkitgtk_4_1
          gtk3
          glib
          gdk-pixbuf
          cairo
          pango
          harfbuzz
          librsvg
          libsoup_3
          openssl
          alsa-lib
          libxkbcommon

          # GStreamer for audio/video
          gst_all_1.gstreamer
          gst_all_1.gst-plugins-base
          gst_all_1.gst-plugins-good
          gst_all_1.gst-plugins-bad
          gst_all_1.gst-plugins-ugly
          gst_all_1.gst-libav
          gst_all_1.gst-vaapi

          # For typing text
          wtype
          ydotool

          # ONNX Runtime for local transcription
          onnxruntime
        ];

        # Build dependencies
        buildDeps = with pkgs; [
          pkg-config
          gobject-introspection
          cmake
          clang
          llvmPackages.libclang
          # Vulkan for whisper-rs GPU acceleration
          vulkan-headers
          vulkan-loader
          shaderc
          # ONNX Runtime for whisper/moonshine models
          onnxruntime
        ];

        # Fetch node_modules as a fixed-output derivation
        nodeModules = pkgs.stdenv.mkDerivation {
          pname = "omegawhisper-node-modules";
          version = "0.1.0";

          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let baseName = baseNameOf path;
              in baseName == "package.json" || baseName == "bun.lockb" || baseName == "bun.lock";
          };

          nativeBuildInputs = [ pkgs.bun pkgs.cacert ];

          buildPhase = ''
            export HOME=$(mktemp -d)
            bun install --frozen-lockfile
          '';

          installPhase = ''
            mkdir -p $out
            cp -r node_modules $out/
          '';

          outputHashMode = "recursive";
          outputHashAlgo = "sha256";
          outputHash = "sha256-3Mbrhp7OEQQIPt8ZYVo0P0CYrgIcpCLgePjgaVXuz1Q=";
        };

        # Fetch cargo dependencies
        # To update this hash after changing Cargo.toml: set to "" and run `nix build`, then copy the "got:" hash
        cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
          src = ./src-tauri;
          hash = "sha256-jj1Rov0DgulNQDFSwc95Vr7o1wDntAEKMbSebc0GZl0=";
        };

        # Build the frontend
        frontend = pkgs.stdenv.mkDerivation {
          pname = "omegawhisper-frontend";
          version = "0.1.0";

          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let
                baseName = baseNameOf path;
                relativePath = pkgs.lib.removePrefix (toString ./. + "/") path;
              in
              # Include frontend source files
              baseName == "package.json"
              || baseName == "tsconfig.json"
              || baseName == "tsconfig.node.json"
              || baseName == "vite.config.ts"
              || baseName == "index.html"
              || pkgs.lib.hasPrefix "src/" relativePath
              || baseName == "src"
              || baseName == "components.json";
          };

          nativeBuildInputs = [ pkgs.bun pkgs.nodejs-slim ];

          buildPhase = ''
            export HOME=$(mktemp -d)

            # Copy node_modules (need writable copy to patch shebangs)
            cp -r ${nodeModules}/node_modules node_modules
            chmod -R u+w node_modules

            # Patch shebangs in node_modules binaries
            patchShebangs node_modules

            # Build using bun directly instead of npm scripts
            ./node_modules/.bin/tsc
            ./node_modules/.bin/vite build
          '';

          installPhase = ''
            mkdir -p $out
            cp -r dist/* $out/
          '';
        };

      in
      {
        packages.default = pkgs.stdenv.mkDerivation {
          pname = "omegawhisper";
          version = "0.1.0";

          src = pkgs.lib.cleanSource ./.;

          nativeBuildInputs = buildDeps ++ (with pkgs; [
            makeWrapper
            rustToolchain
            cargo-tauri
          ]);

          buildInputs = runtimeDeps ++ (with pkgs; [
            at-spi2-atk
            atkmm
          ]);

          # Environment variables for build
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          ORT_LIB_LOCATION = "${pkgs.onnxruntime}/lib";

          # Don't run cmake configure phase
          dontUseCmakeConfigure = true;

          buildPhase = ''
            runHook preBuild

            # Copy frontend dist
            mkdir -p dist
            cp -r ${frontend}/* dist/

            # Debug: show what's in dist
            echo "Frontend dist contents:"
            ls -la dist/

            # No icon step. Every icon is a committed file in src-tauri/icons/,
            # so there is nothing here that can fail before the app has a face.

            # Setup cargo vendor directory
            mkdir -p .cargo
            cat > .cargo/config.toml <<EOF
            [source.crates-io]
            replace-with = "vendored-sources"

            [source.vendored-sources]
            directory = "${cargoDeps}"
            EOF

            # Also put cargo config in src-tauri
            mkdir -p src-tauri/.cargo
            cp .cargo/config.toml src-tauri/.cargo/

            # Build using cargo-tauri for proper frontend embedding
            # Skip the beforeBuildCommand since we already built the frontend
            export TAURI_SKIP_DEVSERVER_CHECK=true

            # Use cargo tauri build which properly embeds frontend
            # Pass --ci to skip prompts, --no-bundle to skip packaging
            # Override config to skip beforeBuildCommand (frontend already built)
            cargo tauri build --no-bundle --ci --config '{"build":{"beforeBuildCommand":""}}' -- --offline

            echo "Build complete, checking for binary:"
            ls -la src-tauri/target/release/

            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall

            mkdir -p $out/bin
            cp src-tauri/target/release/omegawhisper $out/bin/

            wrapProgram $out/bin/omegawhisper \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath runtimeDeps}" \
              --prefix PATH : "${pkgs.lib.makeBinPath [ pkgs.wtype pkgs.ydotool ]}"

            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "Type 3x faster, without lifting a finger";
            homepage = "https://github.com/webtemp/omegawhisper";
            license = licenses.gpl3Plus;
            maintainers = [ ];
            platforms = platforms.linux;
            mainProgram = "omegawhisper";
          };
        };

        # Expose nodeModules for hash updating
        packages.nodeModules = nodeModules;

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = buildDeps ++ (with pkgs; [
            rustToolchain
            cargo-tauri
            bun
            nodejs-slim
          ]);

          buildInputs = runtimeDeps ++ (with pkgs; [
            at-spi2-atk
            atkmm
          ]);

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeDeps;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          shellHook = ''
            echo "Omegawhisper development environment"
            echo "Run 'bun run tauri dev' to start development"
          '';
        };
      }
    );
}
