#!/usr/bin/env bash
# Spike-local vLLM serve for Qwen3-Omni-30B-A3B-Instruct (AWQ 4-bit).
#
# Why this exists: AF3's processor enforces 1:1 text-to-audio (concat-audio
# fallback gave 100% positional bias). Qwen3-Omni natively supports
# multi-audio per chat turn. cpatonn's AWQ build broke transformers' MoE
# unpacker, but vLLM has its own AWQ-marlin loader that handles the packed
# format. Confirmed model already in HF cache from prior download attempt.
#
# Run:
#   bash spike/track-grading/serve_qwen3_omni.sh
# Endpoint: http://localhost:8000/v1/chat/completions (OpenAI-compatible)
# Stop: kill the process or Ctrl-C.
set -euo pipefail

VENV="$HOME/.cache/mesh-spike/vllm-env"
HF_HOME="$HOME/.cache/mesh-spike/hf"
MODEL="cpatonn/Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit"
PORT="${VLLM_PORT:-8000}"

if [ ! -x "$VENV/bin/vllm" ] && [ ! -f "$VENV/bin/activate" ]; then
  echo "[serve] missing venv at $VENV — run pip install vllm first" >&2
  exit 1
fi

export HF_HOME HF_HUB_CACHE="$HF_HOME/hub"
# Detect host CUDA driver. Set both LD_LIBRARY_PATH (vLLM bootstrap) and
# TRITON_LIBCUDA_PATH (Triton skips its `/sbin/ldconfig -p` probe when set —
# the Nix devshell sandbox doesn't have /sbin/ldconfig at all).
for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
  if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
    export LD_LIBRARY_PATH="$cand:${LD_LIBRARY_PATH:-}"
    export TRITON_LIBCUDA_PATH="$cand"
    break
  fi
done
# Also expose the bundled nvidia/* libs from the venv so Triton can find
# libcudart, libcublas, etc. during JIT-compiled kernel loads.
NVIDIA_LIBS=""
for d in "$VENV"/lib/python3.11/site-packages/nvidia/*/lib; do
  [ -d "$d" ] && NVIDIA_LIBS="$d:$NVIDIA_LIBS"
done
[ -n "$NVIDIA_LIBS" ] && export LD_LIBRARY_PATH="$NVIDIA_LIBS$LD_LIBRARY_PATH"

# vLLM in-venv binary; --quantization awq_marlin uses vLLM's MoE-aware AWQ
# kernel (not transformers'). --limit-mm-per-prompt audio=2 enables the
# multi-audio path. --enforce-eager skips CUDA graph compile (saves ~1 GB
# and avoids long startup on first request).
exec "$VENV/bin/python" -m vllm.entrypoints.openai.api_server \
  --model "$MODEL" \
  --dtype auto \
  --max-model-len 8192 \
  --gpu-memory-utilization 0.92 \
  --enforce-eager \
  --limit-mm-per-prompt '{"audio": 2, "image": 0, "video": 0}' \
  --trust-remote-code \
  --host 0.0.0.0 \
  --port "$PORT"
