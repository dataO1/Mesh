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
  # NumPy's C extensions need libz.so.1 at import; not in the bare nix shell.
  zlibPath = "${pkgs.zlib}/lib";

  # Reference the spike's Python files so Nix puts them in the store and
  # we can `cp` them into the temp_dir at runtime.
  downloadPy = ./convert-muq-mulan/download.py;
  exportPy = ./convert-muq-mulan/export.py;
  mergeLoRaPy = ../spike/track-grading/merge_and_export_lora.py;
  validatePy = ./convert-muq-mulan/validate.py;
  benchPy = ./convert-muq-mulan/bench.py;

  convertScript = pkgs.writeShellScriptBin "convert-muq-mulan-model" ''
    set -euo pipefail

    # PyTorch needs libstdc++.so.6 visible; NumPy needs libz.so.1.
    export LD_LIBRARY_PATH="${libstdcppPath}:${zlibPath}:''${LD_LIBRARY_PATH:-}"

    # NVIDIA host driver: PyTorch needs `libcuda.so.1` (the driver shim,
    # NOT the cu124 wheel's bundled cudart) to detect the GPU. The driver
    # is installed by NixOS at /run/opengl-driver/lib but that path isn't
    # always on LD_LIBRARY_PATH inside a nix-shell. Add it if it exists.
    for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
      if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
        export LD_LIBRARY_PATH="$cand:$LD_LIBRARY_PATH"
        echo "[detect] libcuda.so.1 found at $cand"
        break
      fi
    done

    # ─── Argument parsing ────────────────────────────────────────────────
    DEVICE_OVERRIDE=""
    OUTPUT_DIR=""
    REAL_AUDIO=""
    SKIP_BENCH=0
    REINSTALL_DEPS_FLAG=0
    USE_LORA=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --cpu)            DEVICE_OVERRIDE="cpu";   shift ;;
        --gpu|--cuda)     DEVICE_OVERRIDE="cuda";  shift ;;
        --skip-bench)     SKIP_BENCH=1;            shift ;;
        --audio)          REAL_AUDIO="$2";         shift 2 ;;
        --reinstall-deps) REINSTALL_DEPS_FLAG=1;   shift ;;
        --lora)           USE_LORA="models/lora";  shift ;;
        -h|--help)
          cat <<EOF
Usage: convert-muq-mulan-model [OPTIONS] [OUTPUT_DIR]

  --cpu              Force CPU export (slow — ~10-20 min for the 630M model)
  --gpu, --cuda      Force GPU export (fails if no CUDA detected)
  --lora             Apply LoRA-v2 adapter (epoch 1, +0.90pp PA) before export
  --audio FILE       Use this audio file as a third validation case
  --skip-bench       Don't run the wall-clock benchmark after validation
  --reinstall-deps   Wipe the cached pip install and re-download (~3 GB)
  OUTPUT_DIR         Where to write the ONNX (default: ./models)

Default: auto-detect CUDA via nvidia-smi. Without --lora, exports
the frozen (baseline) encoder.

Caches (persisted across runs to avoid re-downloads):
  Pip site-packages : ~/.cache/mesh-spike/site-packages-{cpu,gpu-cu124}/
  HF model snapshot : ~/.cache/mesh-spike/muq-mulan/
EOF
          exit 0
          ;;
        *)                OUTPUT_DIR="$1";         shift ;;
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

    # Always-fresh scratch (probe scripts, ONNX intermediate). Cleaned on exit.
    TEMP_DIR=$(mktemp -d -t muq-mulan-spike.XXXXXX)
    trap "rm -rf $TEMP_DIR" EXIT

    # Pin HuggingFace caches to the spike's own dir so both the explicit
    # snapshot_download (download.py) and any later from_pretrained()
    # call (export.py / validate.py) hit the same files. Without this
    # they diverge: download.py writes to --cache-dir foo, then
    # MuQMuLan.from_pretrained() ignores that and re-downloads ~1.3 GB
    # into the default ~/.cache/huggingface/hub/ — which we just saw
    # crawl at anonymous-rate-limit speed.
    export HF_HOME="$HOME/.cache/mesh-spike/hf"
    export HF_HUB_CACHE="$HF_HOME/hub"
    mkdir -p "$HF_HUB_CACHE"

    # ─── Stage 1: install (or reuse) Python deps ─────────────────────────
    if [ "$HAS_CUDA" = "1" ]; then
      TORCH_INDEX="https://download.pytorch.org/whl/cu124"
      ORT_PKG="onnxruntime-gpu"
      DEPS_TAG="gpu-cu124"
    else
      TORCH_INDEX="https://download.pytorch.org/whl/cpu"
      ORT_PKG="onnxruntime"
      DEPS_TAG="cpu"
    fi

    # Persistent dep cache keyed by GPU/CPU variant. Subsequent runs skip
    # pip entirely (~3 GB of cu124 wheels otherwise re-downloaded each time).
    # Force a refresh by deleting the dir or passing --reinstall-deps.
    REINSTALL_DEPS=0
    if [ "''${REINSTALL_DEPS_FLAG:-0}" = "1" ]; then
      REINSTALL_DEPS=1
    fi
    SITE="$HOME/.cache/mesh-spike/site-packages-$DEPS_TAG"
    mkdir -p "$SITE"
    export PYTHONPATH="$SITE:''${PYTHONPATH:-}"

    if [ "$REINSTALL_DEPS" = "0" ] && [ -d "$SITE/torch" ] && [ -d "$SITE/muq" ]; then
      echo "[1/5] Reusing cached deps at $SITE"
      echo "      ($DEPS_TAG variant — delete the dir to force a fresh install)"
    else
      echo "[1/5] Installing Python deps into $SITE..."
      echo "      → $DEPS_TAG variant (torch from $TORCH_INDEX, $ORT_PKG)"

      # Single pip install: keeps torch + everything resolved against the
      # same indexes. The previous two-call form let the second call
      # re-resolve torch from default PyPI, pulling cu130 wheels — which
      # then mismatched onnxruntime-gpu's libcublasLt.so.12 expectation,
      # silently falling back the CUDA EP to CPU at session creation.
      # Pinning to 2.5.x keeps a known cu124-compatible pair.
      ${pythonEnv}/bin/pip install --target "$SITE" --upgrade --no-warn-script-location \
        --index-url "$TORCH_INDEX" \
        --extra-index-url "https://pypi.org/simple" \
        "torch>=2.5,<2.6" "torchaudio>=2.5,<2.6" \
        "muq>=0.1.0" \
        "huggingface_hub>=0.20" \
        "transformers>=4.40" \
        "librosa>=0.10" \
        "numpy<2.0" \
        "onnx>=1.15" \
        "$ORT_PKG>=1.18" \
        "optimum[exporters]>=1.20" 2>&1 | tail -8

      echo "[1/5] Deps installed in $SITE"
    fi

    # Add the cu124 wheel's bundled nvidia .so dirs to LD_LIBRARY_PATH so
    # PyTorch can load cuDNN/cuBLAS/etc. inside the nix shell (its normal
    # RPATH-based loading occasionally breaks here).
    if [ "$HAS_CUDA" = "1" ]; then
      NVIDIA_LIBS=""
      for d in "$SITE"/nvidia/*/lib; do
        if [ -d "$d" ]; then
          NVIDIA_LIBS="$d:$NVIDIA_LIBS"
        fi
      done
      if [ -n "$NVIDIA_LIBS" ]; then
        export LD_LIBRARY_PATH="$NVIDIA_LIBS$LD_LIBRARY_PATH"
        echo "      → added bundled nvidia libs to LD_LIBRARY_PATH"
      fi

      # Sanity-check that PyTorch can actually see the GPU before going
      # further. Surface the exact failure reason now instead of after a
      # long download or partial export attempt.
      echo "      → cuda probe..."
      cat > "$TEMP_DIR/cuda_probe.py" <<'PYEOF'
import sys, ctypes.util
import torch
ok = torch.cuda.is_available()
print(f"      torch.__version__         : {torch.__version__}")
print(f"      torch.version.cuda        : {torch.version.cuda}")
print(f"      torch.cuda.is_available() : {ok}")
if not ok:
    drv = ctypes.util.find_library("cuda")
    print(f"      libcuda visible          : {drv!r}")
    try:
        print(f"      device_count             : {torch.cuda.device_count()}")
    except Exception as e:
        print(f"      device_count raised      : {e}")
    print()
    print("      CUDA not usable from inside this shell. Likely fix:")
    print("        - On NixOS: ensure hardware.graphics (or hardware.opengl)")
    print("          is enabled and /run/opengl-driver/lib has libcuda.so.1.")
    print("        - Or re-run with --cpu to proceed without GPU (slower).")
    sys.exit(2)
PYEOF
      if ! ${pythonEnv}/bin/python "$TEMP_DIR/cuda_probe.py"; then
        echo "[!] CUDA probe failed — re-run with --cpu to fall back, or fix driver path." >&2
        exit 1
      fi
    fi
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
      if [ -n "$USE_LORA" ]; then
        echo "[3/5] Exporting LoRA-tuned audio tower to ONNX on $DEVICE..."
        ${pythonEnv}/bin/python ${mergeLoRaPy} \
          --ckpt-dir "$USE_LORA" \
          --device "$DEVICE" \
          --output "$TEMP_DIR/$OUTPUT_NAME.onnx" || {
          echo "[!] merge_and_export_lora.py failed — see error above" >&2
          exit 1
        }
      else
        echo "[3/5] Exporting audio tower to ONNX on $DEVICE..."
        ${pythonEnv}/bin/python ${exportPy} "$DEVICE" "$TEMP_DIR/$OUTPUT_NAME.onnx" || {
          echo "[!] export.py failed — see error above" >&2
          echo "    This is the spike's primary failure mode. Document the" >&2
          echo "    op/error and consider the Python-sidecar fallback per the" >&2
          echo "    decision gate in documents/embedding-models-research.md." >&2
          exit 1
        }
      fi

      cp "$TEMP_DIR/$OUTPUT_NAME.onnx" "$OUTPUT_DIR/$OUTPUT_NAME.onnx"
      # Sidecar with mel-normalization stats + MelSTFT params; Rust reads
      # this to reproduce the exact preprocessing the model was trained on.
      cp "$TEMP_DIR/$OUTPUT_NAME.onnx.norm.json" "$OUTPUT_DIR/$OUTPUT_NAME.onnx.norm.json"
      EXISTING_ONNX="$OUTPUT_DIR/$OUTPUT_NAME.onnx"
      SIZE=$(du -h "$EXISTING_ONNX" | cut -f1)
      echo "[3/5] Wrote $EXISTING_ONNX ($SIZE)"
      echo "[3/5] Wrote $EXISTING_ONNX.norm.json (mel + stats sidecar)"
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
