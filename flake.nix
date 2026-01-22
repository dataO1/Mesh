{
  description = "Mesh - DJ Player and Cue Software";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nn-tilde = {
      url = "github:acids-ircam/nn_tilde";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, nn-tilde }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        # Build nn~ external for Pure Data
        nn-external = pkgs.stdenv.mkDerivation {
          pname = "nn-tilde";
          version = "unstable";
          src = nn-tilde;

          nativeBuildInputs = with pkgs; [
            cmake
            pkg-config
          ];

          buildInputs = with pkgs; [
            puredata
            libtorch-bin
          ];

          # CMakeLists.txt is in src/ subdirectory
          cmakeDir = "../src";

          cmakeFlags = [
            "-DCMAKE_BUILD_TYPE=Release"
            "-DPD_INCLUDE_DIR=${pkgs.puredata}/include/pd"
          ];

          installPhase = ''
            mkdir -p $out/lib/pd-externals
            find . -name "*.pd_linux" -exec cp {} $out/lib/pd-externals/ \; || true
          '';
        };

        # Build Essentia library from source (nixpkgs only has binary extractor)
        # Required for mesh-cue's essentia-rs bindings
        # Using master branch for Python 3.12+ compatibility (WAF updates)
        essentia = pkgs.stdenv.mkDerivation rec {
          pname = "essentia";
          version = "2.1_beta6-dev";

          src = pkgs.fetchFromGitHub {
            owner = "MTG";
            repo = "essentia";
            rev = "17484ff0256169f14a959d62aa89a1463fead13f";
            hash = "sha256-q+TI03Y5Mw9W+ZNE8I1fEWvn3hjRyaxb7M6ZgntA8RA=";
          };

          nativeBuildInputs = with pkgs; [
            python3
            pkg-config
          ];

          buildInputs = with pkgs; [
            eigen
            fftwFloat
            taglib
            chromaprint
            libsamplerate
            libyaml
            ffmpeg_4-headless  # Headless: no SDL/GUI deps (~700MB smaller closure)
            zlib      # Required for linking
          ];

          configurePhase = ''
            runHook preConfigure
            python3 waf configure \
              --prefix=$out \
              --mode=release
            runHook postConfigure
          '';

          buildPhase = ''
            runHook preBuild
            python3 waf build -j $NIX_BUILD_CORES
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            python3 waf install
            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "Audio analysis and audio-based music information retrieval library";
            homepage = "https://essentia.upf.edu/";
            license = licenses.agpl3Plus;
            platforms = platforms.linux;
          };
        };

        # Runtime dependencies for development
        runtimeInputs = with pkgs; [
          # Core runtime (C++ stdlib needed by many deps)
          stdenv.cc.cc.lib  # libstdc++.so.6

          # Audio (libjack2 = client library only, no Python/FireWire bloat)
          libjack2
          alsa-lib
          pipewire

          # GUI (iced dependencies)
          wayland
          libxkbcommon
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
          vulkan-loader
          libGL

          # Misc
          openssl
        ];

        # Build-time only dependencies
        buildOnlyInputs = with pkgs; [
          glibc.dev    # Headers for cc-rs crates
        ];

        # Combined build inputs
        buildInputs = runtimeInputs ++ buildOnlyInputs;

        # Library paths for runtime
        libraryPath = pkgs.lib.makeLibraryPath runtimeInputs;

        # =======================================================================
        # Package Builds for Distribution (.deb/.rpm)
        # =======================================================================

        # Filtered source for Rust builds - only includes files needed for compilation
        # This prevents rebuilds when unrelated files change (distrobox.ini, packaging/, etc.)
        rustSrc = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let
              baseName = baseNameOf path;
              relPath = pkgs.lib.removePrefix (toString ./. + "/") path;
            in
            # Always include directories (filter will recurse into them)
            type == "directory" ||
            # Cargo files
            baseName == "Cargo.toml" ||
            baseName == "Cargo.lock" ||
            # Rust source files
            pkgs.lib.hasSuffix ".rs" baseName;
        };

        # Filtered source for .deb packaging - only Cargo.toml (metadata) and packaging/
        # This is very small, so meshDeb rebuilds are fast even when triggered
        debSrc = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let
              baseName = baseNameOf path;
              relPath = pkgs.lib.removePrefix (toString ./. + "/") path;
            in
            type == "directory" ||
            baseName == "Cargo.toml" ||
            baseName == "Cargo.lock" ||
            pkgs.lib.hasPrefix "packaging/" relPath;
        };

        # Common build inputs for Rust packages
        meshBuildInputs = runtimeInputs ++ [
          essentia
        ] ++ (with pkgs; [
          eigen
          fftwFloat
          taglib
          chromaprint
          libsamplerate
          libyaml
          ffmpeg_4-headless
          zlib
        ]);

        # Build the Rust workspace using rustPlatform (handles dependency fetching)
        # Uses filtered rustSrc to avoid rebuilds when non-Rust files change
        meshBuild = pkgs.rustPlatform.buildRustPackage {
          pname = "mesh";
          version = "0.1.0";
          src = rustSrc;

          # Cargo.lock hash - update this when deps change
          # Run: nix build .#mesh-build 2>&1 | grep "got:" to get new hash
          cargoHash = "sha256-jMe47CJzn8W/fzTe0BNAUy+QsSMyeDSzp3866s67aeM=";

          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            clang
            llvmPackages.libclang
            gnumake
          ];

          buildInputs = meshBuildInputs;

          # Build environment
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include -isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/21/include";
          PKG_CONFIG_PATH = "${essentia}/lib/pkgconfig";
          USE_TENSORFLOW = "0";
          CPLUS_INCLUDE_PATH = "${pkgs.eigen}/include/eigen3";

          # Build specific packages
          cargoBuildFlags = [ "-p" "mesh-player" "-p" "mesh-cue" ];

          meta = with pkgs.lib; {
            description = "DJ Player and Cue Software";
            license = licenses.agpl3Plus;
          };
        };

        # Create portable .deb packages with patchelf
        # This uses the pre-built binaries from meshBuild
        # Uses filtered debSrc (only Cargo.toml + packaging/) for fast rebuilds
        meshDeb = pkgs.stdenv.mkDerivation {
          pname = "mesh-deb";
          version = "0.1.0";
          src = debSrc;

          nativeBuildInputs = with pkgs; [
            patchelf
            cargo-deb
            rustToolchain  # For cargo-deb
          ];

          # No build phase - we use pre-built binaries
          dontBuild = true;
          dontConfigure = true;

          installPhase = ''
            runHook preInstall

            # Create target directory structure that cargo-deb expects
            mkdir -p target/release/bundled

            # Copy pre-built binaries from meshBuild
            cp ${meshBuild}/bin/mesh-player target/release/
            cp ${meshBuild}/bin/mesh-cue target/release/

            echo "=== Patching binaries for portability ==="
            # Make binaries writable for patchelf
            chmod +w target/release/mesh-player target/release/mesh-cue

            # Remove Nix store paths from RUNPATH, set to standard Linux + bundled lib path
            patchelf --set-rpath '/usr/lib/x86_64-linux-gnu:/usr/lib' target/release/mesh-player
            patchelf --set-rpath '/usr/lib/mesh:/usr/lib/x86_64-linux-gnu:/usr/lib' target/release/mesh-cue

            # Set interpreter to standard Linux path
            patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 target/release/mesh-player
            patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 target/release/mesh-cue

            echo "=== Verifying patchelf results ==="
            patchelf --print-rpath target/release/mesh-player
            patchelf --print-rpath target/release/mesh-cue

            echo "=== Staging bundled libraries ==="
            # Bundle libessentia
            cp ${essentia}/lib/libessentia.so target/release/bundled/
            chmod +w target/release/bundled/libessentia.so

            # Bundle FFmpeg 4.x libraries (essentia depends on these, modern distros have FFmpeg 6.x)
            # Only copy the versioned .so files, not symlinks
            cp ${pkgs.ffmpeg_4-headless.lib}/lib/libavcodec.so.58.134.100 target/release/bundled/libavcodec.so.58
            cp ${pkgs.ffmpeg_4-headless.lib}/lib/libavformat.so.58.76.100 target/release/bundled/libavformat.so.58
            cp ${pkgs.ffmpeg_4-headless.lib}/lib/libavutil.so.56.70.100 target/release/bundled/libavutil.so.56
            cp ${pkgs.ffmpeg_4-headless.lib}/lib/libswresample.so.3.9.100 target/release/bundled/libswresample.so.3

            # Patch libessentia to find FFmpeg in /usr/lib/mesh/ instead of Nix store
            patchelf --set-rpath '/usr/lib/mesh:/usr/lib/x86_64-linux-gnu:/usr/lib' target/release/bundled/libessentia.so

            echo "Bundled libraries:"
            ls -lh target/release/bundled/

            echo "=== Creating .deb packages ==="
            cargo deb -p mesh-player --no-build --no-strip
            cargo deb -p mesh-cue --no-build --no-strip

            # Copy outputs
            mkdir -p $out
            cp target/debian/*.deb $out/

            echo "=== Build complete ==="
            ls -la $out/

            runHook postInstall
          '';
        };

      in
      {
        # Export packages
        packages = {
          mesh-build = meshBuild;
          mesh-deb = meshDeb;
          default = meshDeb;
        };

        devShells.default = pkgs.mkShell {
          name = "mesh-dev-shell";

          # mkShell properly adds all packages to PATH (unlike stdenv.mkDerivation)
          packages = buildInputs ++ [
            # Custom essentia library (built from source)
            essentia
          ] ++ (with pkgs; [
            # Essentia dependencies (needed for essentia-sys pkg-config)
            eigen
            fftwFloat
            taglib
            chromaprint
            libsamplerate
            libyaml
            ffmpeg_4-headless  # Headless: no SDL/GUI deps (~700MB smaller closure)
            zlib      # Required for linking

            # Development tools
            rustToolchain
            rust-analyzer
            cargo-watch
            cargo-edit
            cargo-expand
            pkg-config
            cmake
            clang
            llvmPackages.libclang
            gcc.cc  # For C++ stdlib
            gnumake  # For libffi-sys build
            autoconf
            automake
            libtool

            # Debugging
            gdb
            lldb

            # Database inspection (CozoDB uses SQLite backend)
            sqlite

            # Package testing (requires podman on host, see shellHook)
            distrobox
          ]);

          shellHook = ''
            # Rust environment
            export RUST_BACKTRACE=1

            # Logging: only show mesh-* crate logs at info level, filter out noisy dependencies
            export RUST_LOG="warn,mesh_core=debug,mesh_cue=debug,mesh_player=debug"

            # Library paths
            export LD_LIBRARY_PATH="${libraryPath}:$LD_LIBRARY_PATH"

            # Ensure GNU make is in PATH first and used everywhere (required by libffi-sys)
            # Create a temp bin dir with make symlink to ensure GNU make is used
            export MESH_MAKE_DIR=$(mktemp -d)
            ln -sf ${pkgs.gnumake}/bin/make $MESH_MAKE_DIR/make
            ln -sf ${pkgs.gnumake}/bin/make $MESH_MAKE_DIR/gmake
            ln -sf ${pkgs.cmake}/bin/cmake $MESH_MAKE_DIR/cmake
            export PATH="$MESH_MAKE_DIR:${pkgs.gnumake}/bin:${pkgs.cmake}/bin:$PATH"
            export MAKE="${pkgs.gnumake}/bin/make"

            # Use clang for C/C++ compilation (better nix compatibility than gcc)
            export CC="${pkgs.clang}/bin/clang"
            export CXX="${pkgs.clang}/bin/clang++"

            # Clang/LLVM for bindgen (only for Rust FFI generation)
            export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"

            # Clang needs to know where headers and libs are in nix
            # Use -idirafter for glibc so it comes AFTER C++ headers (for #include_next)
            export CFLAGS="-idirafter ${pkgs.glibc.dev}/include -isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/21/include"
            export CXXFLAGS="-isystem ${pkgs.gcc.cc}/include/c++/${pkgs.gcc.version} -isystem ${pkgs.gcc.cc}/include/c++/${pkgs.gcc.version}/x86_64-unknown-linux-gnu -idirafter ${pkgs.glibc.dev}/include -isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/21/include"
            export LDFLAGS="-L${pkgs.glibc}/lib -L${pkgs.gcc.cc.lib}/lib"

            # Bindgen needs to know where C headers are (glibc + clang builtins)
            export BINDGEN_EXTRA_CLANG_ARGS="-isystem ${pkgs.glibc.dev}/include -isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/21/include"

            # PD externals path (nn~ and others)
            # nn-external will be built separately; for now just use local externals
            export PD_EXTERNALS="./effects/pd/externals"

            # JACK settings
            export JACK_NO_AUDIO_RESERVATION=1

            # Torch library path (for nn~)
            export LIBTORCH="${pkgs.libtorch-bin}"
            export LIBTORCH_LIB="${pkgs.libtorch-bin}/lib"
            export LIBTORCH_INCLUDE="${pkgs.libtorch-bin}/include"

            # Essentia library (built from source for mesh-cue)
            export PKG_CONFIG_PATH="${essentia}/lib/pkgconfig:$PKG_CONFIG_PATH"
            export LD_LIBRARY_PATH="${essentia}/lib:$LD_LIBRARY_PATH"
            # Disable TensorFlow in essentia-sys (not needed for BPM/key detection)
            export USE_TENSORFLOW=0
            # Fix Eigen include path for essentia-sys (it incorrectly appends /eigen3)
            export CPLUS_INCLUDE_PATH="${pkgs.eigen}/include/eigen3:$CPLUS_INCLUDE_PATH"

            # Vulkan for iced
            export VK_ICD_FILENAMES="${pkgs.vulkan-loader}/share/vulkan/icd.d/intel_icd.x86_64.json:${pkgs.vulkan-loader}/share/vulkan/icd.d/radeon_icd.x86_64.json"

            echo ""
            echo "╔═══════════════════════════════════════════════════════════════════════╗"
            echo "║                      Mesh Development Shell                           ║"
            echo "╠═══════════════════════════════════════════════════════════════════════╣"
            echo "║  Development:                                                         ║"
            echo "║    cargo run -p mesh-player          # DJ application                 ║"
            echo "║    cargo run -p mesh-cue             # Track preparation              ║"
            echo "║    cargo test                        # Run all tests                  ║"
            echo "╠═══════════════════════════════════════════════════════════════════════╣"
            echo "║  Build portable .deb packages:                                        ║"
            echo "║    nix build .#mesh-deb             # Build .debs → ./result/         ║"
            echo "╠═══════════════════════════════════════════════════════════════════════╣"
            echo "║  Test .deb packages (requires: virtualisation.podman.enable = true)   ║"
            echo "║    distrobox assemble create        # Create container + auto-install ║"
            echo "║    distrobox enter mesh-ubuntu      # Enter and test (mesh-player)    ║"
            echo "║    distrobox assemble rm            # Clean up when done              ║"
            echo "╚═══════════════════════════════════════════════════════════════════════╝"
            echo ""
          '';
        };

        # For CI/CD or quick checks
        checks = {
          format = pkgs.runCommand "check-format" {
            nativeBuildInputs = [ rustToolchain ];
          } ''
            cd ${./.}
            cargo fmt --check
            touch $out
          '';
        };
      }
    );
}
