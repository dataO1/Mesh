# Development shell for mesh
# Provides all tools needed for local development
{ pkgs, common, rustToolchain }:

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

    # Enable native JACK backend by default for Linux development
    # This provides port-level routing control (e.g., Scarlett 18i20 outputs 1-2 vs 3-4)
    # To use CPAL instead: cargo run -p mesh-player --no-default-features
    export CARGO_BUILD_FEATURES="jack-backend"

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

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║                      Mesh Development Shell                           ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Audio: Native JACK backend enabled (jack-backend feature)            ║"
    echo "║         For CPAL instead: cargo run -p mesh-player --no-default-features"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Development:                                                         ║"
    echo "║    cargo run -p mesh-player          # DJ application (JACK)          ║"
    echo "║    cargo run -p mesh-cue             # Track preparation              ║"
    echo "║    cargo test                        # Run all tests                  ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Build portable packages:                                             ║"
    echo "║    nix build .#mesh-deb             # Build .debs → ./result/         ║"
    echo "║    nix run .#build-windows          # Build .exe  → ./dist/windows/   ║"
    echo "╠═══════════════════════════════════════════════════════════════════════╣"
    echo "║  Test .deb packages (requires: virtualisation.podman.enable = true)   ║"
    echo "║    distrobox assemble create        # Create container + auto-install ║"
    echo "║    distrobox enter mesh-ubuntu      # Enter and test (mesh-player)    ║"
    echo "║    distrobox assemble rm            # Clean up when done              ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
  '';
}
