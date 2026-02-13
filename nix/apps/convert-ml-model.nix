# ONNX model conversion script for Essentia ML classification heads
# Converts TensorFlow frozen .pb models to ONNX format using tf2onnx
#
# The Essentia project only publishes ONNX files for the base embedding model
# (discogs-effnet-bsdynamic-1.onnx) but not for classification heads.
# This script downloads the TF .pb files and converts them to ONNX.
#
# Usage: nix run .#convert-ml-model [-- [MODEL_TYPE] [OUTPUT_DIR]]
{ pkgs }:

let
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    pip
  ]);

  convertScript = pkgs.writeShellScriptBin "convert-ml-model" ''
    set -euo pipefail

    MODEL_TYPE="''${1:-genre_discogs400}"
    OUTPUT_DIR="$(realpath -m "''${2:-./models}")"

    # Model registry: name -> (pb_url, input_node, output_node, description)
    case "$MODEL_TYPE" in
      genre_discogs400)
        PB_URL="https://essentia.upf.edu/models/classification-heads/genre_discogs400/genre_discogs400-discogs-effnet-1.pb"
        INPUT_NODE="serving_default_model_Placeholder:0"
        OUTPUT_NODE="PartitionedCall:0"
        OUTPUT_NAME="genre_discogs400-discogs-effnet-1"
        DESCRIPTION="Genre Discogs400 classification head (400 genres, ~2MB)"
        ;;
      *)
        echo "Usage: convert-ml-model [MODEL_TYPE] [OUTPUT_DIR]"
        echo ""
        echo "Converts Essentia TensorFlow classification heads to ONNX format."
        echo ""
        echo "MODEL_TYPE options:"
        echo "  genre_discogs400  - Genre classification, 400 Discogs styles (default)"
        echo ""
        echo "Examples:"
        echo "  convert-ml-model                                    # Convert genre model"
        echo "  convert-ml-model genre_discogs400 ./my-models       # Custom output dir"
        exit 1
        ;;
    esac

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║          Essentia ML Classification Head — TF → ONNX Conversion      ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "Model:  $MODEL_TYPE ($DESCRIPTION)"
    echo "Output: $OUTPUT_DIR"
    echo ""

    mkdir -p "$OUTPUT_DIR"

    if [ -f "$OUTPUT_DIR/$OUTPUT_NAME.onnx" ]; then
      echo "Model already exists at $OUTPUT_DIR/$OUTPUT_NAME.onnx"
      echo "Delete it first if you want to reconvert."
      exit 0
    fi

    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    echo "[1/3] Installing tf2onnx..."
    ${pythonEnv}/bin/pip install --target "$TEMP_DIR/site-packages" --no-warn-script-location \
      tf2onnx "tensorflow>=2.8,<2.17" onnx 2>&1 | tail -5

    export PYTHONPATH="$TEMP_DIR/site-packages:''${PYTHONPATH:-}"

    echo "[2/3] Downloading $OUTPUT_NAME.pb..."
    ${pkgs.curl}/bin/curl --fail --location --progress-bar \
      -o "$TEMP_DIR/$OUTPUT_NAME.pb" \
      "$PB_URL"
    echo ""

    echo "[3/3] Converting to ONNX (opset 15)..."
    ${pythonEnv}/bin/python -m tf2onnx.convert \
      --graphdef "$TEMP_DIR/$OUTPUT_NAME.pb" \
      --output "$TEMP_DIR/$OUTPUT_NAME.onnx" \
      --inputs "$INPUT_NODE" \
      --outputs "$OUTPUT_NODE" \
      --opset 15 \
      2>&1 | grep -v "^$" | tail -20

    if [ -f "$TEMP_DIR/$OUTPUT_NAME.onnx" ]; then
      cp "$TEMP_DIR/$OUTPUT_NAME.onnx" "$OUTPUT_DIR/$OUTPUT_NAME.onnx"
      SIZE=$(du -h "$OUTPUT_DIR/$OUTPUT_NAME.onnx" | cut -f1)
      echo ""
      echo "Success! Model saved to: $OUTPUT_DIR/$OUTPUT_NAME.onnx ($SIZE)"
      echo ""
      echo "To use immediately, copy to cache:"
      echo "  mkdir -p ~/.cache/mesh-cue/ml-models"
      echo "  cp $OUTPUT_DIR/$OUTPUT_NAME.onnx ~/.cache/mesh-cue/ml-models/"
      echo ""
      echo "For releases, upload to GitHub:"
      echo "  gh release upload models $OUTPUT_DIR/$OUTPUT_NAME.onnx"
    else
      echo "Conversion failed — ONNX file not found"
      echo "Check the output above for errors."
      ls -la "$TEMP_DIR/" 2>/dev/null || true
      exit 1
    fi
  '';

in convertScript
