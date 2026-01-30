# Container-based Windows cross-compilation
# Uses official Rust image + MinGW-w64 toolchain
#
# Usage: nix run .#build-windows
# Output: dist/windows/mesh-player_win.zip, mesh-cue_win.zip
#
# Options:
#   DEBUG_CONSOLE=1 nix run .#build-windows   # Enable console window for debugging
#
# Prerequisites:
#   - Podman or Docker installed and running
#   - NixOS: virtualisation.podman.enable = true (or docker)
#
# GPU Acceleration:
#   - DirectML support enabled via runtime DLL loading (load-dynamic feature)
#   - Pre-built ONNX Runtime DirectML DLLs downloaded from Microsoft NuGet
#   - Works with any DirectX 12 GPU (AMD, NVIDIA, Intel integrated/discrete)
#   - DLLs (onnxruntime.dll, DirectML.dll) bundled in mesh-cue_win.zip
#
# Why container? Pure Nix cross-compilation fails due to MinGW-w64 pthreads
# __ImageBase linker errors in nixpkgs. Container has working toolchain.
#
# ═══════════════════════════════════════════════════════════════════════════════
# ARCHITECTURE OVERVIEW
# ═══════════════════════════════════════════════════════════════════════════════
#
# This build system cross-compiles two Rust applications for Windows:
#   - mesh-player.exe: GUI DJ application (iced + wgpu)
#   - mesh-cue.exe: Audio analysis tool (requires Essentia C++ library)
#
# The complexity comes from mesh-cue needing Essentia, a large C++ audio
# analysis library with many dependencies. Cross-compiling C++ for Windows
# from Linux requires careful handling of:
#
#   1. HOST vs TARGET distinction (Cargo build scripts run on HOST)
#   2. Windows DLL export semantics (different from Linux .so)
#   3. FFmpeg API compatibility (Essentia uses deprecated FFmpeg 4.x APIs)
#
# ═══════════════════════════════════════════════════════════════════════════════
# KEY INSIGHT: WINDOWS DLL SYMBOL EXPORTS
# ═══════════════════════════════════════════════════════════════════════════════
#
# Linux .so files export all non-static symbols by default.
# Windows .dll files export NOTHING by default - each symbol must be marked
# with __declspec(dllexport) or listed in a .def file.
#
# Essentia (like many C++ libraries) was written for Linux and does not use
# __declspec(dllexport). The solution is the MinGW linker flag:
#
#   -Wl,--export-all-symbols
#
# This makes the linker behave like Linux, exporting all symbols. Without it,
# you get hundreds of "undefined reference" errors at link time because the
# Rust code cannot find Essentia functions in the DLL.
#
# ═══════════════════════════════════════════════════════════════════════════════
# BUILD PHASES
# ═══════════════════════════════════════════════════════════════════════════════
#
# Phase 1: Install MinGW-w64 toolchain in container
# Phase 2: Build Essentia dependencies for Windows (FFmpeg, FFTW3, TagLib, etc.)
# Phase 3: Build Essentia DLL for Windows with --export-all-symbols
# Phase 4: Build mesh-player.exe (straightforward Rust cross-compilation)
# Phase 5: Build native Linux Essentia for HOST builds (essentia-sys build.rs)
# Phase 6: Build mesh-cue.exe with both HOST and TARGET essentia available
#
# Caching: Dependencies and Essentia are cached in target/windows/ for fast
# subsequent builds. Delete target/windows/essentia-win/ to force rebuild.
#
# ═══════════════════════════════════════════════════════════════════════════════
{ pkgs, essentiaLinux }:

pkgs.writeShellApplication {
  name = "build-windows";
  runtimeInputs = with pkgs; [ podman coreutils zip ];
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

    # Linux essentia (from Nix) for host builds during cross-compilation
    ESSENTIA_LINUX="${essentiaLinux}"

    # Debug mode: show console window on Windows for stdout/stderr output
    DEBUG_CONSOLE="''${DEBUG_CONSOLE:-}"
    if [[ -n "$DEBUG_CONSOLE" ]]; then
      CONSOLE_FEATURE=",console"
      echo "Debug mode:      ENABLED (console window visible)"
    else
      CONSOLE_FEATURE=""
      echo "Debug mode:      disabled (use DEBUG_CONSOLE=1 to enable)"
    fi

    echo "Project root:    $PROJECT_ROOT"
    echo "Output dir:      $OUTPUT_DIR"
    echo "Container:       $IMAGE"
    echo "Linux essentia:  $ESSENTIA_LINUX"
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
    # Note: We build BOTH Windows and Linux essentia inside the container to avoid
    # glibc/gcc compatibility issues with Nix-built libraries
    podman run --rm \
      -v "$PROJECT_ROOT:/project:ro" \
      -v "$TARGET_DIR:/project/target:rw" \
      -e "CONSOLE_FEATURE=$CONSOLE_FEATURE" \
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
          unzip \
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

        if [[ -f "$ESSENTIA_PREFIX/lib/essentia.dll" ]] || [[ -f "$ESSENTIA_PREFIX/bin/essentia.dll" ]]; then
          echo "==> Essentia already built (cached), skipping..."
          # Ensure libessentia symlinks exist even for cached builds
          if [[ -f "$ESSENTIA_PREFIX/lib/essentia.dll.a" ]] && [[ ! -f "$ESSENTIA_PREFIX/lib/libessentia.a" ]]; then
            ln -sf essentia.dll.a "$ESSENTIA_PREFIX/lib/libessentia.dll.a"
            ln -sf essentia.dll.a "$ESSENTIA_PREFIX/lib/libessentia.a"
            echo "==> Created libessentia symlinks for MinGW linker (cached build)"
          fi
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

          # --- Chromaprint (audio fingerprinting, uses FFTW3) ---
          if [[ -f "$DEPS_PREFIX/lib/libchromaprint.a" ]]; then
            echo "==> Chromaprint already built (cached)"
          else
            echo "==> Building Chromaprint for Windows..."
            curl -sL https://github.com/acoustid/chromaprint/releases/download/v1.5.1/chromaprint-1.5.1.tar.gz | tar xz --no-same-owner
            cd chromaprint-1.5.1
            mkdir build && cd build
            cat > toolchain.cmake << TOOLCHAIN
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH /usr/x86_64-w64-mingw32 $DEPS_PREFIX)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_PREFIX_PATH $DEPS_PREFIX)
TOOLCHAIN
            cmake .. -DCMAKE_TOOLCHAIN_FILE=toolchain.cmake \
              -DCMAKE_INSTALL_PREFIX="$DEPS_PREFIX" \
              -DBUILD_SHARED_LIBS=OFF \
              -DBUILD_TOOLS=OFF \
              -DBUILD_TESTS=OFF \
              -DFFT_LIB=fftw3f \
              -DFFTW3_DIR="$DEPS_PREFIX" \
              -DCMAKE_BUILD_TYPE=Release 2>/dev/null
            make -j$(nproc) 2>/dev/null
            make install 2>/dev/null

            cd ../..
            rm -rf chromaprint-1.5.1
          fi

          # ---------------------------------------------------------------
          # Build Essentia DLL for Windows
          # ---------------------------------------------------------------
          # CRITICAL: See header comment about --export-all-symbols flag.
          # Without it, the DLL exports nothing and Rust linking fails.
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
          # - _USE_MATH_DEFINES: exposes M_PI, M_LN2, etc.
          # - TAGLIB_STATIC: tells TagLib headers we link statically
          # - CHROMAPRINT_NODLL: tells Chromaprint headers we link statically
          export CFLAGS="-D_USE_MATH_DEFINES"
          export CXXFLAGS="-std=c++14 -D_USE_MATH_DEFINES -DTAGLIB_STATIC -DCHROMAPRINT_NODLL"
          # Force link fftw3f for chromaprint static dependency
          # *** CRITICAL FIX: --export-all-symbols ***
          # This flag makes the Windows DLL export all symbols like Linux .so files do.
          # Without it, essentia.dll exports nothing (Windows requires explicit exports),
          # causing 100+ "undefined reference" errors when linking mesh-cue.exe.
          #
          # -static-libgcc -static-libstdc++: Embed MinGW runtime INTO essentia.dll
          # Without this, users would need libstdc++-6.dll and libgcc_s_seh-1.dll
          # alongside mesh-cue.exe. With static linking, only essentia.dll is needed.
          export LDFLAGS="-L$DEPS_PREFIX/lib -lfftw3f -Wl,--export-all-symbols -static-libgcc -static-libstdc++"

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

          # Fix library naming for MinGW linker: -lessentia looks for libessentia.a or libessentia.dll.a
          # waf creates essentia.dll.a, so we symlink it
          if [[ -f "$ESSENTIA_PREFIX/lib/essentia.dll.a" ]]; then
            ln -sf essentia.dll.a "$ESSENTIA_PREFIX/lib/libessentia.dll.a"
            ln -sf essentia.dll.a "$ESSENTIA_PREFIX/lib/libessentia.a"
            echo "==> Created libessentia symlinks for MinGW linker"
          fi

          cd /project
          rm -rf /tmp/essentia-build
        fi

        # =====================================================================
        # Phase 4: Build Rust crates
        # =====================================================================
        # CRITICAL: Unset global CC/CXX that were set for Essentia build.
        # Cargo uses target-specific vars (CC_x86_64_pc_windows_gnu) for Windows
        # but needs native gcc for host builds (build scripts, proc-macros).
        unset CC CXX AR RANLIB CFLAGS CXXFLAGS LDFLAGS

        echo ""
        echo "==> Building mesh-player..."
        echo "    CONSOLE_FEATURE='$CONSOLE_FEATURE'"
        # Use --no-default-features to disable JACK backend (Linux-only, uses CPAL on Windows)
        # CONSOLE_FEATURE adds ",console" when DEBUG_CONSOLE=1 to show stdout/stderr
        # Force rebuild when features change by cleaning the package first
        if [[ -n "$CONSOLE_FEATURE" ]]; then
          echo "    Console mode: ENABLED - cleaning cached binary to force rebuild..."
          cargo clean -p mesh-player --release --target x86_64-pc-windows-gnu 2>/dev/null || true
          cargo build --release --target x86_64-pc-windows-gnu -p mesh-player --no-default-features --features console
        else
          cargo build --release --target x86_64-pc-windows-gnu -p mesh-player --no-default-features
        fi

        # =====================================================================
        # Phase 5: Build mesh-cue (needs Essentia for both HOST and TARGET)
        # =====================================================================
        # KEY INSIGHT: Cargo cross-compilation has two environments:
        #   - HOST: Where build scripts (build.rs) and proc-macros run (Linux)
        #   - TARGET: Where the final binary runs (Windows)
        #
        # essentia-sys has a build.rs that compiles C++ bridge code. This code
        # runs on HOST (Linux) but must also link against TARGET (Windows) libs.
        # We need TWO essentia installations:
        #   - $ESSENTIA_HOST: Native Linux build for HOST compilation
        #   - $ESSENTIA_PREFIX: Cross-compiled Windows build for TARGET linking
        # =====================================================================
        echo ""
        echo "==> Building mesh-cue..."

        # ---------------------------------------------------------------
        # Build Essentia for Linux HOST (native, not cross-compiled)
        # Cannot use Nix essentia due to glibc version mismatch with container
        # ---------------------------------------------------------------
        # Install native Linux dev packages needed for essentia-sys bridge compilation
        # These must be installed even if essentia-host is cached (for eigen3.pc, etc.)
        echo "==> Installing native Linux dev packages for essentia-sys..."
        apt-get install -y -qq \
          libfftw3-dev \
          libtag1-dev \
          libsamplerate0-dev \
          libchromaprint-dev \
          libyaml-dev \
          libeigen3-dev \
          python3 \
          >/dev/null 2>&1

        ESSENTIA_HOST=/project/target/essentia-host
        if [[ -f "$ESSENTIA_HOST/lib/libessentia.so" ]]; then
          echo "==> Host Essentia already built (cached)"
        else
          echo "==> Building Essentia for Linux (host builds)..."
          echo "    (This builds a native Linux essentia compatible with container glibc)"

          mkdir -p /tmp/essentia-host-build
          cd /tmp/essentia-host-build

          # Build FFmpeg 4.4.4 for host (container FFmpeg is too new, missing deprecated APIs)
          # Essentia uses av_register_all(), AVStream->codec, avcodec_encode_audio2() etc.
          if [[ -f "$ESSENTIA_HOST/lib/libavcodec.so" ]]; then
            echo "==> Host FFmpeg already built (cached)"
          else
            echo "==> Building FFmpeg 4.4.4 for Linux (host, for essentia API compatibility)..."
            curl -sL https://ffmpeg.org/releases/ffmpeg-4.4.4.tar.xz | tar xJ --no-same-owner
            cd ffmpeg-4.4.4
            ./configure --prefix="$ESSENTIA_HOST" \
              --enable-shared --disable-static \
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

          # Export PKG_CONFIG_PATH so essentia finds our FFmpeg, not system one
          export PKG_CONFIG_PATH="$ESSENTIA_HOST/lib/pkgconfig:$PKG_CONFIG_PATH"
          export LD_LIBRARY_PATH="$ESSENTIA_HOST/lib:$LD_LIBRARY_PATH"
          export LIBRARY_PATH="$ESSENTIA_HOST/lib:$LIBRARY_PATH"
          export CPLUS_INCLUDE_PATH="$ESSENTIA_HOST/include:$CPLUS_INCLUDE_PATH"

          # Clone essentia (reuse if possible from Windows build cache)
          if [[ -d /project/target/essentia-src ]]; then
            cp -r /project/target/essentia-src essentia
          else
            git clone --depth 1 https://github.com/MTG/essentia.git
            mkdir -p /project/target/essentia-src
            cp -r essentia/* /project/target/essentia-src/
          fi
          cd essentia

          # Build with native compiler (container gcc/glibc)
          # Use C++14 for essentia itself (same as Windows build)
          export CFLAGS="-D_USE_MATH_DEFINES"
          export CXXFLAGS="-std=c++14 -D_USE_MATH_DEFINES"

          if ! python3 waf configure \
            --prefix="$ESSENTIA_HOST" \
            --fft=FFTW; then
            echo "ERROR: Host Essentia configure failed"
            exit 1
          fi

          if ! python3 waf build -j$(nproc); then
            echo "ERROR: Host Essentia build failed"
            exit 1
          fi

          python3 waf install

          # Create pkg-config file
          mkdir -p "$ESSENTIA_HOST/lib/pkgconfig"
          cat > "$ESSENTIA_HOST/lib/pkgconfig/essentia.pc" << ESSPC
prefix=$ESSENTIA_HOST
exec_prefix=\''${prefix}
libdir=\''${exec_prefix}/lib
includedir=\''${prefix}/include/essentia

Name: essentia
Description: Audio analysis library
Version: 2.1_beta6
Libs: -L\''${libdir} -lessentia -lfftw3f -ltag -lsamplerate -lchromaprint -lavformat -lavcodec -lavutil -lswresample -lyaml
Cflags: -I\''${includedir}
ESSPC

          cd /project
          rm -rf /tmp/essentia-host-build
          unset CFLAGS CXXFLAGS
        fi

        echo "==> Setting up cross-compilation environment..."
        echo "    Host essentia:   $ESSENTIA_HOST"
        echo "    Target essentia: $ESSENTIA_PREFIX"

        # Set PKG_CONFIG_PATH for host builds (Linux essentia + system libraries)
        # essentia-sys uses pkg-config to find essentia, eigen3, fftw3, etc.
        SYS_PKG_CONFIG="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
        export PKG_CONFIG_PATH="$ESSENTIA_HOST/lib/pkgconfig:$SYS_PKG_CONFIG"
        # Also set HOST_PKG_CONFIG_PATH for cross-compilation scenarios
        export HOST_PKG_CONFIG_PATH="$PKG_CONFIG_PATH"

        # Create pkg-config wrapper for target builds (Windows)
        # When TARGET contains "windows", use Windows essentia
        mkdir -p /tmp/pkg-config-wrapper
        cat > /tmp/pkg-config-wrapper/pkg-config << PKGWRAPPER
#!/bin/bash
if [[ "\$TARGET" == *"dows"* ]]; then
  export PKG_CONFIG_PATH="/project/target/essentia-win/lib/pkgconfig:$SYS_PKG_CONFIG"
fi
exec /usr/bin/pkg-config "\$@"
PKGWRAPPER
        chmod +x /tmp/pkg-config-wrapper/pkg-config
        export PATH="/tmp/pkg-config-wrapper:$PATH"

        # Set paths for host (Linux) builds
        # Include Eigen3 path explicitly (system eigen3.pc may have wrong path)
        export CPLUS_INCLUDE_PATH="$ESSENTIA_HOST/include:/usr/include/eigen3:$CPLUS_INCLUDE_PATH"
        export LIBRARY_PATH="$ESSENTIA_HOST/lib:$LIBRARY_PATH"
        export LD_LIBRARY_PATH="$ESSENTIA_HOST/lib:$LD_LIBRARY_PATH"

        # Configure for cross-compilation
        export PKG_CONFIG_ALLOW_CROSS=1
        export USE_TENSORFLOW=0

        # Set C++ flags for Windows TARGET builds only (not host builds):
        # - C++17: required for std::variant in essentia-sys bridge code
        # - _BYTE_DEFINED: prevents MinGW rpcndr.h byte typedef conflicting with std::byte
        export CXXFLAGS_x86_64_pc_windows_gnu="-std=c++17 -D_USE_MATH_DEFINES -DTAGLIB_STATIC -DCHROMAPRINT_NODLL -D_BYTE_DEFINED"

        # Use --no-default-features to disable JACK backend (Linux-only, uses CPAL on Windows)
        # WORKAROUND: Build essentia first without load-dynamic feature
        # load-dynamic causes essentia-codegen to fail (cargo feature/build order issue)
        echo "    Building essentia first (without load-dynamic)..."
        cargo build --release --target x86_64-pc-windows-gnu -p essentia -p essentia-sys --no-default-features 2>/dev/null || true

        # Now build mesh-cue with load-dynamic and directml (essentia is already cached)
        # load-dynamic: enables runtime DLL loading for ONNX Runtime (avoids MinGW/MSVC ABI issues)
        # directml: enables DirectML execution provider for GPU acceleration on Windows
        # CONSOLE_FEATURE adds ",console" when DEBUG_CONSOLE=1 to show stdout/stderr
        if [[ -n "$CONSOLE_FEATURE" ]]; then
          MESH_CUE_FEATURES="load-dynamic,directml,console"
          echo "    Console mode: ENABLED - cleaning cached binary to force rebuild..."
          cargo clean -p mesh-cue --release --target x86_64-pc-windows-gnu 2>/dev/null || true
        else
          MESH_CUE_FEATURES="load-dynamic,directml"
        fi
        echo "    Features: $MESH_CUE_FEATURES"
        cargo build --release --target x86_64-pc-windows-gnu -p mesh-cue --no-default-features --features "$MESH_CUE_FEATURES" || {
          echo ""
          echo "WARNING: mesh-cue build failed (Essentia cross-compilation is complex)"
          echo "         mesh-player.exe was built successfully"
          echo ""
        }

        # =====================================================================
        # Phase 6: Download ONNX Runtime DirectML (pre-built from Microsoft)
        # =====================================================================
        # Since we use load-dynamic feature, we need to bundle the DLLs.
        # Download from Microsoft NuGet: Microsoft.ML.OnnxRuntime.DirectML
        # This provides GPU acceleration for any DirectX 12 GPU (AMD/NVIDIA/Intel)
        echo ""
        echo "==> Downloading ONNX Runtime DirectML..."
        # Note: DirectML package versioning lags behind main ORT releases
        # Check available versions: curl -s https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/index.json
        ORT_VERSION="1.23.0"
        ORT_CACHE="/project/target/onnxruntime-directml-$ORT_VERSION"
        mkdir -p "$ORT_CACHE"

        if [[ -f "$ORT_CACHE/onnxruntime.dll" ]]; then
          echo "  ONNX Runtime already downloaded (cached)"
        else
          echo "  Downloading Microsoft.ML.OnnxRuntime.DirectML $ORT_VERSION..."
          cd /tmp
          # NuGet packages are just ZIP files with .nupkg extension
          # Use NuGet v3 flat container API (most reliable, lowercase package ID required)
          ORT_PKG_ID="microsoft.ml.onnxruntime.directml"
          ORT_URL="https://api.nuget.org/v3-flatcontainer/$ORT_PKG_ID/$ORT_VERSION/$ORT_PKG_ID.$ORT_VERSION.nupkg"
          echo "  URL: $ORT_URL"
          curl -fSL "$ORT_URL" -o ort.nupkg
          unzip -q ort.nupkg -d ort-package
          # DLLs are in runtimes/win-x64/native/
          cp ort-package/runtimes/win-x64/native/onnxruntime.dll "$ORT_CACHE/"
          cp ort-package/runtimes/win-x64/native/DirectML.dll "$ORT_CACHE/" 2>/dev/null || true
          rm -rf ort.nupkg ort-package
          cd /project
          echo "  Downloaded ONNX Runtime DirectML $ORT_VERSION"
        fi

        # =====================================================================
        # Phase 7: Gather runtime DLLs for distribution
        # =====================================================================
        # MinGW applications need these DLLs at runtime. Rather than fighting
        # with static linking (especially pthread), we ship them alongside .exe
        # Reference: https://discussion.fedoraproject.org/t/location-of-mingw64-mingw32-system-libraries
        echo ""
        echo "==> Gathering runtime DLLs for distribution..."
        mkdir -p /project/target/dlls

        # Find MinGW runtime DLLs using the compiler
        for dll in libstdc++-6.dll libgcc_s_seh-1.dll libwinpthread-1.dll; do
          dll_path=$(x86_64-w64-mingw32-gcc --print-file-name "$dll")
          if [[ -f "$dll_path" ]]; then
            cp "$dll_path" /project/target/dlls/
            echo "  Found: $dll"
          else
            echo "  WARNING: $dll not found"
          fi
        done

        # Copy essentia.dll (needed by mesh-cue.exe)
        if [[ -f "$ESSENTIA_PREFIX/lib/essentia.dll" ]]; then
          cp "$ESSENTIA_PREFIX/lib/essentia.dll" /project/target/dlls/
          echo "  Found: essentia.dll"
        elif [[ -f "$ESSENTIA_PREFIX/bin/essentia.dll" ]]; then
          cp "$ESSENTIA_PREFIX/bin/essentia.dll" /project/target/dlls/
          echo "  Found: essentia.dll"
        fi

        # Copy ONNX Runtime DirectML DLLs (needed for GPU-accelerated stem separation)
        if [[ -f "$ORT_CACHE/onnxruntime.dll" ]]; then
          cp "$ORT_CACHE/onnxruntime.dll" /project/target/dlls/
          echo "  Found: onnxruntime.dll (DirectML)"
        fi
        if [[ -f "$ORT_CACHE/DirectML.dll" ]]; then
          cp "$ORT_CACHE/DirectML.dll" /project/target/dlls/
          echo "  Found: DirectML.dll"
        fi

        echo ""
        echo "==> DLLs gathered in /project/target/dlls/"
        ls -lh /project/target/dlls/
      '

    echo ""
    echo "==> Creating distribution packages..."

    # Create separate folders for each application
    PLAYER_DIR="$OUTPUT_DIR/mesh-player"
    CUE_DIR="$OUTPUT_DIR/mesh-cue"
    DLL_DIR="$TARGET_DIR/dlls"

    rm -rf "$PLAYER_DIR" "$CUE_DIR"
    mkdir -p "$PLAYER_DIR" "$CUE_DIR"

    # Runtime DLLs needed by both applications (MinGW C++ runtime)
    RUNTIME_DLLS="libstdc++-6.dll libgcc_s_seh-1.dll libwinpthread-1.dll"

    # --- mesh-player package ---
    PLAYER_EXE="$TARGET_DIR/x86_64-pc-windows-gnu/release/mesh-player.exe"
    if [[ -f "$PLAYER_EXE" ]]; then
      echo "==> Packaging mesh-player..."
      cp "$PLAYER_EXE" "$PLAYER_DIR/"

      # Copy runtime DLLs
      for dll in $RUNTIME_DLLS; do
        if [[ -f "$DLL_DIR/$dll" ]]; then
          cp "$DLL_DIR/$dll" "$PLAYER_DIR/"
          echo "  ✓ $dll"
        fi
      done
      echo "  ✓ mesh-player.exe"

      # Create zip (remove old zip first to avoid update errors)
      rm -f "$OUTPUT_DIR/mesh-player_win.zip"
      (cd "$OUTPUT_DIR" && zip -r mesh-player_win.zip mesh-player/)
      echo "  ✓ Created dist/windows/mesh-player_win.zip"
    else
      echo "  ✗ mesh-player.exe (build failed)"
    fi

    # --- mesh-cue package ---
    CUE_EXE="$TARGET_DIR/x86_64-pc-windows-gnu/release/mesh-cue.exe"
    if [[ -f "$CUE_EXE" ]]; then
      echo ""
      echo "==> Packaging mesh-cue..."
      cp "$CUE_EXE" "$CUE_DIR/"

      # Copy runtime DLLs
      for dll in $RUNTIME_DLLS; do
        if [[ -f "$DLL_DIR/$dll" ]]; then
          cp "$DLL_DIR/$dll" "$CUE_DIR/"
          echo "  ✓ $dll"
        fi
      done

      # Copy essentia.dll (only needed by mesh-cue)
      if [[ -f "$DLL_DIR/essentia.dll" ]]; then
        cp "$DLL_DIR/essentia.dll" "$CUE_DIR/"
        echo "  ✓ essentia.dll"
      fi

      # Copy ONNX Runtime DirectML DLLs (GPU-accelerated stem separation)
      if [[ -f "$DLL_DIR/onnxruntime.dll" ]]; then
        cp "$DLL_DIR/onnxruntime.dll" "$CUE_DIR/"
        echo "  ✓ onnxruntime.dll (DirectML)"
      fi
      if [[ -f "$DLL_DIR/DirectML.dll" ]]; then
        cp "$DLL_DIR/DirectML.dll" "$CUE_DIR/"
        echo "  ✓ DirectML.dll"
      fi
      echo "  ✓ mesh-cue.exe"

      # Create zip (remove old zip first to avoid update errors)
      rm -f "$OUTPUT_DIR/mesh-cue_win.zip"
      (cd "$OUTPUT_DIR" && zip -r mesh-cue_win.zip mesh-cue/)
      echo "  ✓ Created dist/windows/mesh-cue_win.zip"
    else
      echo ""
      echo "  ✗ mesh-cue.exe (not built)"
    fi

    # Clean up temporary folders - only keep the zips
    rm -rf "$PLAYER_DIR" "$CUE_DIR"

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║                        Build Complete!                                ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Distribution packages ready in dist/windows/                         ║"
    echo "║                                                                       ║"
    echo "║  mesh-player_win.zip:                                                     ║"
    echo "║    - mesh-player.exe + MinGW runtime DLLs                             ║"
    echo "║                                                                       ║"
    echo "║  mesh-cue_win.zip:                                                        ║"
    echo "║    - mesh-cue.exe + essentia.dll + onnxruntime.dll + DirectML.dll    ║"
    echo "║    - GPU acceleration via DirectML (AMD/NVIDIA/Intel DirectX 12)     ║"
    echo "║                                                                       ║"
    echo "║  Just extract and run on Windows 10+ - no installation needed!        ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "Distribution packages:"
    ls -lh "$OUTPUT_DIR/"*.zip 2>/dev/null || echo "  (no zip files created)"
  '';
}
