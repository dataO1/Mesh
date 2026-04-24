# Development shell for mesh
# Provides only what's needed for local cargo build/run/test
# Heavy deps (PyTorch, libtorch, model conversion) are handled by CI
{ pkgs, common, rustToolchain }:

# Commented out: model conversion is handled by CI (release.yml build-models job)
# Uncomment if you need to run `nix run .#convert-model` locally
# let
#   pythonEnv = pkgs.python311.withPackages (ps: with ps; [
#     torch
#     numpy
#     librosa
#     onnxruntime
#     onnx
#     soundfile
#     tqdm
#   ]);
# in

pkgs.mkShell {
  name = "mesh-dev-shell";

  packages = common.buildInputs ++ [
    # Custom essentia library (built from source)
    common.essentia
  ] ++ common.essentiaDeps ++ (with pkgs; [
    # Rust toolchain
    rustToolchain
    rust-analyzer
    cargo-watch
    cargo-release
    pkg-config

    # C/C++ build tools (bindgen, essentia, libpd)
    cmake
    clang
    llvmPackages.libclang
    gcc.cc
    gnumake
    autoconf
    automake
    libtool

    # GitHub CLI
    gh

    # Commented out: not needed for regular dev, adds minutes to shell build
    # Uncomment individually if needed:
    # cargo-edit       # `cargo add/rm` — handy but not essential
    # cargo-expand     # macro expansion debugging
    # gdb              # GNU debugger
    # lldb             # LLVM debugger
    # sqlite           # CozoDB database inspection
    # distrobox        # container-based .deb testing (CI handles this)
    # pythonEnv        # PyTorch/ONNX model conversion (CI handles this)
  ]);

  shellHook = ''
    # Rust environment
    export RUST_BACKTRACE=1

    # Override extreme release optimizations for faster dev builds
    # (lto=true + codegen-units=1 in Cargo.toml is great for distribution but painful for iteration)
    export CARGO_PROFILE_RELEASE_LTO=thin
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16

    # Logging: only show mesh-* crate logs at info level, filter out noisy dependencies
    export RUST_LOG="warn,wgpu_hal=error,mesh_core=debug,mesh_cue=debug,mesh_player=debug"

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

    # JACK settings
    export JACK_NO_AUDIO_RESERVATION=1

    # Setup patched crates (referenced by [patch.crates-io] in Cargo.toml)
    # Downloads directly from crates.io to avoid chicken-and-egg with cargo fetch
    mkdir -p patches

    # libpd-sys: 32-bit floats (required for PD external compatibility)
    if [ ! -d "patches/libpd-sys" ]; then
      echo "Setting up libpd-sys patch for 32-bit float compatibility..."
      curl -sL https://crates.io/api/v1/crates/libpd-sys/0.3.4/download | tar xz -C patches
      mv patches/libpd-sys-0.3.4 patches/libpd-sys
      sed -i 's/const PD_FLOATSIZE: &str = "64"/const PD_FLOATSIZE: \&str = "32"/' patches/libpd-sys/build.rs
      echo "  ✓ Patched libpd-sys for 32-bit floats"
    fi

    # libpd-rs: c_char portability (i8 on x86_64, u8 on aarch64)
    if [ ! -d "patches/libpd-rs" ]; then
      echo "Setting up libpd-rs patch for c_char portability..."
      curl -sL https://crates.io/api/v1/crates/libpd-rs/0.2.0/download | tar xz -C patches
      mv patches/libpd-rs-0.2.0 patches/libpd-rs
      sed -i 's/\*const i8/\*const os::raw::c_char/g' patches/libpd-rs/src/functions/receive.rs
      echo "  ✓ Patched libpd-rs for c_char portability"
    fi

    # Commented out: libtorch for nn~ Pure Data external (nix run .#build-nn-tilde)
    # nn~ is not built by CI — uncomment if you need to build it locally
    # export LIBTORCH="${pkgs.libtorch-bin}"
    # export LIBTORCH_LIB="${pkgs.libtorch-bin}/lib"
    # export LIBTORCH_INCLUDE="${pkgs.libtorch-bin}/include"
    # export PD_EXTERNALS="./effects/pd/externals"

    # Essentia library (built from source for mesh-cue)
    export PKG_CONFIG_PATH="${common.essentia}/lib/pkgconfig:$PKG_CONFIG_PATH"
    export LD_LIBRARY_PATH="${common.essentia}/lib:$LD_LIBRARY_PATH"
    # Disable TensorFlow in essentia-sys (not needed for BPM/key detection)
    export USE_TENSORFLOW=0
    # Fix Eigen include path for essentia-sys (it incorrectly appends /eigen3)
    export CPLUS_INCLUDE_PATH="${pkgs.eigen}/include/eigen3:$CPLUS_INCLUDE_PATH"

    # Vulkan for iced — AutoVsync tries Mailbox then falls back to Fifo
    # (Mailbox not supported on all GPU/driver combos, e.g. Nvidia on X11)
    # ICD discovery: NixOS sets up /run/opengl-driver/share/vulkan/icd.d/ via
    # hardware.graphics.enable, and the Vulkan loader searches it automatically.
    # No VK_ICD_FILENAMES override needed (vulkan-loader package has no ICDs).
    export WGPU_BACKEND=vulkan
    export ICED_PRESENT_MODE=auto_vsync

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
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
  '';
}
