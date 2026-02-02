# nn~ Pure Data external for neural audio processing
#
# This derivation builds the nn~ external for use with libpd-rs.
#
# The resulting package includes:
#   - nn~.pd_linux (the external)
#   - lib/ (bundled libtorch libraries)
#
# Usage:
#   nix build .#nn-tilde
#   # Output: result/lib/pd/extra/nn~.pd_linux + result/lib/pd/extra/lib/*.so
#
# The nn~ external is configured with RPATH=$ORIGIN/lib so it can find
# the bundled libtorch libraries at runtime.

{ lib
, stdenv
, llvmPackages  # Use LLVM for better C++ compatibility with libtorch
, fetchFromGitHub
, cmake
, puredata
, libtorch-bin
, curl
, patchelf
}:

let
  # Get the libtorch share directory for CMake
  libtorchCmake = "${libtorch-bin.dev}/share/cmake";
in
# Use LLVM stdenv for better libtorch compatibility
# GCC 15 has stricter template parsing that breaks libtorch headers
llvmPackages.stdenv.mkDerivation {
  pname = "nn-tilde";
  version = "unstable-2024-07-08";

  src = fetchFromGitHub {
    owner = "acids-ircam";
    repo = "nn_tilde";
    rev = "ca97f8259442649fe7b5acacd8d9cdd8757098dd";  # From flake.lock
    hash = "sha256-1WbXH7KuW7MGfKppE1lMbuY3Xlf/Jae+k6SQ4dtScPo=";
    fetchSubmodules = true;
  };

  nativeBuildInputs = [
    cmake
    patchelf
  ];

  buildInputs = [
    puredata
    libtorch-bin
    curl
  ];

  # Create symlink structure that nn_tilde's cmake expects
  # This prevents it from downloading its own libtorch
  preConfigure = ''
    echo "Setting up libtorch symlinks..."

    # nn_tilde's add_torch.cmake looks for libtorch at ../torch/libtorch
    # relative to the build directory
    mkdir -p torch/libtorch
    ln -sf ${libtorch-bin}/lib torch/libtorch/lib
    ln -sf ${libtorch-bin.dev}/include torch/libtorch/include
    ln -sf ${libtorch-bin.dev}/share torch/libtorch/share

    # Also set up curl for any network operations
    mkdir -p env/lib env/include
    ln -sf ${curl.out}/lib/libcurl.so* env/lib/
    ln -sf ${curl.dev}/include/curl env/include/

    echo "Libtorch linked from: ${libtorch-bin}"
  '';

  cmakeDir = "../src";

  # Use permissive mode for GCC 15 compatibility with libtorch templates
  # PDINSTANCE=1 is required because libpd-rs uses MULTI=true mode
  NIX_CFLAGS_COMPILE = "-fpermissive -DPDINSTANCE=1 -DPDTHREADS=1";

  cmakeFlags = [
    "-DCMAKE_BUILD_TYPE=Release"
    # Point CMake to our libtorch
    "-DCMAKE_PREFIX_PATH=${libtorchCmake}"
    "-DTorch_DIR=${libtorchCmake}/Torch"
    # Tell nn_tilde where to find Pure Data headers (m_pd.h)
    # Without this, it tries to download from GitHub which fails in sandbox
    "-DPUREDATA_INCLUDE_DIR=${puredata}/include"
  ];

  # Patch source to handle Pure Data / libtorch macro conflict
  # m_pd.h defines macros like s_, x_, etc. that conflict with libtorch's
  # CREATE_ACCESSOR macro which generates methods like s_(), x_(), etc.
  postPatch = ''
    # The issue: m_pd.h line 1062: #define s_ (pd_this->pd_s_)
    # This breaks libtorch's CREATE_ACCESSOR(String, s) which generates s_()
    #
    # Solution: Ensure torch headers are included BEFORE m_pd.h,
    # or undefine the conflicting macros before torch includes.
    #
    # nn_tilde.cpp includes backend.h at line 37, which includes torch/script.h
    # But somehow m_pd.h is already included before that.
    #
    # Let's check the include order and fix it by ensuring m_pd.h is included
    # after all torch headers, not before.

    # Create a wrapper that handles the macro conflict
    cat > src/frontend/puredata/nn_tilde/pd_torch_compat.h << 'COMPAT_HEADER'
#pragma once

// This header must be included BEFORE any Pure Data headers
// It saves and undefines macros that conflict with libtorch

// First, check if m_pd.h was already included
#ifdef PD_MAJOR_VERSION
#error "m_pd.h must not be included before libtorch headers! Include pd_torch_compat.h first."
#endif

// Now include all libtorch headers we need
#include <torch/script.h>
#include <torch/torch.h>

// After libtorch, it's safe to include m_pd.h
// The PD macros won't affect libtorch anymore
COMPAT_HEADER

    # Patch nn_tilde.cpp to use our compatibility header
    # Replace the torch includes with our wrapper
    sed -i 's|#include <torch/script.h>|// torch/script.h included via pd_torch_compat.h|g' src/frontend/puredata/nn_tilde/nn_tilde.cpp
    sed -i 's|#include <torch/torch.h>|// torch/torch.h included via pd_torch_compat.h|g' src/frontend/puredata/nn_tilde/nn_tilde.cpp

    # Add our compatibility header at the very beginning
    sed -i '1i #include "pd_torch_compat.h"' src/frontend/puredata/nn_tilde/nn_tilde.cpp

    # Also check backend.h - it might include torch/script.h
    if grep -q "torch/script.h" src/backend/backend.h; then
      # backend.h includes torch - we need to make sure it's included before m_pd.h
      echo "Note: backend.h includes torch headers"
    fi
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib/pd/extra/lib

    # Find and copy the external
    NN_FILE=$(find . -name "nn~.pd_linux" -print -quit)
    if [ -n "$NN_FILE" ]; then
      cp "$NN_FILE" $out/lib/pd/extra/
      echo "Installed nn~.pd_linux"
    else
      echo "ERROR: nn~.pd_linux not found!"
      find . -name "*.pd_linux" -o -name "*.so"
      exit 1
    fi

    # Install help file if available
    if [ -f ../src/help/nn~-help.pd ]; then
      cp ../src/help/nn~-help.pd $out/lib/pd/extra/
      echo "Installed nn~-help.pd"
    fi

    # Bundle libtorch libraries for runtime
    echo "Bundling libtorch libraries..."
    for lib in ${libtorch-bin}/lib/*.so*; do
      if [ -f "$lib" ]; then
        libname=$(basename "$lib")
        # Follow symlinks to get actual file
        cp -L "$lib" $out/lib/pd/extra/lib/
        echo "  Bundled $libname"
      fi
    done

    # Also bundle libc10 if separate
    for lib in ${libtorch-bin}/lib/libc10*.so*; do
      if [ -f "$lib" ]; then
        libname=$(basename "$lib")
        cp -L "$lib" $out/lib/pd/extra/lib/ 2>/dev/null || true
      fi
    done

    # Bundle libcurl (required for model download functionality)
    echo "Bundling libcurl..."
    cp -L ${curl.out}/lib/libcurl.so* $out/lib/pd/extra/lib/ 2>/dev/null || true
    echo "  Bundled libcurl"

    # Patch RPATH so nn~ finds bundled libraries
    echo "Patching RPATH..."
    patchelf --set-rpath '$ORIGIN/lib' $out/lib/pd/extra/nn~.pd_linux
    echo "RPATH set to \$ORIGIN/lib"

    # Verify the build
    echo ""
    echo "Verifying nn~ build:"
    echo "  File: $out/lib/pd/extra/nn~.pd_linux"
    echo "  Size: $(du -h $out/lib/pd/extra/nn~.pd_linux | cut -f1)"
    echo "  Libs: $(ls $out/lib/pd/extra/lib/ | wc -l) files bundled"

    runHook postInstall
  '';

  meta = with lib; {
    description = "Neural network external for Pure Data (with multi-instance support)";
    homepage = "https://github.com/acids-ircam/nn_tilde";
    license = licenses.gpl3;
    platforms = platforms.linux;
    maintainers = [ ];
  };
}
