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

    # Parse arguments
    MODEL_TYPE="''${1:-htdemucs}"
    OUTPUT_DIR="$(realpath -m "''${2:-./models}")"

    # Validate model type
    case "$MODEL_TYPE" in
      htdemucs|htdemucs_ft|htdemucs_6s)
        ;;
      *)
        echo "Usage: convert-model [MODEL_TYPE] [OUTPUT_DIR]"
        echo ""
        echo "MODEL_TYPE options:"
        echo "  htdemucs     - Standard 4-stem model (default, ~163MB)"
        echo "  htdemucs_ft  - Fine-tuned 4-stem model (better quality, ~163MB)"
        echo "  htdemucs_6s  - 6-stem model with piano/guitar (~200MB)"
        echo ""
        echo "Examples:"
        echo "  convert-model                    # Convert htdemucs to ./models"
        echo "  convert-model htdemucs_ft        # Convert fine-tuned model"
        echo "  convert-model htdemucs_ft ~/models"
        exit 1
        ;;
    esac

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║              Demucs ONNX Model Conversion                             ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "Model: $MODEL_TYPE"
    echo "Output: $OUTPUT_DIR"
    echo ""

    # Create output directory
    mkdir -p "$OUTPUT_DIR"

    # Check if model already exists
    if [ -f "$OUTPUT_DIR/$MODEL_TYPE.onnx" ]; then
      echo "Model already exists at $OUTPUT_DIR/$MODEL_TYPE.onnx"
      echo "Delete it first if you want to reconvert."
      exit 0
    fi

    echo "Converting $MODEL_TYPE to ONNX format..."
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

    # Determine conversion flags based on model type
    CONVERT_FLAGS=""
    case "$MODEL_TYPE" in
      htdemucs_ft)
        # Fine-tuned model requires special handling - it's a "bag of models"
        # For now, we use the base htdemucs and note that _ft needs different export
        echo "Note: htdemucs_ft uses fine-tuned weights for each stem"
        ;;
      htdemucs_6s)
        CONVERT_FLAGS="--six-source"
        ;;
    esac

    # Run conversion (patch script to use opset 18 instead of 17 for compatibility)
    echo "[2/3] Converting model (this may take a few minutes)..."
    cd "$TEMP_DIR"

    # Create modified conversion script that handles model selection
    cat > convert.py << 'PYTHON_EOF'
import sys
import torch
from torch.nn import functional as F
from pathlib import Path
from demucs.pretrained import get_model
from demucs.htdemucs import HTDemucs, standalone_spec, standalone_magnitude

model_name = sys.argv[1] if len(sys.argv) > 1 else "htdemucs"
dest_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("./onnx-output")
dest_dir.mkdir(parents=True, exist_ok=True)

print(f"Loading model: {model_name}")
model = get_model(model_name)

# Handle BagOfModels (used by htdemucs_ft)
if isinstance(model, HTDemucs):
    core_model = model
elif hasattr(model, 'models') and isinstance(model.models[0], HTDemucs):
    core_model = model.models[0]
    print(f"Note: Using first model from BagOfModels")
else:
    raise TypeError(f"Unsupported model type: {type(model)}")

print(f"Model sources: {core_model.sources}")

# Prepare dummy inputs
training_length = int(core_model.segment * core_model.samplerate)
dummy_waveform = torch.randn(1, 2, training_length)
magspec = standalone_magnitude(standalone_spec(dummy_waveform))
dummy_input = (dummy_waveform, magspec)

onnx_file = dest_dir / f"{model_name}.onnx"
print(f"Exporting to: {onnx_file}")

torch.onnx.export(
    core_model,
    dummy_input,
    onnx_file,
    export_params=True,
    opset_version=18,
    do_constant_folding=True,
    input_names=['input', 'x'],
    output_names=['output', 'add_67']
)
print(f"Success! Model saved to {onnx_file}")
PYTHON_EOF

    ${pythonEnv}/bin/python convert.py "$MODEL_TYPE" ./onnx-output 2>&1 | \
      grep -v "^$" | head -40

    # Copy to output (include external data file if present)
    echo "[3/3] Copying model to $OUTPUT_DIR..."
    mkdir -p "$OUTPUT_DIR"
    if [ -f "./onnx-output/$MODEL_TYPE.onnx" ]; then
      cp "./onnx-output/$MODEL_TYPE.onnx" "$OUTPUT_DIR/$MODEL_TYPE.onnx"

      # Copy external data file if it exists (large models store weights separately)
      if [ -f "./onnx-output/$MODEL_TYPE.onnx.data" ]; then
        cp "./onnx-output/$MODEL_TYPE.onnx.data" "$OUTPUT_DIR/$MODEL_TYPE.onnx.data"
        TOTAL_SIZE=$(du -ch "$OUTPUT_DIR/$MODEL_TYPE.onnx" "$OUTPUT_DIR/$MODEL_TYPE.onnx.data" | tail -1 | cut -f1)
        echo ""
        echo "✓ Success! Model saved to: $OUTPUT_DIR/$MODEL_TYPE.onnx ($TOTAL_SIZE total)"
        echo "  (includes external data file: $MODEL_TYPE.onnx.data)"
      else
        SIZE=$(du -h "$OUTPUT_DIR/$MODEL_TYPE.onnx" | cut -f1)
        echo ""
        echo "✓ Success! Model saved to: $OUTPUT_DIR/$MODEL_TYPE.onnx ($SIZE)"
      fi
      echo ""
      echo "To use immediately, copy to cache:"
      echo "  mkdir -p ~/.cache/mesh-cue/models"
      echo "  cp $OUTPUT_DIR/$MODEL_TYPE.onnx* ~/.cache/mesh-cue/models/"
      echo ""
      echo "For releases, upload to GitHub:"
      echo "  gh release upload models $OUTPUT_DIR/$MODEL_TYPE.onnx*"
    else
      echo "✗ Conversion failed - ONNX file not found"
      echo "Check the output above for errors."
      exit 1
    fi
  '';

in convertScript
