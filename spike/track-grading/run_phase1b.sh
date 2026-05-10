#!/usr/bin/env bash
# Round-7.7 Phase-1b orchestration: add Gemini Flash as 4th juror, re-aggregate,
# measure drift, write report. Idempotent — safe to re-run; resumes if any step
# was partial.
#
# Pre-requisite: GEMINI_API_KEY exported in the environment.
#
# Usage:
#   GEMINI_API_KEY=... bash spike/track-grading/run_phase1b.sh
#
# Env overrides (optional):
#   GEMINI_MODEL        default gemini-3-flash-preview (preview tier)
#                       fallback options: gemini-2.5-flash (stable),
#                       gemini-3.1-flash-lite (smaller, cheaper)
#   GEMINI_CONCURRENCY  default 50; lower (e.g. 20) on preview if 429s persist
#   PHASE1B_LIMIT       cap pending captions for smoke test (e.g. 100)
#   PHASE1B_SKIP_INSTALL   set to "1" to skip pip install check
#
# What it does, step by step:
#   1. Ensure google-genai is installed in the spike venv.
#   2. Snapshot the current 3-juror consensus to a baseline file (idempotent).
#   3. Run Gemini Flash on all MF captions → caption_intensity_gemini_flash.npz
#      (resumes if NPZ already partial).
#   4. Re-aggregate Dawid-Skene with 4 jurors → consensus_4juror.npz
#   5. Measure drift: 3-juror baseline vs 4-juror updated → vault MD report
#   6. Print the FRAGILE/ROBUST verdict.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

VENV="$HOME/.cache/mesh-spike/vllm-env"
VENV_PY="$VENV/bin/python"
VENV_PIP="$VENV/bin/pip"

# Post-juror stages (aggregate_consensus, measure_consensus_drift) bypass the
# `run_r7_step.sh` shim because its zlib auto-detection occasionally picks up
# a 32-bit libz from /nix/store and breaks numpy. We pin a known-good 64-bit
# zlib + the latest gcc-lib for libstdc++ explicitly. Verified 2026-05-10.
POST_LIBZ="/nix/store/2kdz3m7ic8w226pcvkz1dlg169v91p6a-zlib-1.3.2/lib"
POST_STDCPP="$(ls /nix/store/*gcc-1*-lib/lib/libstdc++.so.6 2>/dev/null | sort -V | tail -1 | xargs -r dirname)"
POST_LD="$POST_LIBZ:$POST_STDCPP"

GRADE_DIR="/home/data01/Music/mesh-track-grading"
CAPTIONS_ROOT="$GRADE_DIR/round7_6_captions/music_flamingo"
JUROR_NEMOTRON="$GRADE_DIR/round7_6_caption_intensity_nemotron.npz"
JUROR_QWEN="$GRADE_DIR/round7_6_caption_intensity_qwen36.npz"
JUROR_MINSTRAL="$GRADE_DIR/round7_6_caption_intensity_local_minstral.npz"
JUROR_GEMINI="$GRADE_DIR/round7_6_caption_intensity_gemini_flash.npz"
CONSENSUS_PROD="$GRADE_DIR/round7_6_consensus.npz"
CONSENSUS_BASELINE="$GRADE_DIR/round7_6_consensus_3juror_baseline.npz"
CONSENSUS_4JUROR="$GRADE_DIR/round7_6_consensus_4juror.npz"

GEMINI_MODEL="${GEMINI_MODEL:-gemini-3-flash-preview}"
GEMINI_CONCURRENCY="${GEMINI_CONCURRENCY:-50}"
DRIFT_REPORT="/home/data01/Notes/🗂️ Collection/Mesh — 3 vs 4 Juror Drift Report.md"

log() { printf '[phase1b] %s\n' "$*" >&2; }
fail() { printf '[phase1b] ERROR: %s\n' "$*" >&2; exit 1; }

# ─── 0. Pre-flight checks ────────────────────────────────────────────────────
[[ -n "${GEMINI_API_KEY:-}" ]] || fail "GEMINI_API_KEY not set in environment"
[[ -x "$VENV_PY" ]] || fail "spike venv python not found at $VENV_PY"
[[ -d "$CAPTIONS_ROOT" ]] || fail "captions root not found: $CAPTIONS_ROOT"
for j in "$JUROR_NEMOTRON" "$JUROR_QWEN" "$JUROR_MINSTRAL"; do
  [[ -f "$j" ]] || fail "existing juror NPZ missing: $j"
done
[[ -f "$CONSENSUS_PROD" ]] || fail "production 3-juror consensus missing: $CONSENSUS_PROD"

# ─── 1. Install google-genai if missing ──────────────────────────────────────
if [[ "${PHASE1B_SKIP_INSTALL:-0}" != "1" ]]; then
  if ! "$VENV_PY" -c "import google.genai" 2>/dev/null; then
    log "google-genai missing — installing into spike venv"
    "$VENV_PIP" install --quiet google-genai
    "$VENV_PY" -c "import google.genai; print('google-genai', google.genai.__version__)" \
      || fail "google-genai install failed"
  else
    "$VENV_PY" -c "import google.genai; print('[phase1b] google-genai', google.genai.__version__)"
  fi
fi

# ─── 2. Snapshot the 3-juror baseline (idempotent) ───────────────────────────
if [[ -f "$CONSENSUS_BASELINE" ]]; then
  log "baseline snapshot already exists: $CONSENSUS_BASELINE (keeping it)"
else
  log "snapshotting current 3-juror consensus → $CONSENSUS_BASELINE"
  cp -p "$CONSENSUS_PROD" "$CONSENSUS_BASELINE"
fi

# ─── 3. Run the Gemini Flash juror (resumes if partial) ──────────────────────
log "running Gemini juror (model=$GEMINI_MODEL, concurrency=$GEMINI_CONCURRENCY)"
GEMINI_ARGS=(
  --captions-root "$CAPTIONS_ROOT"
  --out "$JUROR_GEMINI"
  --model "$GEMINI_MODEL"
  --concurrency "$GEMINI_CONCURRENCY"
)
if [[ -n "${PHASE1B_LIMIT:-}" ]]; then
  log "smoke-test limit: $PHASE1B_LIMIT pending captions"
  GEMINI_ARGS+=(--limit "$PHASE1B_LIMIT")
fi
bash spike/track-grading/run_r7_step.sh \
  caption_intensity_rating_gemini.py "${GEMINI_ARGS[@]}"

[[ -f "$JUROR_GEMINI" ]] || fail "Gemini juror NPZ not produced: $JUROR_GEMINI"

# ─── 4. Re-aggregate with 4 jurors ───────────────────────────────────────────
log "re-aggregating Dawid-Skene with 4 jurors"
LD_LIBRARY_PATH="$POST_LD" "$VENV_PY" spike/track-grading/aggregate_consensus.py \
  --cap-intensity "$JUROR_NEMOTRON" \
  --cap-intensity "$JUROR_QWEN" \
  --cap-intensity "$JUROR_MINSTRAL" \
  --cap-intensity "$JUROR_GEMINI" \
  --out "$CONSENSUS_4JUROR"

[[ -f "$CONSENSUS_4JUROR" ]] || fail "4-juror consensus not produced: $CONSENSUS_4JUROR"

# ─── 5. Drift measurement ────────────────────────────────────────────────────
log "measuring drift (baseline vs 4-juror updated)"
LD_LIBRARY_PATH="$POST_LD" "$VENV_PY" spike/track-grading/measure_consensus_drift.py \
  --baseline "$CONSENSUS_BASELINE" \
  --updated "$CONSENSUS_4JUROR" \
  --new-juror "caption_text_llm_gemini_flash" \
  --captions-root "$CAPTIONS_ROOT" \
  --out "$DRIFT_REPORT"

log "drift report written to: $DRIFT_REPORT"
log "Phase 1b complete — read the report and execute the FRAGILE / ROBUST branch."
