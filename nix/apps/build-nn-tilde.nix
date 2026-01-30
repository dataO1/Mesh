# Build nn~ Pure Data external for neural audio processing
#
# This app builds the nn~ external and outputs it to the current directory.
# The resulting nn~.pd_linux can be uploaded as a GitHub release asset.
#
# Usage:
#   nix run .#build-nn-tilde
#
# Output:
#   ./nn~.pd_linux - The compiled external (Linux)
#   ./nn~-help.pd  - Help patch

{ pkgs }:

pkgs.writeShellApplication {
  name = "build-nn-tilde";

  runtimeInputs = with pkgs; [
    git
    cmake
    gnumake
    gcc
    puredata
    libtorch-bin
    curl
    curl.dev
  ];

  text = ''
    set -euo pipefail

    # Output goes to dist/nn~/ in the project root
    PROJECT_DIR="$(pwd)"
    OUTPUT_DIR="$PROJECT_DIR/dist/nn~"
    mkdir -p "$OUTPUT_DIR"

    echo "╔════════════════════════════════════════════════════════════════╗"
    echo "║           Building nn~ Pure Data External                      ║"
    echo "╚════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "Output will be written to: $OUTPUT_DIR"

    BUILD_DIR=$(mktemp -d)
    trap 'rm -rf "$BUILD_DIR"' EXIT

    echo ""
    echo "Step 1: Cloning nn_tilde repository..."
    git clone --recurse-submodules --depth 1 \
      https://github.com/acids-ircam/nn_tilde.git \
      "$BUILD_DIR/nn_tilde"

    echo ""
    echo "Step 2: Setting up build environment..."
    cd "$BUILD_DIR/nn_tilde"

    # Create env/lib structure expected by nn_tilde CMakeLists.txt
    mkdir -p env/lib env/include
    ln -sf "${pkgs.curl.out}/lib/libcurl.so" env/lib/
    ln -sf "${pkgs.curl.dev}/include/curl" env/include/

    echo ""
    echo "Step 3: Configuring with CMake..."
    cd src
    mkdir -p build && cd build
    cmake .. -DCMAKE_BUILD_TYPE=Release

    echo ""
    echo "Step 4: Building nn~..."
    make -j"$(nproc)"

    echo ""
    echo "Step 5: Copying output files to $OUTPUT_DIR ..."

    # Find and copy the external
    NN_FILE=$(find . -name "nn~.pd_linux" -print -quit)
    if [ -n "$NN_FILE" ]; then
      cp "$NN_FILE" "$OUTPUT_DIR/nn~.pd_linux"
      echo "  ✓ nn~.pd_linux"
    else
      echo "  ✗ nn~.pd_linux not found!"
      exit 1
    fi

    # Copy help file if available
    if [ -f "../help/nn~-help.pd" ]; then
      cp "../help/nn~-help.pd" "$OUTPUT_DIR/nn~-help.pd"
      echo "  ✓ nn~-help.pd"
    fi

    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo "Build complete!"
    echo ""
    echo "Output files in dist/nn~/:"
    ls -la "$OUTPUT_DIR"/nn~* 2>/dev/null || echo "  (no files found)"
    echo ""
    echo "To upload as GitHub release:"
    echo "  gh release upload <tag> dist/nn~/nn~.pd_linux dist/nn~/nn~-help.pd"
    echo ""
    echo "To install for mesh:"
    echo "  cp dist/nn~/nn~.pd_linux ~/Music/mesh-collection/effects/externals/"
    echo "════════════════════════════════════════════════════════════════"
  '';
}
