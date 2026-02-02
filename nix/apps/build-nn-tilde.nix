# Install nn~ Pure Data external for neural audio effects
#
# This app installs the pre-built nn~ external to the mesh effects directory.
#
# Usage:
#   nix run .#build-nn-tilde
#   # Or: nix run .#build-nn-tilde -- /custom/path

{ pkgs, nn-tilde }:

pkgs.writeShellApplication {
  name = "build-nn-tilde";

  runtimeInputs = [ pkgs.coreutils ];

  text = ''
    OUTPUT_DIR="''${1:-$PWD/dist/nn~}"

    echo "╔════════════════════════════════════════════════════════════════╗"
    echo "║     Installing nn~ with Multi-Instance Support                 ║"
    echo "╚════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "Source:  ${nn-tilde}/lib/pd/extra"
    echo "Output:  $OUTPUT_DIR"
    echo ""

    # Create output directory and clean old files
    mkdir -p "$OUTPUT_DIR"
    rm -rf "''${OUTPUT_DIR:?}/nn~.pd_linux" "''${OUTPUT_DIR:?}/nn~-help.pd" "''${OUTPUT_DIR:?}/lib" 2>/dev/null || true

    # Copy nn~ external
    echo "Installing nn~.pd_linux..."
    cp "${nn-tilde}/lib/pd/extra/nn~.pd_linux" "$OUTPUT_DIR/"
    chmod +w "$OUTPUT_DIR/nn~.pd_linux"

    # Create symlink for libpd naming convention
    # libpd looks for name.linux-amd64-0.so instead of name.pd_linux
    ln -sf "nn~.pd_linux" "$OUTPUT_DIR/nn~.linux-amd64-0.so"
    echo "  ✓ Created symlink nn~.linux-amd64-0.so -> nn~.pd_linux"

    # Copy help file if present
    if [ -f "${nn-tilde}/lib/pd/extra/nn~-help.pd" ]; then
      cp "${nn-tilde}/lib/pd/extra/nn~-help.pd" "$OUTPUT_DIR/"
      echo "  ✓ nn~-help.pd"
    fi

    # Copy bundled libraries
    echo "Installing bundled libraries..."
    mkdir -p "$OUTPUT_DIR/lib"
    cp -r "${nn-tilde}/lib/pd/extra/lib/"* "$OUTPUT_DIR/lib/" 2>/dev/null || true
    chmod -R +w "$OUTPUT_DIR/lib/" 2>/dev/null || true

    # Show results
    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo "Installation complete!"
    echo ""
    echo "Files installed:"
    ls -la "$OUTPUT_DIR/nn~.pd_linux"
    echo ""
    # shellcheck disable=SC2012
    echo "Libraries bundled: $(find "$OUTPUT_DIR/lib/" -maxdepth 1 -type f 2>/dev/null | wc -l) files"
    echo ""
    echo "To use with mesh:"
    echo "  1. Copy to your effects/externals directory:"
    echo "     cp -r $OUTPUT_DIR/* ~/Music/mesh-collection/effects/externals/"
    echo ""
    echo "  2. Or set PD_EXTRA_PATH environment variable:"
    echo "     export PD_EXTRA_PATH=$OUTPUT_DIR"
    echo ""
    echo "Build info:"
    echo "  - RPATH configured: \$ORIGIN/lib (bundled libtorch)"
    echo "════════════════════════════════════════════════════════════════"
  '';
}
