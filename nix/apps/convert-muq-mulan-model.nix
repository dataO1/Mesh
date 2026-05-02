# MuQ-MuLan ONNX export spike (Phase 2 of the embedding-models research).
#
# Converts the OpenMuQ/MuQ-MuLan-large checkpoint's audio tower from
# PyTorch to ONNX so the Mesh production code can keep using the existing
# `ort`-based per-thread analyzer pattern without a Python sidecar.
#
# Auto-detects CUDA via `nvidia-smi`. With a GPU, tracing takes seconds
# (vs ~10–20 minutes on CPU) and validation also runs ONNX through the
# CUDA execution provider to confirm the production GPU path will work.
# The exported `.onnx` is device-agnostic regardless.
#
# This spike does NOT touch `crates/`. It's a one-shot developer tool —
# success/failure feeds the decision in
# `documents/embedding-models-research.md::Phase 2`.
#
# Usage:
#   nix run .#convert-muq-mulan-model               # auto-detect GPU
#   nix run .#convert-muq-mulan-model -- --cpu      # force CPU export
#   nix run .#convert-muq-mulan-model -- --gpu      # force GPU (fails if missing)
#   nix run .#convert-muq-mulan-model -- ./out      # custom output dir
{ pkgs }:

let
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    pip
  ]);

  # PyTorch pip wheels link against libstdc++.so.6 at import time;
  # in pure Nix env the system .so isn't on LD_LIBRARY_PATH by default.
  libstdcppPath = "${pkgs.stdenv.cc.cc.lib}/lib";

  # Reference the spike's Python files so Nix puts them in the store and
  # we can `cp` them into the temp_dir at runtime.
  downloadPy = ./convert-muq-mulan/download.py;
  exportPy = ./convert-muq-mulan/export.py;
  validatePy = ./convert-muq-mulan/validate.py;
  benchPy = ./convert-muq-mulan/bench.py;

  convertScript = pkgs.writeShellScriptBin "convert-muq-mulan-model" ''
    set -euo pipefail

    # PyTorch needs libstdc++.so.6 visible.
    export LD_LIBRARY_PATH="${libstdcppPath}:''${LD_LIBRARY_PATH:-}"

    # ─── Argument parsing ────────────────────────────────────────────────
    DEVICE_OVERRIDE=""
    OUTPUT_DIR=""
    REAL_AUDIO=""
    SKIP_BENCH=0
    while [ $# -gt 0 ]; do
      case "$1" in
        --cpu)        DEVICE_OVERRIDE="cpu";  shift ;;
        --gpu|--cuda) DEVICE_OVERRIDE="cuda"; shift ;;
        --skip-bench) SKIP_BENCH=1;           shift ;;
        --audio)      REAL_AUDIO="$2";        shift 2 ;;
        -h|--help)
          cat <<EOF
Usage: convert-muq-mulan-model [--cpu|--gpu] [--audio FILE] [--skip-bench] [OUTPUT_DIR]

  --cpu          Force CPU export (slow — ~10-20 min for the 630M model)
  --gpu, --cuda  Force GPU export (fails if no CUDA detected)
  --audio FILE   Use this audio file as a third validation case
  --skip-bench   Don't run the wall-clock benchmark after validation
  OUTPUT_DIR     Where to write the ONNX (default: ./models)

Default: auto-detect CUDA via nvidia-smi.
EOF
          exit 0
          ;;
        *)            OUTPUT_DIR="$1";        shift ;;
      esac
    done
    OUTPUT_DIR="$(realpath -m "''${OUTPUT_DIR:-./models}")"
    OUTPUT_NAME="muq-mulan-audio-tower"

    # ─── GPU detection ───────────────────────────────────────────────────
    DEVICE="cpu"
    HAS_CUDA=0
    if [ "$DEVICE_OVERRIDE" = "cuda" ]; then
      DEVICE="cuda"
      HAS_CUDA=1
    elif [ "$DEVICE_OVERRIDE" = "cpu" ]; then
      DEVICE="cpu"
    elif command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1; then
      DEVICE="cuda"
      HAS_CUDA=1
      GPU_NAME=$(nvidia-smi --query-gpu=name --format=csv,noheader,nounits | head -1)
      echo "[detect] CUDA GPU found: $GPU_NAME"
    else
      echo "[detect] no CUDA — falling back to CPU (will be slow)"
    fi

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║          MuQ-MuLan ONNX Export Spike  —  audio tower                 ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo "Device : $DEVICE"
    echo "Output : $OUTPUT_DIR/$OUTPUT_NAME.onnx"
    echo ""

    mkdir -p "$OUTPUT_DIR"

    if [ -f "$OUTPUT_DIR/$OUTPUT_NAME.onnx" ]; then
      echo "[!] Output already exists: $OUTPUT_DIR/$OUTPUT_NAME.onnx"
      echo "    Delete it first if you want to re-export."
      echo "    Validating + benching the existing file instead."
      EXISTING_ONNX="$OUTPUT_DIR/$OUTPUT_NAME.onnx"
      SKIP_DOWNLOAD_AND_EXPORT=1
    else
      EXISTING_ONNX=""
      SKIP_DOWNLOAD_AND_EXPORT=0
    fi

    TEMP_DIR=$(mktemp -d -t muq-mulan-spike.XXXXXX)
    trap "rm -rf $TEMP_DIR" EXIT
    SITE="$TEMP_DIR/site-packages"
    mkdir -p "$SITE"
    export PYTHONPATH="$SITE:''${PYTHONPATH:-}"

    # ─── Stage 1: install Python deps ────────────────────────────────────
    echo "[1/5] Installing Python deps into temp dir (one-shot, not persisted)..."
    if [ "$HAS_CUDA" = "1" ]; then
      TORCH_INDEX="https://download.pytorch.org/whl/cu124"
      ORT_PKG="onnxruntime-gpu"
      echo "      → torch from cu124 index, onnxruntime-gpu"
    else
      TORCH_INDEX="https://download.pytorch.org/whl/cpu"
      ORT_PKG="onnxruntime"
      echo "      → torch from cpu index, onnxruntime (CPU-only)"
    fi

    ${pythonEnv}/bin/pip install --target "$SITE" --no-warn-script-location \
      --index-url "$TORCH_INDEX" \
      --extra-index-url "https://pypi.org/simple" \
      "torch>=2.2,<2.6" "torchaudio>=2.2,<2.6" 2>&1 | tail -3

    ${pythonEnv}/bin/pip install --target "$SITE" --no-warn-script-location \
      "muq>=0.1.0" \
      "huggingface_hub>=0.20" \
      "transformers>=4.40" \
      "librosa>=0.10" \
      "numpy<2.0" \
      "onnx>=1.15" \
      "$ORT_PKG>=1.18" \
      "optimum[exporters]>=1.20" 2>&1 | tail -5

    echo "[1/5] Deps installed in $SITE"
    echo ""

    if [ "$SKIP_DOWNLOAD_AND_EXPORT" = "0" ]; then
      # ─── Stage 2: download checkpoint ──────────────────────────────────
      echo "[2/5] Downloading MuQ-MuLan-large checkpoint (~2.65 GB, idempotent)..."
      ${pythonEnv}/bin/python ${downloadPy} || {
        echo "[!] download.py failed" >&2
        exit 1
      }
      echo ""

      # ─── Stage 3: ONNX export ──────────────────────────────────────────
      echo "[3/5] Exporting audio tower to ONNX on $DEVICE..."
      ${pythonEnv}/bin/python ${exportPy} "$DEVICE" "$TEMP_DIR/$OUTPUT_NAME.onnx" || {
        echo "[!] export.py failed — see error above" >&2
        echo "    This is the spike's primary failure mode. Document the" >&2
        echo "    op/error and consider the Python-sidecar fallback per the" >&2
        echo "    decision gate in documents/embedding-models-research.md." >&2
        exit 1
      }

      cp "$TEMP_DIR/$OUTPUT_NAME.onnx" "$OUTPUT_DIR/$OUTPUT_NAME.onnx"
      EXISTING_ONNX="$OUTPUT_DIR/$OUTPUT_NAME.onnx"
      SIZE=$(du -h "$EXISTING_ONNX" | cut -f1)
      echo "[3/5] Wrote $EXISTING_ONNX ($SIZE)"
      echo ""
    else
      echo "[2-3/5] Skipped download + export (using existing $EXISTING_ONNX)"
      echo ""
    fi

    # ─── Stage 4: validate ───────────────────────────────────────────────
    echo "[4/5] Validating ONNX vs PyTorch reference (cosine ≥ 0.9999)..."
    VALIDATE_ARGS=("$EXISTING_ONNX" "$DEVICE")
    if [ -n "$REAL_AUDIO" ]; then
      VALIDATE_ARGS+=("$REAL_AUDIO")
    fi
    ${pythonEnv}/bin/python ${validatePy} "''${VALIDATE_ARGS[@]}" || {
      echo "[!] validate.py reported FAIL — see numbers above" >&2
      echo "    The ONNX exists but doesn't numerically match PyTorch." >&2
      echo "    Likely cause: an op fell back to a less-precise ONNX kernel" >&2
      echo "    during tracing. Inspect the export warnings + try" >&2
      echo "    --opset-version 18 or torch.onnx.dynamo_export." >&2
      exit 1
    }
    echo ""

    # ─── Stage 5: benchmark ──────────────────────────────────────────────
    if [ "$SKIP_BENCH" = "0" ]; then
      echo "[5/5] Benchmarking inference latency (CPU + GPU if present)..."
      ${pythonEnv}/bin/python ${benchPy} "$EXISTING_ONNX" 20 || {
        echo "[!] bench.py failed (non-fatal)" >&2
      }
    else
      echo "[5/5] Skipped benchmark (--skip-bench)"
    fi
    echo ""

    echo "════════════════════════════════════════════════════════════════════════"
    echo "Spike complete."
    echo ""
    echo "Result    : $EXISTING_ONNX"
    echo "Next step : if validation passed, document the I/O signature in"
    echo "            documents/muq-mulan-onnx-export.md and proceed to the"
    echo "            production-integration scope (see Phase 2 open tasks in"
    echo "            documents/embedding-models-research.md)."
    echo ""
    echo "To inspect the ONNX I/O programmatically:"
    echo "  python -c \"import onnx; m = onnx.load('$EXISTING_ONNX');"
    echo "  print([(i.name, [d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]) for i in m.graph.input]);"
    echo "  print([(o.name, [d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]) for o in m.graph.output])\""
  '';

in convertScript
