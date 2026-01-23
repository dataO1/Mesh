# Container-based Windows cross-compilation
# Uses official Rust image + MinGW-w64 toolchain
#
# Usage: nix run .#build-windows
# Output: dist/windows/mesh-player.exe, mesh-cue.exe
#
# Prerequisites:
#   - Podman or Docker installed and running
#   - NixOS: virtualisation.podman.enable = true (or docker)
#
# Why container? Pure Nix cross-compilation fails due to MinGW-w64 pthreads
# __ImageBase linker errors in nixpkgs. Container has working toolchain.
{ pkgs }:

pkgs.writeShellApplication {
  name = "build-windows";
  runtimeInputs = with pkgs; [ podman coreutils ];
  text = ''
    set -euo pipefail

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║             Windows Cross-Compilation (Container)                     ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""

    # Determine project root (where Cargo.toml is)
    PROJECT_ROOT="$(pwd)"
    if [[ ! -f "$PROJECT_ROOT/Cargo.toml" ]]; then
      echo "ERROR: Must be run from project root (where Cargo.toml is)"
      exit 1
    fi

    OUTPUT_DIR="$PROJECT_ROOT/dist/windows"
    TARGET_DIR="$PROJECT_ROOT/target/windows"
    # Rust 1.88+ required for iced 0.14 and wgpu 27
    IMAGE="docker.io/library/rust:1.88"

    echo "Project root: $PROJECT_ROOT"
    echo "Output dir:   $OUTPUT_DIR"
    echo "Container:    $IMAGE"
    echo ""

    # Create output directory
    mkdir -p "$OUTPUT_DIR"
    mkdir -p "$TARGET_DIR"

    echo "==> Pulling Rust image (if needed)..."
    podman pull "$IMAGE" || {
      echo "Failed to pull image. Is podman running?"
      echo "On NixOS, enable: virtualisation.podman.enable = true"
      exit 1
    }

    echo ""
    echo "==> Building mesh-player for Windows..."
    echo "    (First run installs MinGW toolchain, subsequent builds are faster)"
    echo ""

    # Run the build in the container
    # - Install MinGW-w64 and Windows target on first run
    # - Mount source as /project
    # - Mount separate target dir (persisted for incremental builds)
    # - Mount cargo registry for caching
    podman run --rm \
      -v "$PROJECT_ROOT:/project:ro" \
      -v "$TARGET_DIR:/project/target:rw" \
      -w /project \
      "$IMAGE" \
      bash -c '
        set -e

        # =====================================================================
        # Phase 1: Install build tools
        # =====================================================================
        echo "==> Installing MinGW-w64 toolchain and build dependencies..."
        apt-get update -qq
        apt-get install -y -qq \
          gcc-mingw-w64-x86-64 \
          g++-mingw-w64-x86-64 \
          libclang-dev \
          perl \
          cmake \
          yasm \
          nasm \
          git \
          pkg-config \
          autoconf \
          automake \
          libtool \
          curl \
          xz-utils \
          >/dev/null 2>&1

        echo "==> Adding Windows target..."
        rustup target add x86_64-pc-windows-gnu

        # =====================================================================
        # Phase 2: Set up cross-compilation environment
        # =====================================================================
        export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
        export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
        export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
        export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc

        # Statically link MinGW runtime (libstdc++, libgcc, pthreads) for standalone .exe
        export RUSTFLAGS="-C link-args=-static-libgcc -C link-args=-static-libstdc++ -C link-args=-Wl,-Bstatic -C link-args=-lpthread -C link-args=-Wl,-Bdynamic"

        # Cross-compilation vars for autotools/cmake
        export CC=x86_64-w64-mingw32-gcc
        export CXX=x86_64-w64-mingw32-g++
        export AR=x86_64-w64-mingw32-ar
        export RANLIB=x86_64-w64-mingw32-ranlib
        export STRIP=x86_64-w64-mingw32-strip
        export HOST=x86_64-w64-mingw32

        # Tell bindgen/clang where MinGW headers are
        GCC_INCLUDE=$(find /usr/lib/gcc/x86_64-w64-mingw32 -name "include" -type d 2>/dev/null | head -1)
        export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/x86_64-w64-mingw32 -I/usr/x86_64-w64-mingw32/include ''${GCC_INCLUDE:+-I$GCC_INCLUDE}"

        # =====================================================================
        # Phase 3: Build Essentia for Windows (if not cached)
        # =====================================================================
        ESSENTIA_PREFIX=/project/target/essentia-win
        PKG_CONFIG_WIN="$ESSENTIA_PREFIX/lib/pkgconfig"

        if [[ -f "$ESSENTIA_PREFIX/lib/libessentia.a" ]]; then
          echo "==> Essentia already built (cached), skipping..."
        else
          echo "==> Building Essentia and dependencies for Windows..."
          echo "    (This takes a while on first run, but is cached for future builds)"
          echo ""

          # Install pre-built MinGW development packages where available
          echo "==> Installing MinGW development libraries..."
          apt-get install -y -qq \
            libfftw3-dev \
            libeigen3-dev \
            libyaml-dev \
            libsamplerate0-dev \
            >/dev/null 2>&1 || true

          mkdir -p /tmp/essentia-build
          cd /tmp/essentia-build

          # ---------------------------------------------------------------
          # Build dependencies manually with proper cross-compilation
          # ---------------------------------------------------------------
          DEPS_PREFIX="$ESSENTIA_PREFIX"
          mkdir -p "$DEPS_PREFIX"/{include,lib,lib/pkgconfig}

          # Reset CC/CXX for host builds (cmake toolchain will handle cross)
          unset CC CXX AR RANLIB

          # --- Eigen3 (header-only) ---
          if [[ -d "$DEPS_PREFIX/include/Eigen" ]]; then
            echo "==> Eigen3 already installed (cached)"
          else
            echo "==> Installing Eigen3 headers..."
            curl -sL https://gitlab.com/libeigen/eigen/-/archive/3.4.0/eigen-3.4.0.tar.gz | tar xz --no-same-owner
            cp -r eigen-3.4.0/Eigen "$DEPS_PREFIX/include/"
            cp -r eigen-3.4.0/unsupported "$DEPS_PREFIX/include/"
            rm -rf eigen-3.4.0
          fi
          cat > "$DEPS_PREFIX/lib/pkgconfig/eigen3.pc" << EIGENPC
prefix=$DEPS_PREFIX
includedir=\''${prefix}/include

Name: Eigen3
Description: Lightweight C++ template library for linear algebra
Version: 3.4.0
Cflags: -I\''${includedir}
EIGENPC

          # --- FFTW3 (build from source for MinGW) ---
          if [[ -f "$DEPS_PREFIX/lib/libfftw3f.a" ]]; then
            echo "==> FFTW3 already built (cached)"
          else
            echo "==> Building FFTW3 for Windows..."
            curl -sL http://www.fftw.org/fftw-3.3.10.tar.gz | tar xz --no-same-owner
            cd fftw-3.3.10
            ./configure --host=x86_64-w64-mingw32 --prefix="$DEPS_PREFIX" \
              --enable-float --enable-static --disable-shared \
              --with-our-malloc16 --disable-fortran \
              CC=x86_64-w64-mingw32-gcc 2>/dev/null
            make -j$(nproc) 2>/dev/null
            make install 2>/dev/null
            cd ..
            rm -rf fftw-3.3.10
          fi

          # --- libyaml ---
          if [[ -f "$DEPS_PREFIX/lib/libyaml.a" ]]; then
            echo "==> libyaml already built (cached)"
          else
            echo "==> Building libyaml for Windows..."
            curl -sL https://github.com/yaml/libyaml/releases/download/0.2.5/yaml-0.2.5.tar.gz | tar xz --no-same-owner
            cd yaml-0.2.5
            ./configure --host=x86_64-w64-mingw32 --prefix="$DEPS_PREFIX" \
              --enable-static --disable-shared \
              CC=x86_64-w64-mingw32-gcc 2>/dev/null
            make -j$(nproc) 2>/dev/null
            make install 2>/dev/null
            cd ..
            rm -rf yaml-0.2.5
          fi

          # --- TagLib ---
          if [[ -f "$DEPS_PREFIX/lib/libtag.a" ]]; then
            echo "==> TagLib already built (cached)"
          else
            echo "==> Building TagLib for Windows..."
            curl -sL https://taglib.org/releases/taglib-1.13.1.tar.gz | tar xz --no-same-owner
            cd taglib-1.13.1
            mkdir build && cd build
            cat > toolchain.cmake << TOOLCHAIN
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH /usr/x86_64-w64-mingw32)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
TOOLCHAIN
            cmake .. -DCMAKE_TOOLCHAIN_FILE=toolchain.cmake \
              -DCMAKE_INSTALL_PREFIX="$DEPS_PREFIX" \
              -DBUILD_SHARED_LIBS=OFF \
              -DCMAKE_BUILD_TYPE=Release 2>/dev/null
            make -j$(nproc) 2>/dev/null
            make install 2>/dev/null
            cd ../..
            rm -rf taglib-1.13.1
          fi

          # --- libsamplerate ---
          if [[ -f "$DEPS_PREFIX/lib/libsamplerate.a" ]]; then
            echo "==> libsamplerate already built (cached)"
          else
            echo "==> Building libsamplerate for Windows..."
            curl -sL https://github.com/libsndfile/libsamplerate/releases/download/0.2.2/libsamplerate-0.2.2.tar.xz | tar xJ --no-same-owner
            cd libsamplerate-0.2.2
            ./configure --host=x86_64-w64-mingw32 --prefix="$DEPS_PREFIX" \
              --enable-static --disable-shared \
              CC=x86_64-w64-mingw32-gcc 2>/dev/null
            make -j$(nproc) 2>/dev/null
            make install 2>/dev/null
            cd ..
            rm -rf libsamplerate-0.2.2
          fi

          # --- Chromaprint (simplified, optional) ---
          echo "==> Skipping chromaprint (requires FFmpeg, optional for basic Essentia)..."

          # --- FFmpeg (simplified static build) ---
          if [[ -f "$DEPS_PREFIX/lib/libavcodec.a" ]]; then
            echo "==> FFmpeg already built (cached)"
          else
            echo "==> Building minimal FFmpeg for Windows..."
            curl -sL https://ffmpeg.org/releases/ffmpeg-4.4.4.tar.xz | tar xJ --no-same-owner
            cd ffmpeg-4.4.4
            ./configure --arch=x86_64 --target-os=mingw32 \
              --cross-prefix=x86_64-w64-mingw32- \
              --prefix="$DEPS_PREFIX" \
              --enable-static --disable-shared \
              --disable-programs --disable-doc \
              --disable-avdevice --disable-postproc --disable-avfilter \
              --disable-network --disable-encoders --disable-muxers \
              --disable-bsfs --disable-devices --disable-filters \
              --enable-small 2>/dev/null
            make -j$(nproc) 2>/dev/null
            make install 2>/dev/null
            cd ..
            rm -rf ffmpeg-4.4.4
          fi

          # ---------------------------------------------------------------
          # Build Essentia
          # ---------------------------------------------------------------
          echo "==> Building Essentia library..."
          git clone --depth 1 https://github.com/MTG/essentia.git
          cd essentia

          # Restore cross-compilation environment
          export CC=x86_64-w64-mingw32-gcc
          export CXX=x86_64-w64-mingw32-g++
          export AR=x86_64-w64-mingw32-ar
          export RANLIB=x86_64-w64-mingw32-ranlib

          # MinGW cross-compilation flags:
          # - C++14: avoids std::byte conflict with Windows rpcndr.h byte typedef
          # - _USE_MATH_DEFINES: exposes M_PI, M_LN2, etc. (POSIX math constants)
          export CFLAGS="-D_USE_MATH_DEFINES"
          export CXXFLAGS="-std=c++14 -D_USE_MATH_DEFINES"

          if ! python3 waf configure \
            --prefix="$ESSENTIA_PREFIX" \
            --cross-compile-mingw32 \
            --pkg-config-path="$DEPS_PREFIX/lib/pkgconfig" \
            --fft=FFTW; then
            echo ""
            echo "ERROR: Essentia configure failed!"
            echo "Check the logs above for missing dependencies."
            exit 1
          fi

          if ! python3 waf build -j$(nproc); then
            echo ""
            echo "ERROR: Essentia build failed!"
            exit 1
          fi

          python3 waf install
          cd /project
          rm -rf /tmp/essentia-build
        fi

        # =====================================================================
        # Phase 4: Build mesh-player (no Essentia needed)
        # =====================================================================
        echo ""
        echo "==> Building mesh-player..."
        cargo build --release --target x86_64-pc-windows-gnu -p mesh-player

        # =====================================================================
        # Phase 5: Build mesh-cue (needs Essentia)
        # =====================================================================
        echo ""
        echo "==> Building mesh-cue..."
        export PKG_CONFIG_PATH="$PKG_CONFIG_WIN:$PKG_CONFIG_PATH"
        export PKG_CONFIG_ALLOW_CROSS=1
        export PKG_CONFIG_SYSROOT_DIR=/usr/x86_64-w64-mingw32
        export USE_TENSORFLOW=0

        # Add library paths for linking
        export LIBRARY_PATH="$ESSENTIA_PREFIX/lib:$LIBRARY_PATH"

        cargo build --release --target x86_64-pc-windows-gnu -p mesh-cue || {
          echo ""
          echo "WARNING: mesh-cue build failed (Essentia cross-compilation is complex)"
          echo "         mesh-player.exe was built successfully"
          echo ""
        }
      '

    echo ""
    echo "==> Copying outputs..."

    # Copy mesh-player
    PLAYER_EXE="$TARGET_DIR/x86_64-pc-windows-gnu/release/mesh-player.exe"
    if [[ -f "$PLAYER_EXE" ]]; then
      cp "$PLAYER_EXE" "$OUTPUT_DIR/"
      chmod 644 "$OUTPUT_DIR/mesh-player.exe"
      echo "  ✓ mesh-player.exe"
    else
      echo "  ✗ mesh-player.exe (build failed)"
    fi

    # Copy mesh-cue (if built)
    CUE_EXE="$TARGET_DIR/x86_64-pc-windows-gnu/release/mesh-cue.exe"
    if [[ -f "$CUE_EXE" ]]; then
      cp "$CUE_EXE" "$OUTPUT_DIR/"
      chmod 644 "$OUTPUT_DIR/mesh-cue.exe"
      echo "  ✓ mesh-cue.exe"
    else
      echo "  ✗ mesh-cue.exe (not built - Essentia cross-compilation pending)"
    fi

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║                        Build Complete!                                ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Output directory: dist/windows/                                        ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    ls -lh "$OUTPUT_DIR/"
  '';
}
