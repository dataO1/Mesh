# ONNX model conversion script
# Converts Demucs PyTorch weights to ONNX format for stem separation
{ pkgs, demucs-onnx }:

let
  # Python environment with all dependencies for conversion
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    torch
    numpy
    onnxruntime
    onnx
    tqdm
  ]);

  # The conversion script
  convertScript = pkgs.writeShellScriptBin "convert-model" ''
    set -euo pipefail

    # Output directory in repo root
    SCRIPT_DIR="$(cd "$(dirname "''${BASH_SOURCE[0]}")" && pwd)"
    # When run via nix run, we're in the repo root
    OUTPUT_DIR="''${1:-./models}"

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║              Demucs ONNX Model Conversion                             ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""

    # Create output directory
    mkdir -p "$OUTPUT_DIR"

    # Check if model already exists
    if [ -f "$OUTPUT_DIR/demucs-4stems.onnx" ]; then
      echo "Model already exists at $OUTPUT_DIR/demucs-4stems.onnx"
      echo "Delete it first if you want to reconvert."
      exit 0
    fi

    echo "Converting Demucs PyTorch model to ONNX format..."
    echo "This will download the PyTorch weights (~1GB) on first run."
    echo ""

    # Create temp directory for conversion
    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    # Install the local demucs fork (required for conversion)
    echo "[1/3] Installing demucs fork..."
    cd "${demucs-onnx}/demucs-for-onnx"
    ${pythonEnv}/bin/pip install --quiet --target "$TEMP_DIR/site-packages" -e . 2>/dev/null || true

    # Set up Python path
    export PYTHONPATH="$TEMP_DIR/site-packages:${demucs-onnx}/demucs-for-onnx:$PYTHONPATH"

    # Run conversion
    echo "[2/3] Converting model (this may take a few minutes)..."
    cd "$TEMP_DIR"
    ${pythonEnv}/bin/python "${demucs-onnx}/scripts/convert-pth-to-onnx.py" ./onnx-output 2>&1 | \
      grep -v "^$" | head -20

    # Copy to output
    echo "[3/3] Copying model to $OUTPUT_DIR..."
    if [ -f "./onnx-output/htdemucs.onnx" ]; then
      cp "./onnx-output/htdemucs.onnx" "$OUTPUT_DIR/demucs-4stems.onnx"
      SIZE=$(du -h "$OUTPUT_DIR/demucs-4stems.onnx" | cut -f1)
      echo ""
      echo "✓ Success! Model saved to: $OUTPUT_DIR/demucs-4stems.onnx ($SIZE)"
      echo ""
      echo "To use immediately, copy to cache:"
      echo "  mkdir -p ~/.cache/mesh-cue/models"
      echo "  cp $OUTPUT_DIR/demucs-4stems.onnx ~/.cache/mesh-cue/models/"
      echo ""
      echo "For releases, upload to GitHub:"
      echo "  gh release upload models $OUTPUT_DIR/demucs-4stems.onnx"
    else
      echo "✗ Conversion failed - ONNX file not found"
      echo "Check the output above for errors."
      exit 1
    fi
  '';

in convertScript
