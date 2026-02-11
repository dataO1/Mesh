# Container-based portable .deb packaging
# Uses Ubuntu 22.04 for glibc 2.35 compatibility (Pop!_OS 22.04, Ubuntu 22.04+)
#
# Usage:
#   nix run .#build-deb           # CPU-only build (works everywhere)
#   nix run .#build-deb-cuda      # NVIDIA CUDA 12 GPU acceleration
#
# Output: dist/deb/mesh-player_*.deb, mesh-cue_*.deb (or mesh-cue-cuda_*.deb)
#
# Prerequisites:
#   - Podman or Docker installed and running
#   - NixOS: virtualisation.podman.enable = true (or docker)
#   - For CUDA builds: NVIDIA drivers + CUDA 12 toolkit on target system
#
# Why container? Nix uses a newer glibc than most distros. Building in Ubuntu 22.04
# ensures the binaries work on Pop!_OS 22.04, Ubuntu 22.04, Debian 12, etc.
#
# ═══════════════════════════════════════════════════════════════════════════════
# ARCHITECTURE OVERVIEW
# ═══════════════════════════════════════════════════════════════════════════════
#
# This builds portable .deb packages that work on older distros by:
#   1. Building in Ubuntu 22.04 container (glibc 2.35)
#   2. Bundling libraries not available in older distros:
#      - libessentia.so (not packaged in any distro)
#      - FFmpeg 4.x libs (distros have FFmpeg 6.x, ABI incompatible)
#      - libtag.so.2 (TagLib 2.x, older distros have 1.x)
#   3. Setting rpath to /usr/lib/mesh/ for bundled libs
#
# GPU Acceleration:
#   - CPU build (default): Works on any system, no external dependencies
#   - CUDA build: Requires NVIDIA GPU + CUDA 12 toolkit installed on target
#
# ═══════════════════════════════════════════════════════════════════════════════
{ pkgs, enableCuda ? false }:

pkgs.writeShellApplication {
  name = "build-deb${if enableCuda then "-cuda" else ""}";
  runtimeInputs = with pkgs; [ podman coreutils ];
  text = ''
    set -euo pipefail

    # GPU acceleration settings (set at Nix build time)
    ENABLE_CUDA="${if enableCuda then "1" else "0"}"
    GPU_SUFFIX="${if enableCuda then "-cuda" else ""}"

    if [[ "$ENABLE_CUDA" == "1" ]]; then
      echo "╔═══════════════════════════════════════════════════════════════════════╗"
      echo "║       Portable .deb Build (Ubuntu 22.04 + NVIDIA CUDA 12)             ║"
      echo "╚═══════════════════════════════════════════════════════════════════════╝"
    else
      echo "╔═══════════════════════════════════════════════════════════════════════╗"
      echo "║           Portable .deb Build (Ubuntu 22.04 Container)                ║"
      echo "╚═══════════════════════════════════════════════════════════════════════╝"
    fi
    echo ""

    # Determine project root (where Cargo.toml is)
    PROJECT_ROOT="$(pwd)"
    if [[ ! -f "$PROJECT_ROOT/Cargo.toml" ]]; then
      echo "ERROR: Must be run from project root (where Cargo.toml is)"
      exit 1
    fi

    OUTPUT_DIR="$PROJECT_ROOT/dist/deb"
    TARGET_DIR="$PROJECT_ROOT/target/deb-build"
    # Ubuntu 22.04 = glibc 2.35 (compatible with Pop!_OS 22.04+)
    IMAGE="docker.io/library/ubuntu:22.04"

    echo "Project root:    $PROJECT_ROOT"
    echo "Output dir:      $OUTPUT_DIR"
    echo "Container:       $IMAGE"
    echo ""

    # Create output directory
    mkdir -p "$OUTPUT_DIR"
    mkdir -p "$TARGET_DIR"

    echo "==> Pulling Ubuntu 22.04 image (if needed)..."
    podman pull "$IMAGE" || {
      echo "Failed to pull image. Is podman running?"
      echo "On NixOS, enable: virtualisation.podman.enable = true"
      exit 1
    }

    echo ""
    echo "==> Building .deb packages in Ubuntu 22.04 container..."
    echo "    (First run installs build deps, subsequent builds are faster)"
    echo ""

    # Run the build in the container
    podman run --rm -i \
      -v "$PROJECT_ROOT:/project:ro" \
      -v "$TARGET_DIR:/build:rw" \
      -v "$OUTPUT_DIR:/output:rw" \
      -e "ENABLE_CUDA=$ENABLE_CUDA" \
      -e "GPU_SUFFIX=$GPU_SUFFIX" \
      -w /project \
      "$IMAGE" \
      bash << 'CONTAINER_SCRIPT'
        set -e

        # =====================================================================
        # Phase 1: Install build tools and dependencies
        # =====================================================================
        echo "==> [1/8] Installing build dependencies..."
        echo "    Updating apt cache..."
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq

        # Install CMake 3.25+ from Kitware APT repo (Ubuntu 22.04 ships 3.22, libpd needs 3.25)
        echo "    Adding Kitware APT repository for CMake 3.25+..."
        apt-get install -y ca-certificates gpg wget 2>&1 | grep -E "^(Setting up|Processing)" || true
        wget -qO - https://apt.kitware.com/keys/kitware-archive-latest.asc | gpg --dearmor -o /usr/share/keyrings/kitware-archive-keyring.gpg
        echo 'deb [signed-by=/usr/share/keyrings/kitware-archive-keyring.gpg] https://apt.kitware.com/ubuntu/ jammy main' > /etc/apt/sources.list.d/kitware.list
        apt-get update -qq

        echo "    Installing packages (this takes 1-2 minutes on first run)..."
        apt-get install -y \
          curl \
          build-essential \
          pkg-config \
          cmake \
          git \
          python3 \
          patchelf \
          libclang-dev \
          libasound2-dev \
          libvulkan-dev \
          libxkbcommon-dev \
          libwayland-dev \
          libfontconfig1-dev \
          libx11-dev \
          libx11-xcb-dev \
          libxcb1-dev \
          libxcursor-dev \
          libxrandr-dev \
          libxi-dev \
          libssl-dev \
          libjack-jackd2-dev \
          libfftw3-dev \
          libtag1-dev \
          libchromaprint-dev \
          libsamplerate0-dev \
          libyaml-dev \
          libeigen3-dev \
          yasm \
          nasm \
          2>&1 | grep -E "^(Get:|Hit:|Fetched|Setting up|Processing)" || true

        echo "    Build dependencies installed."

        # Use cached Rust installation in build volume
        export RUSTUP_HOME=/build/rustup
        export CARGO_HOME=/build/cargo

        echo ""
        echo "==> [2/8] Setting up Rust toolchain..."
        echo "    Cache contents: $(ls /build 2>/dev/null | tr '\n' ' ' || echo '(empty)')"

        if [[ -f "$CARGO_HOME/bin/cargo" ]]; then
          echo "    Rust already installed (cached)"
          echo "    Version: $($CARGO_HOME/bin/rustc --version)"
        else
          echo "    Installing Rust (will be cached for future builds)..."
          curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
        fi
        export PATH="$CARGO_HOME/bin:$PATH"

        if [[ -f "$CARGO_HOME/bin/cargo-deb" ]]; then
          echo "    cargo-deb already installed (cached)"
        else
          echo "    Installing cargo-deb..."
          cargo install cargo-deb
        fi
        echo "    Rust toolchain ready."

        # =====================================================================
        # Phase 3: Build FFmpeg 4.x (distros have 6.x which is ABI-incompatible)
        # =====================================================================
        echo ""
        echo "==> [3/8] Building FFmpeg 4.x (required by Essentia)..."
        DEPS_PREFIX=/build/deps
        mkdir -p "$DEPS_PREFIX"/{lib,include,lib/pkgconfig}

        if [[ -f "$DEPS_PREFIX/lib/libavcodec.so.58" ]]; then
          echo "    Already built (cached)"
        else
          echo "    Downloading FFmpeg 4.4.4..."
          cd /tmp
          curl -L https://ffmpeg.org/releases/ffmpeg-4.4.4.tar.xz | tar xJ
          cd ffmpeg-4.4.4
          echo "    Configuring..."
          ./configure --prefix="$DEPS_PREFIX" \
            --enable-shared --disable-static \
            --disable-programs --disable-doc \
            --disable-avdevice --disable-postproc --disable-avfilter \
            --disable-network --disable-encoders --disable-muxers \
            --disable-bsfs --disable-devices --disable-filters \
            --enable-small
          echo "    Compiling (this takes a few minutes)..."
          make -j$(nproc)
          echo "    Installing..."
          make install
          cd /project
          rm -rf /tmp/ffmpeg-4.4.4
          echo "    FFmpeg 4.4.4 build complete!"
        fi

        # =====================================================================
        # Phase 4: Build TagLib 2.x (older distros only have 1.x)
        # =====================================================================
        echo ""
        echo "==> [4/8] Building TagLib 2.x (audio metadata library)..."
        if [[ -f "$DEPS_PREFIX/lib/libtag.so.2" ]]; then
          echo "    Already built (cached)"
        else
          cd /tmp
          echo "    Cloning TagLib 2.0.2 with submodules..."
          git clone --depth 1 --branch v2.0.2 --recurse-submodules https://github.com/taglib/taglib.git taglib-2.0.2
          cd taglib-2.0.2
          mkdir build && cd build
          echo "    Configuring..."
          if ! cmake .. -DCMAKE_INSTALL_PREFIX="$DEPS_PREFIX" \
            -DBUILD_SHARED_LIBS=ON \
            -DCMAKE_BUILD_TYPE=Release; then
            echo "ERROR: TagLib cmake failed"
            exit 1
          fi
          echo "    Compiling..."
          if ! make -j$(nproc); then
            echo "ERROR: TagLib make failed"
            exit 1
          fi
          echo "    Installing..."
          make install
          cd /project
          rm -rf /tmp/taglib-2.0.2
          echo "    TagLib 2.0.2 build complete!"
        fi

        # =====================================================================
        # Phase 5: Build Essentia (not packaged in any distro)
        # =====================================================================
        echo ""
        echo "==> [5/8] Building Essentia (audio analysis library)..."
        if [[ -f "$DEPS_PREFIX/lib/libessentia.so" ]]; then
          echo "    Already built (cached)"
        else
          cd /tmp

          # Set environment to find our FFmpeg and TagLib
          export PKG_CONFIG_PATH="$DEPS_PREFIX/lib/pkgconfig:$PKG_CONFIG_PATH"
          export LD_LIBRARY_PATH="$DEPS_PREFIX/lib:$LD_LIBRARY_PATH"
          export LIBRARY_PATH="$DEPS_PREFIX/lib:$LIBRARY_PATH"
          export CPLUS_INCLUDE_PATH="$DEPS_PREFIX/include:$CPLUS_INCLUDE_PATH"

          echo "    Cloning Essentia repository..."
          git clone --depth 1 https://github.com/MTG/essentia.git
          cd essentia

          echo "    Configuring..."
          # Include both our custom deps AND system paths for eigen3, fftw3, etc.
          FULL_PKG_PATH="$DEPS_PREFIX/lib/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/lib/pkgconfig:/usr/share/pkgconfig"
          python3 waf configure \
            --prefix="$DEPS_PREFIX" \
            --pkg-config-path="$FULL_PKG_PATH" \
            --mode=release \
            --fft=FFTW

          echo "    Compiling (this takes 3-5 minutes)..."
          python3 waf build -j$(nproc)

          echo "    Installing..."
          python3 waf install

          cd /project
          rm -rf /tmp/essentia
          echo "    Essentia build complete!"
        fi

        # Update library search paths
        export PKG_CONFIG_PATH="$DEPS_PREFIX/lib/pkgconfig:$PKG_CONFIG_PATH"
        export LD_LIBRARY_PATH="$DEPS_PREFIX/lib:$LD_LIBRARY_PATH"
        export LIBRARY_PATH="$DEPS_PREFIX/lib:$LIBRARY_PATH"
        export CPLUS_INCLUDE_PATH="$DEPS_PREFIX/include:/usr/include/eigen3:$CPLUS_INCLUDE_PATH"

        # =====================================================================
        # Phase 6: Build Rust applications
        # =====================================================================
        echo ""
        echo "==> [6/8] Building Rust applications..."

        # Copy source to writable location (container mounts /project as read-only)
        # Exclude 'target' dir to avoid circular copy (target/deb-build is mounted as /build)
        echo "    Syncing source to build directory..."
        rm -rf /build/src
        mkdir -p /build/src
        cd /project
        find . -maxdepth 1 ! -name target ! -name . -exec cp -r {} /build/src/ \;
        cd /build/src

        # Use cached target directory (persists compiled dependencies between runs)
        export CARGO_TARGET_DIR=/build/target

        # Environment for essentia-sys
        export USE_TENSORFLOW=0

        echo "    Building mesh-player..."
        echo "    (First build compiles all dependencies, subsequent builds are incremental)"
        cargo build --release -p mesh-player

        echo ""
        echo "    Building mesh-cue..."
        # GPU feature is passed from Nix via environment variable
        # Always use load-dynamic to avoid glibc version mismatch with ort's pre-built binaries
        # (pyke.io binaries are built against glibc 2.38+, Ubuntu 22.04 has 2.35)
        #
        # IMPORTANT: Clean mesh-cue and ort to prevent feature flag contamination
        # If cuda build ran previously, incremental compilation would reuse cuda-enabled ort
        echo "    Cleaning mesh-cue and ort (prevents cuda/non-cuda feature mixing)..."
        cargo clean --release -p mesh-cue -p ort 2>/dev/null || true
        #
        # WORKAROUND: Build essentia first without load-dynamic feature
        # load-dynamic causes essentia-codegen to fail (cargo feature/build order issue)
        echo "    Building essentia first (without load-dynamic)..."
        cargo build --release -p essentia -p essentia-sys 2>/dev/null || true

        if [[ "''${ENABLE_CUDA:-0}" == "1" ]]; then
          echo "    (with CUDA 12 GPU acceleration + load-dynamic)"
          cargo build --release -p mesh-cue --features cuda,load-dynamic
        else
          echo "    (with load-dynamic for ONNX Runtime)"
          cargo build --release -p mesh-cue --features load-dynamic
        fi
        echo "    Rust builds complete!"

        # =====================================================================
        # Phase 7: Prepare bundled libraries
        # =====================================================================
        echo ""
        echo "==> [7/8] Bundling libraries for portability..."
        mkdir -p "$CARGO_TARGET_DIR/release/bundled"

        # Download ONNX Runtime from Microsoft (compatible with older glibc)
        # Use GPU version for CUDA builds, CPU version otherwise
        ORT_VERSION="1.23.2"
        if [[ "''${ENABLE_CUDA:-0}" == "1" ]]; then
          ORT_VARIANT="gpu"
          ORT_CACHE="/build/onnxruntime-gpu-$ORT_VERSION"
          ORT_TARBALL="onnxruntime-linux-x64-gpu-$ORT_VERSION"
        else
          ORT_VARIANT="cpu"
          ORT_CACHE="/build/onnxruntime-cpu-$ORT_VERSION"
          ORT_TARBALL="onnxruntime-linux-x64-$ORT_VERSION"
        fi

        if [[ -f "$ORT_CACHE/libonnxruntime.so" ]]; then
          echo "    ONNX Runtime ($ORT_VARIANT) already downloaded (cached)"
        else
          echo "    Downloading ONNX Runtime $ORT_VERSION ($ORT_VARIANT) from Microsoft..."
          mkdir -p "$ORT_CACHE"
          cd /tmp
          curl -sL "https://github.com/microsoft/onnxruntime/releases/download/v$ORT_VERSION/$ORT_TARBALL.tgz" | tar xz
          cp "$ORT_TARBALL/lib/libonnxruntime.so.$ORT_VERSION" "$ORT_CACHE/libonnxruntime.so"
          rm -rf "$ORT_TARBALL"
          cd /build/src
          echo "    Downloaded ONNX Runtime $ORT_VERSION ($ORT_VARIANT)"
        fi

        echo "    Copying libraries to bundle..."
        cp -v "$DEPS_PREFIX/lib/libessentia.so" "$CARGO_TARGET_DIR/release/bundled/"
        cp -v "$DEPS_PREFIX/lib/libavcodec.so.58" "$CARGO_TARGET_DIR/release/bundled/"
        cp -v "$DEPS_PREFIX/lib/libavformat.so.58" "$CARGO_TARGET_DIR/release/bundled/"
        cp -v "$DEPS_PREFIX/lib/libavutil.so.56" "$CARGO_TARGET_DIR/release/bundled/"
        cp -v "$DEPS_PREFIX/lib/libswresample.so.3" "$CARGO_TARGET_DIR/release/bundled/"
        cp -v "$DEPS_PREFIX/lib/libtag.so.2" "$CARGO_TARGET_DIR/release/bundled/"
        cp -v "$ORT_CACHE/libonnxruntime.so" "$CARGO_TARGET_DIR/release/bundled/"

        echo ""
        echo "    Patching rpath for portability..."
        patchelf --set-rpath "/usr/lib/mesh:/usr/lib/x86_64-linux-gnu:/usr/lib" "$CARGO_TARGET_DIR/release/mesh-cue"
        patchelf --set-rpath "/usr/lib/x86_64-linux-gnu:/usr/lib" "$CARGO_TARGET_DIR/release/mesh-player"

        for lib in "$CARGO_TARGET_DIR"/release/bundled/*.so*; do
          echo "    Patching: $(basename $lib)"
          patchelf --set-rpath "/usr/lib/mesh:/usr/lib/x86_64-linux-gnu:/usr/lib" "$lib" 2>/dev/null || true
        done

        echo ""
        echo "    Bundled libraries:"
        ls -lh "$CARGO_TARGET_DIR/release/bundled/"

        # =====================================================================
        # Phase 8: Create .deb packages
        # =====================================================================
        echo ""
        echo "==> [8/8] Creating .deb packages..."
        echo "    Building mesh-player.deb..."
        cargo deb -p mesh-player --no-build --no-strip

        echo "    Building mesh-cue.deb..."
        cargo deb -p mesh-cue --no-build --no-strip

        # Rename mesh-cue package for CUDA builds to avoid overwriting CPU build
        if [[ -n "$GPU_SUFFIX" ]]; then
          for deb in "$CARGO_TARGET_DIR/debian/"mesh-cue_*.deb; do
            newname=$(echo "$deb" | sed "s/mesh-cue_/mesh-cue''${GPU_SUFFIX}_/")
            mv "$deb" "$newname"
            echo "    Renamed: $(basename "$deb") → $(basename "$newname")"
          done
        fi

        # Copy to output
        cp "$CARGO_TARGET_DIR/debian/"*.deb /output/

        echo ""
        echo "=========================================="
        echo "Packages created successfully:"
        ls -lh /output/*.deb
        echo "=========================================="
CONTAINER_SCRIPT

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║                        Build Complete!                                ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Portable .deb packages ready in dist/deb/                            ║"
    echo "║                                                                       ║"
    echo "║  Compatible with:                                                     ║"
    echo "║    - Pop!_OS 22.04+                                                   ║"
    echo "║    - Ubuntu 22.04+                                                    ║"
    echo "║    - Debian 12+                                                       ║"
    echo "║    - Linux Mint 21+                                                   ║"
    if [[ "$ENABLE_CUDA" == "1" ]]; then
    echo "║                                                                       ║"
    echo "║  GPU: NVIDIA CUDA 12 acceleration enabled                             ║"
    echo "║       Requires: NVIDIA driver 525+ and CUDA 12 toolkit                ║"
    fi
    echo "║                                                                       ║"
    echo "║  Install with: sudo dpkg -i mesh-*.deb                                ║"
    echo "║                sudo apt-get install -f  # fix dependencies            ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "Distribution packages:"
    ls -lh "$OUTPUT_DIR/"*.deb 2>/dev/null || echo "  (no packages created)"
  '';
}
