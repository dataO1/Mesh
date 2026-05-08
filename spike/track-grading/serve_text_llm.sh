#!/usr/bin/env bash
# Stage S4 helper — vLLM serve for the text-LLM intensity rater.
#
# Caller picks the model via TEXT_LLM_MODEL (no default — too easy to ship
# a quietly-wrong choice). Use AWQ-Marlin or bf16 weights; avoid NVFP4 and
# fp8-block-scale on this Blackwell SM120 box (those need FlashInfer JIT,
# which is fragile on NixOS).
#
# Port 8002 to coexist with Music Flamingo on 8001 (if both fit in VRAM at
# bf16 they can run side-by-side; otherwise stop MF before starting this).
#
# Usage:
#   TEXT_LLM_MODEL=Qwen/Qwen2.5-7B-Instruct bash spike/track-grading/serve_text_llm.sh
# Endpoint: http://localhost:8002/v1/chat/completions
# Stop:     pkill -f vllm.entrypoints.openai  (or Ctrl-C)
set -euo pipefail

VENV="$HOME/.cache/mesh-spike/vllm-env"
HF_HOME="$HOME/.cache/mesh-spike/hf"
# Local secondary juror — runs on the dev machine's RTX 5090 Mobile (24 GB,
# Blackwell SM120). The model is selected by the caller via TEXT_LLM_MODEL.
# Constraint: avoid kernel formats that need FlashInfer JIT (NVFP4 / fp8
# block-scale) — those require a CUDA toolkit + ninja in scope to build SM120
# kernels at startup, which is fragile on NixOS. Prefer AWQ-Marlin or bf16
# weights that ship pre-built kernels.
MODEL="${TEXT_LLM_MODEL:?TEXT_LLM_MODEL must be set; pick an AWQ or bf16 model that fits in 24 GB}"
PORT="${TEXT_LLM_PORT:-8002}"

if [ ! -x "$VENV/bin/vllm" ] && [ ! -f "$VENV/bin/activate" ]; then
  echo "[serve-text-llm] missing venv at $VENV — run pip install vllm first" >&2
  exit 1
fi

export HF_HOME HF_HUB_CACHE="$HF_HOME/hub"
# CUDA driver discovery (NixOS) — same pattern as serve_qwen3_omni.sh.
for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
  if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
    export LD_LIBRARY_PATH="$cand:${LD_LIBRARY_PATH:-}"
    export TRITON_LIBCUDA_PATH="$cand"
    break
  fi
done
NVIDIA_LIBS=""
for d in "$VENV"/lib/python3.11/site-packages/nvidia/*/lib; do
  [ -d "$d" ] && NVIDIA_LIBS="$d:$NVIDIA_LIBS"
done
[ -n "$NVIDIA_LIBS" ] && export LD_LIBRARY_PATH="$NVIDIA_LIBS$LD_LIBRARY_PATH"

export PATH="$VENV/bin:$PATH"

# bf16 with modest gpu-memory-utilization so this can coexist with Music
# Flamingo if both are running. If MF is already at ~21 GB on the 24 GB
# 5090 Mobile, this will OOM — stop MF first.
# Tuning notes (perf audit, 2026-05-07, R11):
# - With MF stopped (sequential GPU usage), text-LLM can take 0.70 utilisation
#   (~16 GB) instead of 0.55 (~13 GB), enabling larger KV → max_num_seqs 32.
# - Workers in caption_intensity_rating bumped to 24 to feed the larger queue.
# - `--enforce-eager` removed to enable CUDA graphs; dramatic speedup for
#   short generations (4 tokens) which are launch-overhead-bound.
# Mistral-line models need `--tokenizer-mode mistral` to load the v3
# Tekken tokenizer + Mistral chat template correctly. NOT adding
# `--config-format mistral --load-format mistral` because community
# AWQ/GPTQ/FP8 quants ship HF-format configs (params.json absent),
# and vLLM auto-detects the correct format. For the original
# `mistralai/...` non-quantized weights, vLLM also auto-detects.
EXTRA=""
if [[ "$MODEL" == *[Mm]istral* ]]; then
  # Mistral-Small-3.2 ships as a Pixtral multimodal model; we're text-only.
  # disable the image input slot so vLLM's MM-profiling skips broken paths.
  EXTRA="--limit-mm-per-prompt {\"image\":0}"
  # Note: community quants (AWQ, NVFP4, etc.) repackage the model in HF
  # format and DROP params.json, so --config-format mistral / --load-format
  # mistral always fail with these. We use HF mode and rely on the
  # transformers tokenizer (regex bug affects <1% of tokens, none of which
  # are single ASCII digits 1-5/0-9 — safe for our use case).
  if [[ "$MODEL" == *AWQ* ]]; then
    EXTRA="$EXTRA --quantization awq_marlin"
  fi
fi

# Concurrency knobs — env-tunable so you can rescale without editing.
# Defaults are tuned for AWQ-int4 24B Mistral on RTX 5090 Mobile (24 GB):
#   max_num_seqs=48 + workers=64 → ~5-8 c/s on 1024-token caption prompts
#   max_num_seqs=32 + workers=24 → ~2 c/s (previous, undersized)
# KV cache scales linearly: at 9 running seqs we observed 16.2% (1.8%/seq),
# so 48 seqs projects to ~85% — pushing the limit but inside the budget.
# If you see KV >90% or "Preempted: N" lines, drop MAX_NUM_SEQS to 32.
MAX_NUM_SEQS="${MAX_NUM_SEQS:-48}"
GPU_MEM_UTIL="${GPU_MEM_UTIL:-0.92}"

# torch.compile / CUDA graphs path — previously disabled because vLLM
# 0.20.1 needed `ptxas-blackwell` which isn't bundled with stock Triton on
# Blackwell SM120. We've since patched the bundled triton ELFs (see
# nix/devshell-mlspike.nix shellHook → patchelf step), so AOT compilation
# now works on this box. Re-enabling CUDA graphs gives +25-40% throughput
# on small max_num_seqs. Set FORCE_EAGER=1 to fall back to eager if you
# see InductorError or "Cannot find ptxas-blackwell" at startup.
EAGER_FLAG=""
if [ "${FORCE_EAGER:-0}" = "1" ]; then
  EAGER_FLAG="--enforce-eager"
fi

exec "$VENV/bin/python" -m vllm.entrypoints.openai.api_server \
  --model "$MODEL" \
  --dtype bfloat16 \
  --max-model-len 4096 \
  --max-num-seqs "$MAX_NUM_SEQS" \
  --gpu-memory-utilization "$GPU_MEM_UTIL" \
  $EAGER_FLAG \
  --trust-remote-code \
  $EXTRA \
  --host 0.0.0.0 \
  --port "$PORT"
