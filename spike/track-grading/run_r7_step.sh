#!/usr/bin/env bash
# Wrapper that sets up LD_LIBRARY_PATH for CUDA + zlib so torch.cuda is
# available, then exec's the requested round-7 python step.
#
# Usage:
#   bash run_r7_step.sh embed_corpus_mulan.py [args...]
#   bash run_r7_step.sh train_axes_r7.py
#   bash run_r7_step.sh joint_blend_r7.py
set -euo pipefail

VENV="$HOME/.cache/mesh-spike/vllm-env"

export PYTHONUNBUFFERED=1
# zlib + libstdc++ needed by spike venv on NixOS (latter for cozo embedded).
export LD_LIBRARY_PATH="/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:/nix/store/1xw5xccqqh1xw3mvd70hyil6x418wxcm-gcc-14.3.0-lib/lib:${LD_LIBRARY_PATH:-}"
for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
  if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
    export LD_LIBRARY_PATH="$cand:$LD_LIBRARY_PATH"
    export TRITON_LIBCUDA_PATH="$cand"
    break
  fi
done
NVIDIA_LIBS=""
for d in "$VENV"/lib/python3.11/site-packages/nvidia/*/lib; do
  [ -d "$d" ] && NVIDIA_LIBS="$d:$NVIDIA_LIBS"
done
[ -n "$NVIDIA_LIBS" ] && export LD_LIBRARY_PATH="$NVIDIA_LIBS$LD_LIBRARY_PATH"

SCRIPT="${1:-}"
if [ -z "$SCRIPT" ]; then
  echo "usage: bash run_r7_step.sh <script.py> [args...]" >&2
  exit 2
fi
shift
exec "$VENV/bin/python" "spike/track-grading/$SCRIPT" "$@"
