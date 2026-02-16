# ONNX model conversion script for Beat This! beat tracking model
# Converts PyTorch pretrained weights to ONNX format for CPU inference
#
# Beat This! (CPJKU, ISMIR 2024) is SOTA beat + downbeat tracking.
# The "small" variant (~2M params, ~8 MB) achieves Beat F1 = 88.8.
#
# Usage: nix run .#convert-beat-model [-- [VARIANT] [OUTPUT_DIR]]
{ pkgs }:

let
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    pip
  ]);

  convertScript = pkgs.writeShellScriptBin "convert-beat-model" ''
    set -euo pipefail

    VARIANT="''${1:-small}"
    OUTPUT_DIR="$(realpath -m "''${2:-./models}")"

    case "$VARIANT" in
      small)
        DESCRIPTION="Small variant (~2M params, 128-dim, 8 heads, ~8 MB)"
        ;;
      final)
        DESCRIPTION="Full variant (~20M params, 512-dim, 16 heads, ~78 MB)"
        ;;
      *)
        echo "Usage: convert-beat-model [VARIANT] [OUTPUT_DIR]"
        echo ""
        echo "Converts Beat This! PyTorch weights to ONNX format."
        echo "Paper: 'Beat This! Accurate, Fast, and Lightweight Beat Tracking' (ISMIR 2024)"
        echo ""
        echo "VARIANT options:"
        echo "  small  - Small model, ~8 MB, Beat F1=88.8 (default, recommended)"
        echo "  final  - Full model, ~78 MB, Beat F1=89.1"
        echo ""
        echo "Examples:"
        echo "  convert-beat-model                          # Convert small model"
        echo "  convert-beat-model final ./my-models        # Convert full model"
        exit 1
        ;;
    esac

    OUTPUT_NAME="beat_this_$VARIANT"

    echo "======================================================================"
    echo "  Beat This! — PyTorch to ONNX Conversion"
    echo "======================================================================"
    echo ""
    echo "Variant: $VARIANT ($DESCRIPTION)"
    echo "Output:  $OUTPUT_DIR/$OUTPUT_NAME.onnx"
    echo ""

    mkdir -p "$OUTPUT_DIR"

    if [ -f "$OUTPUT_DIR/$OUTPUT_NAME.onnx" ]; then
      echo "Model already exists at $OUTPUT_DIR/$OUTPUT_NAME.onnx"
      echo "Delete it first if you want to reconvert."
      exit 0
    fi

    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    echo "[1/3] Installing dependencies (PyTorch + beat_this)..."
    ${pythonEnv}/bin/pip install --target "$TEMP_DIR/site-packages" --no-warn-script-location \
      torch --index-url https://download.pytorch.org/whl/cpu 2>&1 | tail -3

    ${pythonEnv}/bin/pip install --target "$TEMP_DIR/site-packages" --no-warn-script-location \
      "beat_this @ git+https://github.com/CPJKU/beat_this.git" onnx 2>&1 | tail -5

    export PYTHONPATH="$TEMP_DIR/site-packages:''${PYTHONPATH:-}"

    echo "[2/3] Exporting $VARIANT model to ONNX (opset 17)..."

    cat > "$TEMP_DIR/export.py" << 'PYTHON_EOF'
import sys
import os
import torch

variant = sys.argv[1] if len(sys.argv) > 1 else "small"
output_path = sys.argv[2] if len(sys.argv) > 2 else f"beat_this_{variant}.onnx"

print(f"Loading Beat This! variant: {variant}")

from beat_this.model import BeatThis

# Load pretrained model — "small" or "final" (full)
model = BeatThis.from_pretrained(variant)
model.eval()

# Disable flash attention for ONNX compatibility if the model uses it
# Beat This! checks for flash_attn availability at runtime
for module in model.modules():
    if hasattr(module, 'use_flash_attn'):
        module.use_flash_attn = False
    # Also handle any scaled_dot_product_attention flags
    if hasattr(module, 'flash'):
        module.flash = False

# Dummy input: [batch=1, channel=1, n_frames=1500, n_mels=128]
# 1500 frames = 30 seconds at 50 fps
dummy_input = torch.randn(1, 1, 1500, 128)

print(f"Model parameters: {sum(p.numel() for p in model.parameters()):,}")
print(f"Exporting to: {output_path}")

with torch.no_grad():
    # Verify forward pass works
    beat_act, downbeat_act = model(dummy_input)
    print(f"Forward pass OK — beat: {beat_act.shape}, downbeat: {downbeat_act.shape}")

    torch.onnx.export(
        model,
        dummy_input,
        output_path,
        input_names=["mel_spectrogram"],
        output_names=["beat_activation", "downbeat_activation"],
        dynamic_axes={
            "mel_spectrogram": {0: "batch", 2: "time"},
            "beat_activation": {0: "batch", 1: "time"},
            "downbeat_activation": {0: "batch", 1: "time"},
        },
        opset_version=17,
        do_constant_folding=True,
    )

if os.path.exists(output_path):
    size_mb = os.path.getsize(output_path) / (1024 * 1024)
    print(f"Export successful: {output_path} ({size_mb:.1f} MB)")
else:
    print("Export failed — output file not created")
    sys.exit(1)
PYTHON_EOF

    ${pythonEnv}/bin/python "$TEMP_DIR/export.py" "$VARIANT" "$TEMP_DIR/$OUTPUT_NAME.onnx" 2>&1 | \
      grep -v "^$" | head -30

    echo "[3/3] Copying to output directory..."

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
