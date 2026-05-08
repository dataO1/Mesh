#!/usr/bin/env bash
# Thin shim for round-7 spike Python steps.
#
# Inside `nix develop .#mlspike` the devshell hook already sets:
#   LD_LIBRARY_PATH (CUDA driver + zlib + libstdc++ + bundled NVIDIA wheels)
#   TRITON_LIBCUDA_PATH, HF_HOME, HF_HUB_CACHE
#   PATH (venv bin first), PYTHONUNBUFFERED, OMP_NUM_THREADS, …
# This wrapper just exec's the requested script through the venv python.
#
# Standalone invocation (outside the devshell) is supported as a fallback —
# the env-detection block below mirrors what the devshell does. Prefer the
# devshell on dev machines; this fallback is for one-off remote runs.
#
# Usage:
#   bash spike/track-grading/run_r7_step.sh embed_corpus_mulan.py [args...]
set -euo pipefail

VENV="$HOME/.cache/mesh-spike/vllm-env"

if [ "${MESH_MLSPIKE_ENV:-}" != "1" ]; then
  # Fallback path — devshell hasn't set the env. Apply minimum needed.
  echo "[run_r7_step] MESH_MLSPIKE_ENV unset — using fallback env setup." >&2
  echo "[run_r7_step] For permanent setup: nix develop .#mlspike" >&2
  export PYTHONUNBUFFERED=1
  : "${OMP_NUM_THREADS:=16}"
  : "${OPENBLAS_NUM_THREADS:=16}"
  : "${MKL_NUM_THREADS:=16}"
  : "${NUMEXPR_NUM_THREADS:=16}"
  export OMP_NUM_THREADS OPENBLAS_NUM_THREADS MKL_NUM_THREADS NUMEXPR_NUM_THREADS

  # zlib (numpy import) — discover dynamically (Nix store paths change on GC)
  ZLIB_LIBDIR=$(ls /nix/store/*zlib-1.3*/lib/libz.so.1 2>/dev/null | sort -V | tail -1 | xargs -r dirname)
  STDCPP_LIBDIR=$(ls /nix/store/*gcc-1*-lib/lib/libstdc++.so.6 2>/dev/null | sort -V | tail -1 | xargs -r dirname)
  [ -n "$ZLIB_LIBDIR" ]   && export LD_LIBRARY_PATH="$ZLIB_LIBDIR:${LD_LIBRARY_PATH:-}"
  [ -n "$STDCPP_LIBDIR" ] && export LD_LIBRARY_PATH="$STDCPP_LIBDIR:${LD_LIBRARY_PATH:-}"

  # CUDA driver discovery
  for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
    if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
      export LD_LIBRARY_PATH="$cand:${LD_LIBRARY_PATH:-}"
      export TRITON_LIBCUDA_PATH="$cand"
      break
    fi
  done

  # Bundled NVIDIA wheels (torch needs cublas/cudnn/cusparse/nvjitlink/...)
  for d in "$VENV"/lib/python3.11/site-packages/nvidia/*/lib; do
    [ -d "$d" ] && export LD_LIBRARY_PATH="$d:${LD_LIBRARY_PATH:-}"
  done
fi

SCRIPT="${1:-}"
if [ -z "$SCRIPT" ]; then
  echo "usage: bash run_r7_step.sh <script.py> [args...]" >&2
  exit 2
fi
shift
exec "$VENV/bin/python" "spike/track-grading/$SCRIPT" "$@"
