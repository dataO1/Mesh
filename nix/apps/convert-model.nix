# ONNX model conversion script
# Converts Demucs PyTorch weights to ONNX format for stem separation
{ pkgs, demucs-onnx }:

let
  # Minimal Python environment - dependencies installed via pip at runtime
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    pip
  ]);

  # The conversion script
  convertScript = pkgs.writeShellScriptBin "convert-model" ''
    set -euo pipefail

    # Output directory - convert to absolute path for later use after cd
    OUTPUT_DIR="$(realpath -m "''${1:-./models}")"

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║              Demucs ONNX Model Conversion                             ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""

    # Create output directory
    mkdir -p "$OUTPUT_DIR"

    # Check if model already exists
    if [ -f "$OUTPUT_DIR/htdemucs.onnx" ]; then
      echo "Model already exists at $OUTPUT_DIR/htdemucs.onnx"
      echo "Delete it first if you want to reconvert."
      exit 0
    fi

    echo "Converting Demucs PyTorch model to ONNX format..."
    echo "This will download the PyTorch weights (~1GB) on first run."
    echo ""

    # Create temp directory for conversion
    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    # Install Python dependencies to temp directory
    # Use --no-deps on dora-search to avoid re-downloading torch (~2GB)
    echo "[1/3] Installing dependencies..."
    ${pythonEnv}/bin/pip install --target "$TEMP_DIR/site-packages" --no-warn-script-location \
      omegaconf retrying treetable submitit cloudpickle openunmix julius diffq einops onnxscript
    ${pythonEnv}/bin/pip install --target "$TEMP_DIR/site-packages" --no-deps dora-search

    # Set up Python path (include demucs fork directly, no install needed)
    export PYTHONPATH="${demucs-onnx}/demucs-for-onnx:$TEMP_DIR/site-packages:''${PYTHONPATH:-}"

    # Run conversion (patch script to use opset 18 instead of 17 for compatibility)
    echo "[2/3] Converting model (this may take a few minutes)..."
    cd "$TEMP_DIR"
    sed 's/opset_version=17/opset_version=18/' "${demucs-onnx}/scripts/convert-pth-to-onnx.py" > convert.py
    ${pythonEnv}/bin/python convert.py ./onnx-output 2>&1 | \
      grep -v "^$" | head -30

    # Copy to output (include external data file if present)
    # Keep original htdemucs.onnx name since the .onnx file references the .data file by name
    echo "[3/3] Copying model to $OUTPUT_DIR..."
    mkdir -p "$OUTPUT_DIR"
    if [ -f "./onnx-output/htdemucs.onnx" ]; then
      cp "./onnx-output/htdemucs.onnx" "$OUTPUT_DIR/htdemucs.onnx"

      # Copy external data file if it exists (large models store weights separately)
      if [ -f "./onnx-output/htdemucs.onnx.data" ]; then
        cp "./onnx-output/htdemucs.onnx.data" "$OUTPUT_DIR/htdemucs.onnx.data"
        TOTAL_SIZE=$(du -ch "$OUTPUT_DIR/htdemucs.onnx" "$OUTPUT_DIR/htdemucs.onnx.data" | tail -1 | cut -f1)
        echo ""
        echo "✓ Success! Model saved to: $OUTPUT_DIR/htdemucs.onnx ($TOTAL_SIZE total)"
        echo "  (includes external data file: htdemucs.onnx.data)"
      else
        SIZE=$(du -h "$OUTPUT_DIR/htdemucs.onnx" | cut -f1)
        echo ""
        echo "✓ Success! Model saved to: $OUTPUT_DIR/htdemucs.onnx ($SIZE)"
      fi
      echo ""
      echo "To use immediately, copy to cache:"
      echo "  mkdir -p ~/.cache/mesh-cue/models"
      echo "  cp $OUTPUT_DIR/htdemucs.onnx* ~/.cache/mesh-cue/models/"
      echo ""
      echo "For releases, upload to GitHub:"
      echo "  gh release upload models $OUTPUT_DIR/htdemucs.onnx*"
    else
      echo "✗ Conversion failed - ONNX file not found"
      echo "Check the output above for errors."
      exit 1
    fi
  '';

in convertScript
