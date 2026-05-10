#!/usr/bin/env bash
# Round-7.7 Phase-1b orchestration: add new juror(s) to the consensus,
# re-aggregate Dawid-Skene, measure drift, write report. Idempotent — safe
# to re-run; resumes if any step was partial.
#
# Supports running Gemini and DeepSeek jurors in parallel (no cross-
# contamination — each writes to its own NPZ, each uses its own API quota).
# At least one of GEMINI_API_KEY / DEEPSEEK_API_KEY must be set.
#
# Pre-requisite: at least one API key exported in the environment.
#
# Usage:
#   # Gemini only (current Phase 1b default)
#   GEMINI_API_KEY=... bash spike/track-grading/run_phase1b.sh
#
#   # DeepSeek only (Phase 1c-i C1, after Gemini coverage complete)
#   DEEPSEEK_API_KEY=... bash spike/track-grading/run_phase1b.sh
#
#   # Both in parallel — most efficient if both quotas are available
#   GEMINI_API_KEY=... DEEPSEEK_API_KEY=... bash spike/track-grading/run_phase1b.sh
#
# Env overrides (optional):
#   GEMINI_MODEL          default gemini-3-flash-preview
#   GEMINI_CONCURRENCY    default 50; lower (e.g. 20) on preview if 429s persist
#   DEEPSEEK_MODEL        default deepseek-v4-pro
#   DEEPSEEK_URL          default https://api.deepseek.com/v1/chat/completions
#   DEEPSEEK_WORKERS      default 16 (existing caption_intensity_rating uses
#                         ThreadPoolExecutor, not asyncio)
#   PHASE1B_LIMIT         cap pending captions for smoke test (e.g. 100)
#   PHASE1B_SKIP_INSTALL  set to "1" to skip pip install check
#
# What it does:
#   1. Ensure google-genai installed (only if Gemini juror requested).
#   2. Snapshot current 3-juror consensus → baseline (idempotent).
#   3. Launch each requested juror as a background process. Each writes to
#      a distinct NPZ; both use atomic-rename + resume safety so concurrent
#      runs don't lose data.
#   4. Wait for all launched jurors to finish.
#   5. Re-aggregate Dawid-Skene over all available NPZs (3 baseline + N new).
#   6. Measure drift: 3-juror baseline vs N-juror updated → vault MD report.
#   7. Print verdict.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

VENV="$HOME/.cache/mesh-spike/vllm-env"
VENV_PY="$VENV/bin/python"
VENV_PIP="$VENV/bin/pip"

# Post-juror stages bypass the run_r7_step.sh shim because its zlib auto-
# detection occasionally picks a 32-bit libz from /nix/store and breaks
# numpy. Pin a known-good 64-bit zlib + the latest gcc-lib explicitly.
POST_LIBZ="/nix/store/2kdz3m7ic8w226pcvkz1dlg169v91p6a-zlib-1.3.2/lib"
POST_STDCPP="$(ls /nix/store/*gcc-1*-lib/lib/libstdc++.so.6 2>/dev/null | sort -V | tail -1 | xargs -r dirname)"
POST_LD="$POST_LIBZ:$POST_STDCPP"

GRADE_DIR="/home/data01/Music/mesh-track-grading"
CAPTIONS_ROOT="$GRADE_DIR/round7_6_captions/music_flamingo"
JUROR_NEMOTRON="$GRADE_DIR/round7_6_caption_intensity_nemotron.npz"
JUROR_QWEN="$GRADE_DIR/round7_6_caption_intensity_qwen36.npz"
JUROR_MINSTRAL="$GRADE_DIR/round7_6_caption_intensity_local_minstral.npz"
JUROR_GEMINI="$GRADE_DIR/round7_6_caption_intensity_gemini_flash.npz"
JUROR_DEEPSEEK="$GRADE_DIR/round7_6_caption_intensity_deepseek_v4_pro.npz"
CONSENSUS_PROD="$GRADE_DIR/round7_6_consensus.npz"
CONSENSUS_BASELINE="$GRADE_DIR/round7_6_consensus_3juror_baseline.npz"
CONSENSUS_UPDATED="$GRADE_DIR/round7_6_consensus_updated.npz"

GEMINI_MODEL="${GEMINI_MODEL:-gemini-3-flash-preview}"
GEMINI_CONCURRENCY="${GEMINI_CONCURRENCY:-50}"
DEEPSEEK_MODEL="${DEEPSEEK_MODEL:-deepseek-v4-pro}"
DEEPSEEK_URL="${DEEPSEEK_URL:-https://api.deepseek.com/v1/chat/completions}"
DEEPSEEK_WORKERS="${DEEPSEEK_WORKERS:-16}"
DRIFT_REPORT="/home/data01/Notes/🗂️ Collection/Mesh — N-Juror Consensus Drift Report.md"

LOG_DIR="$GRADE_DIR/phase1b_logs"
mkdir -p "$LOG_DIR"

log() { printf '[phase1b] %s\n' "$*" >&2; }
fail() { printf '[phase1b] ERROR: %s\n' "$*" >&2; exit 1; }

# ─── 0. Pre-flight checks ────────────────────────────────────────────────────
RUN_GEMINI=0
RUN_DEEPSEEK=0
[[ -n "${GEMINI_API_KEY:-}" ]] && RUN_GEMINI=1
[[ -n "${DEEPSEEK_API_KEY:-}" ]] && RUN_DEEPSEEK=1
if [[ $RUN_GEMINI -eq 0 && $RUN_DEEPSEEK -eq 0 ]]; then
  fail "neither GEMINI_API_KEY nor DEEPSEEK_API_KEY set in environment"
fi

[[ -x "$VENV_PY" ]] || fail "spike venv python not found at $VENV_PY"
[[ -d "$CAPTIONS_ROOT" ]] || fail "captions root not found: $CAPTIONS_ROOT"
for j in "$JUROR_NEMOTRON" "$JUROR_QWEN" "$JUROR_MINSTRAL"; do
  [[ -f "$j" ]] || fail "existing juror NPZ missing: $j"
done
[[ -f "$CONSENSUS_PROD" ]] || fail "production 3-juror consensus missing: $CONSENSUS_PROD"

log "jurors enabled: $([[ $RUN_GEMINI -eq 1 ]] && echo -n 'gemini ')$([[ $RUN_DEEPSEEK -eq 1 ]] && echo -n 'deepseek')"

# ─── 1. Install google-genai if Gemini requested and missing ─────────────────
if [[ $RUN_GEMINI -eq 1 && "${PHASE1B_SKIP_INSTALL:-0}" != "1" ]]; then
  if ! "$VENV_PY" -c "import google.genai" 2>/dev/null; then
    log "google-genai missing — installing into spike venv"
    "$VENV_PIP" install --quiet google-genai
    "$VENV_PY" -c "import google.genai; print('google-genai', google.genai.__version__)" \
      || fail "google-genai install failed"
  fi
fi

# ─── 2. Snapshot the 3-juror baseline (idempotent) ───────────────────────────
if [[ -f "$CONSENSUS_BASELINE" ]]; then
  log "baseline snapshot already exists: $CONSENSUS_BASELINE (keeping it)"
else
  log "snapshotting current 3-juror consensus → $CONSENSUS_BASELINE"
  cp -p "$CONSENSUS_PROD" "$CONSENSUS_BASELINE"
fi

# ─── 3. Launch jurors in parallel (each in background) ───────────────────────
GEMINI_PID=
DEEPSEEK_PID=
LIMIT_ARG=()
[[ -n "${PHASE1B_LIMIT:-}" ]] && {
  LIMIT_ARG=(--limit "$PHASE1B_LIMIT")
  log "smoke-test limit: $PHASE1B_LIMIT pending captions per juror"
}

if [[ $RUN_GEMINI -eq 1 ]]; then
  GEMINI_LOG="$LOG_DIR/gemini_$(date +%Y%m%d_%H%M%S).log"
  log "launching Gemini juror (model=$GEMINI_MODEL, concurrency=$GEMINI_CONCURRENCY)"
  log "  log: $GEMINI_LOG"
  bash spike/track-grading/run_r7_step.sh \
    caption_intensity_rating_gemini.py \
    --captions-root "$CAPTIONS_ROOT" \
    --out "$JUROR_GEMINI" \
    --model "$GEMINI_MODEL" \
    --concurrency "$GEMINI_CONCURRENCY" \
    "${LIMIT_ARG[@]}" \
    > "$GEMINI_LOG" 2>&1 &
  GEMINI_PID=$!
  log "  Gemini PID=$GEMINI_PID"
fi

if [[ $RUN_DEEPSEEK -eq 1 ]]; then
  DEEPSEEK_LOG="$LOG_DIR/deepseek_$(date +%Y%m%d_%H%M%S).log"
  log "launching DeepSeek juror (model=$DEEPSEEK_MODEL, workers=$DEEPSEEK_WORKERS)"
  log "  url: $DEEPSEEK_URL"
  log "  log: $DEEPSEEK_LOG"
  # Reuses caption_intensity_rating.py (OpenAI-compatible HTTP client) since
  # DeepSeek exposes a chat-completions endpoint with the same wire format
  # as the existing vLLM jurors. --no-health-check skips the /health probe
  # the existing script does for local vLLM (DeepSeek doesn't expose one).
  TEXT_LLM_API_KEY="$DEEPSEEK_API_KEY" \
  bash spike/track-grading/run_r7_step.sh \
    caption_intensity_rating.py \
    --captions-root "$CAPTIONS_ROOT" \
    --out "$JUROR_DEEPSEEK" \
    --url "$DEEPSEEK_URL" \
    --model "$DEEPSEEK_MODEL" \
    --workers "$DEEPSEEK_WORKERS" \
    --no-health-check \
    > "$DEEPSEEK_LOG" 2>&1 &
  DEEPSEEK_PID=$!
  log "  DeepSeek PID=$DEEPSEEK_PID"
fi

log "waiting for jurors to finish (tail the per-juror logs to monitor progress)"
GEMINI_RC=0
DEEPSEEK_RC=0
[[ -n "$GEMINI_PID" ]] && { wait "$GEMINI_PID" || GEMINI_RC=$?; }
[[ -n "$DEEPSEEK_PID" ]] && { wait "$DEEPSEEK_PID" || DEEPSEEK_RC=$?; }

if [[ -n "$GEMINI_PID" ]]; then
  if [[ $GEMINI_RC -eq 0 ]]; then
    log "Gemini juror finished cleanly"
  else
    log "Gemini juror exited with rc=$GEMINI_RC — see $GEMINI_LOG (NPZ may still be partial-but-resumable)"
  fi
fi
if [[ -n "$DEEPSEEK_PID" ]]; then
  if [[ $DEEPSEEK_RC -eq 0 ]]; then
    log "DeepSeek juror finished cleanly"
  else
    log "DeepSeek juror exited with rc=$DEEPSEEK_RC — see $DEEPSEEK_LOG (NPZ may still be partial-but-resumable)"
  fi
fi

# ─── 4. Re-aggregate Dawid-Skene with all available jurors ───────────────────
AGG_ARGS=(
  --cap-intensity "$JUROR_NEMOTRON"
  --cap-intensity "$JUROR_QWEN"
  --cap-intensity "$JUROR_MINSTRAL"
)
NEW_JUROR_FLAGS=()
N_NEW=0
if [[ -f "$JUROR_GEMINI" ]]; then
  AGG_ARGS+=(--cap-intensity "$JUROR_GEMINI")
  NEW_JUROR_FLAGS+=(--new-juror caption_text_llm_gemini_flash)
  N_NEW=$((N_NEW+1))
fi
if [[ -f "$JUROR_DEEPSEEK" ]]; then
  AGG_ARGS+=(--cap-intensity "$JUROR_DEEPSEEK")
  NEW_JUROR_FLAGS+=(--new-juror caption_text_llm_deepseek_v4_pro)
  N_NEW=$((N_NEW+1))
fi
if [[ $N_NEW -eq 0 ]]; then
  fail "no new-juror NPZ produced — both jurors failed?"
fi
TOTAL_JURORS=$((3 + N_NEW))
log "re-aggregating Dawid-Skene with $TOTAL_JURORS jurors ($N_NEW new)"

LD_LIBRARY_PATH="$POST_LD" "$VENV_PY" spike/track-grading/aggregate_consensus.py \
  "${AGG_ARGS[@]}" \
  --out "$CONSENSUS_UPDATED"

[[ -f "$CONSENSUS_UPDATED" ]] || fail "$TOTAL_JURORS-juror consensus not produced: $CONSENSUS_UPDATED"

# ─── 5. Drift measurement ────────────────────────────────────────────────────
log "measuring drift (3-juror baseline vs $TOTAL_JURORS-juror updated)"
LD_LIBRARY_PATH="$POST_LD" "$VENV_PY" spike/track-grading/measure_consensus_drift.py \
  --baseline "$CONSENSUS_BASELINE" \
  --updated "$CONSENSUS_UPDATED" \
  "${NEW_JUROR_FLAGS[@]}" \
  --captions-root "$CAPTIONS_ROOT" \
  --out "$DRIFT_REPORT"

log "drift report: $DRIFT_REPORT"
log "Phase 1b orchestration complete."
