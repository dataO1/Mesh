# Development shell for mesh
# Provides all tools needed for local development
{ pkgs, common, rustToolchain }:

let
  # Python environment for ONNX model conversion (demucs PyTorch → ONNX)
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    torch
    numpy
    librosa
    onnxruntime
    onnx
    soundfile
    tqdm
  ]);
in

pkgs.mkShell {
  name = "mesh-dev-shell";

  # mkShell properly adds all packages to PATH (unlike stdenv.mkDerivation)
  packages = common.buildInputs ++ [
    # Custom essentia library (built from source)
    common.essentia
  ] ++ common.essentiaDeps ++ (with pkgs; [
    # Development tools
    rustToolchain
    rust-analyzer
    cargo-watch
    cargo-edit
    cargo-expand
    cargo-release
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

    # GitHub CLI (embedded setup automation, release management)
    gh

    # Python for ONNX model conversion
    pythonEnv
  ]);

  shellHook = ''
    # Rust environment
    export RUST_BACKTRACE=1

    # Logging: only show mesh-* crate logs at info level, filter out noisy dependencies
    export RUST_LOG="warn,mesh_core=debug,mesh_cue=debug,mesh_player=debug"

    # Library paths
    export LD_LIBRARY_PATH="${common.libraryPath}:$LD_LIBRARY_PATH"

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

    # Setup patched crates (referenced by [patch.crates-io] in Cargo.toml)
    mkdir -p patches

    # libpd-sys: 32-bit floats (required for nn~ external compatibility)
    if [ ! -d "patches/libpd-sys" ]; then
      echo "Setting up libpd-sys patch for 32-bit float compatibility..."
      LIBPD_SYS_SRC=$(find ~/.cargo/registry/src -name "libpd-sys-*" -type d 2>/dev/null | head -1)
      if [ -n "$LIBPD_SYS_SRC" ]; then
        cp -r "$LIBPD_SYS_SRC" patches/libpd-sys
        sed -i 's/const PD_FLOATSIZE: &str = "64"/const PD_FLOATSIZE: \&str = "32"/' patches/libpd-sys/build.rs
        echo "  ✓ Patched libpd-sys for 32-bit floats"
      else
        echo "  ⚠ libpd-sys not found in cargo registry. Run 'cargo fetch' first."
      fi
    fi

    # libpd-rs: c_char portability (i8 on x86_64, u8 on aarch64)
    if [ ! -d "patches/libpd-rs" ]; then
      echo "Setting up libpd-rs patch for c_char portability..."
      LIBPD_RS_SRC=$(find ~/.cargo/registry/src -name "libpd-rs-*" -type d 2>/dev/null | head -1)
      if [ -n "$LIBPD_RS_SRC" ]; then
        cp -r "$LIBPD_RS_SRC" patches/libpd-rs
        sed -i 's/\*const i8/\*const os::raw::c_char/g' patches/libpd-rs/src/functions/receive.rs
        echo "  ✓ Patched libpd-rs for c_char portability"
      else
        echo "  ⚠ libpd-rs not found in cargo registry. Run 'cargo fetch' first."
      fi
    fi

    # Torch library path (for nn~)
    export LIBTORCH="${pkgs.libtorch-bin}"
    export LIBTORCH_LIB="${pkgs.libtorch-bin}/lib"
    export LIBTORCH_INCLUDE="${pkgs.libtorch-bin}/include"

    # Essentia library (built from source for mesh-cue)
    export PKG_CONFIG_PATH="${common.essentia}/lib/pkgconfig:$PKG_CONFIG_PATH"
    export LD_LIBRARY_PATH="${common.essentia}/lib:$LD_LIBRARY_PATH"
    # Disable TensorFlow in essentia-sys (not needed for BPM/key detection)
    export USE_TENSORFLOW=0
    # Fix Eigen include path for essentia-sys (it incorrectly appends /eigen3)
    export CPLUS_INCLUDE_PATH="${pkgs.eigen}/include/eigen3:$CPLUS_INCLUDE_PATH"

    # Vulkan for iced
    export VK_ICD_FILENAMES="${pkgs.vulkan-loader}/share/vulkan/icd.d/intel_icd.x86_64.json:${pkgs.vulkan-loader}/share/vulkan/icd.d/radeon_icd.x86_64.json"

    MESH_VERSION=$(grep -A2 '^\[workspace\.package\]' Cargo.toml | grep '^version' | sed 's/.*"\(.*\)".*/\1/')
    echo ""
    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║                  Mesh Development Shell  v$MESH_VERSION                      ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Run:                                                                ║"
    echo "║    cargo run -p mesh-player          # Performance mode (JACK)       ║"
    echo "║    cargo run -p mesh-cue             # Editor mode (CPU stems)       ║"
    echo "║    cargo run -p mesh-cue --features cuda     # NVIDIA CUDA stems     ║"
    echo "║    cargo test                        # Run all tests                 ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Release (cargo release):                                            ║"
    echo "║    cargo release patch               # x.y.Z+1  bug fix             ║"
    echo "║    cargo release minor               # x.Y+1.0  new feature         ║"
    echo "║    cargo release major               # X+1.0.0  breaking change     ║"
    echo "║    cargo release alpha               # x.y.z-alpha.1  pre-release   ║"
    echo "║    cargo release rc                  # x.y.z-rc.1  release candidate║"
    echo "║                                                                      ║"
    echo "║  This bumps Cargo.toml, commits, tags, and pushes.                   ║"
    echo "║  CI then builds .deb + .zip + models and publishes the release.      ║"
    echo "║  Nix packages read version from Cargo.toml automatically.            ║"
    echo "║                                                                      ║"
    echo "║  Dry run first:  cargo release patch --dry-run                       ║"
    echo "║  All nix run apps:  see flake.nix apps section for full reference   ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Test packages in container:                                         ║"
    echo "║    distrobox assemble create        # Create test container          ║"
    echo "║    distrobox enter mesh-ubuntu      # Enter and test (mesh-player)   ║"
    echo "║    distrobox assemble rm            # Clean up when done             ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  PD Effects (Pure Data neural audio):                                ║"
    echo "║    nix run .#build-nn-tilde         # Build nn~ external for RAVE    ║"
    echo "║                                                                      ║"
    echo "║  Effect structure (mesh-collection/effects/):                        ║"
    echo "║    my-effect/                                                        ║"
    echo "║      metadata.json    # Name, category, params, latency             ║"
    echo "║      my-effect.pd     # PD patch (must match folder name)           ║"
    echo "║    externals/         # Shared externals (nn~.pd_linux, etc.)       ║"
    echo "║    models/            # Shared RAVE models (.ts files)              ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  BPM Accuracy Report:                                                ║"
    echo "║    nix run .#bpm-report               # Export + scrape + report    ║"
    echo "║    nix run .#bpm-report -- --limit 20 # Scrape only 20 new tracks  ║"
    echo "║    nix run .#bpm-report -- --no-scrape  # Report from cached GT    ║"
    echo "║  LUFS Comparison Report:                                             ║"
    echo "║    cargo run -p mesh-core --bin lufs-report  # Drop vs integrated  ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Embedded (Orange Pi 5):                                              ║"
    echo "║    nix run .#embedded-flash            # Download + flash SD image  ║"
    echo "║    nix run .#embedded-flash /dev/sdX   # Flash to specific device   ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
  '';
}
