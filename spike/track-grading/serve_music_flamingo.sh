#!/usr/bin/env bash
# Spike-local vLLM serve for Music Flamingo (NVIDIA, Nov 2025).
#
# Why bf16, not FP8: user preference is quality > speed. bf16 gives the
# native-precision encoder + projector + Qwen2.5-7B text path, no
# quantization drift on subjective music tasks. Cost: ~16 GB weights
# (vs ~9 GB FP8) → max_num_seqs=4 (vs 8) → ~1.0-1.4 K=4 calls/sec
# (vs 2.0-2.5).
#
# Port 8001 to coexist with Qwen3-Omni on 8000.
#
# Run:
#   bash spike/track-grading/serve_music_flamingo.sh
# Endpoint: http://localhost:8001/v1/chat/completions (OpenAI-compatible)
# Stop: Ctrl-C / pkill -f music_flamingo
set -euo pipefail

VENV="$HOME/.cache/mesh-spike/vllm-env"
HF_HOME="$HOME/.cache/mesh-spike/hf"
MODEL="${MUSIC_FLAMINGO_MODEL:-nvidia/music-flamingo-2601-hf}"
PORT="${MUSIC_FLAMINGO_PORT:-8001}"

if [ ! -x "$VENV/bin/vllm" ] && [ ! -f "$VENV/bin/activate" ]; then
  echo "[serve-mf] missing venv at $VENV — run pip install vllm first" >&2
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

# Music Flamingo / AF3 require transformers ≥ 5.0.0.dev for the
# MusicFlamingoForConditionalGeneration / AudioFlamingo3 classes. We
# already have 5.7.0+ in this venv. vLLM ≥ 0.13.0 has native support
# (PRs 32696, 35535, 37643). We're on 0.20.1 → all fixes included.

# Memory budget on RTX 5090 Mobile (24 GB):
#   bf16 weights ≈ 16 GB (Qwen2.5-7B ~14 GB + AF-Whisper 635M ~1.3 GB
#                          + projector ~50 MB)
#   ≤ ~5 GB for KV + activations at max_num_seqs=4, max_model_len=4096
#   = ~21 GB total → leave 0.92 utilisation cap
#
# K=4 prompt budget (per Music Flamingo paper §3):
#   audio: 4 × 30s × 25 tok/s = 3000 tokens
#   text + special tokens: ~400
#   generation budget: 80
#   total: ~3500 tokens, fits in max_model_len=4096
#
# limit-mm-per-prompt audio=4 enables four <sound> placeholders per turn.
# mm-processor-cache-gb=6 lets the AF-Whisper encoder cache outputs
# across calls touching the same track ID — big win under BALD where each
# track participates in many tuples.

# Tuning notes (perf audit, 2026-05-07 → 2026-05-08):
# R1 — `--enforce-eager` removed to let vLLM build CUDA graphs (+25-40% decode
#       throughput on 7B bf16 with small max_num_seqs). Adds ~30-60s warm-up
#       at server start; ~500 MB extra VRAM for graph captures.
# R2 — `max_num_seqs` raised 4→6 to fully utilise the GPU (workers were queueing).
# R3 — `mm_processor_cache_gb` reduced 6→2 because the caption sweep hits each
#       track exactly once (encoder cache hit rate is 0% by construction). For
#       tournament/Likert sweeps (where the same track appears in many tuples),
#       set MM_CACHE_GB=6 in env.
# R4 — `max_num_seqs` raised 6→12 after switching caption sweep to
#       max_tokens=1024. With longer per-call generation, observed KV cache
#       at 9.5% with 6 seqs (1.6%/seq), so 12 seqs lands at ~20% — well
#       within the 24 GB 5090 budget. Caption sweep client uses --workers 16
#       (R5 below) so there's a small queue to keep the GPU saturated.
MAX_NUM_SEQS="${MAX_NUM_SEQS:-12}"
MM_CACHE_GB="${MM_CACHE_GB:-2}"

# `rote_timestamps` is synthesized by a local vLLM patch
# (nix/mlspike-patches/musicflamingo-rote-timestamps.patch, applied by the
# mlspike devshell hook). HF transformers 5.7.0+ doesn't emit it; the patch
# backports vLLM PR #39011's `_build_audio_timestamps` so vLLM derives it
# from feature_attention_mask + chunk_counts inside `_call_hf_processor`.
# No extra serve-time flag required.

exec "$VENV/bin/python" -m vllm.entrypoints.openai.api_server \
  --model "$MODEL" \
  --dtype bfloat16 \
  --max-model-len 4096 \
  --max-num-seqs "$MAX_NUM_SEQS" \
  --gpu-memory-utilization 0.92 \
  --limit-mm-per-prompt '{"audio": 4, "image": 0, "video": 0}' \
  --mm-processor-cache-gb "$MM_CACHE_GB" \
  --trust-remote-code \
  --host 0.0.0.0 \
  --port "$PORT"
