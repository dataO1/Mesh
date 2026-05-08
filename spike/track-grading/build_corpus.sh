#!/usr/bin/env bash
# End-to-end round-7 corpus build: scrape → search Deezer → download
# preview MP3s. Idempotent + resume-safe. Run from repo root.
#
# Usage:
#   bash spike/track-grading/build_corpus.sh
#   bash spike/track-grading/build_corpus.sh --tracks-per-seed 5
#
# Forward any flags to fetch_deezer_tracks.py.
#
# Phase rate-limit posture:
#   - Phase 1 (metadata) hits api.deezer.com at 10 req/s (the script's
#     internal gate). Anonymous limit is ~50/5s; we stay safely under.
#   - Phase 2 (audio) hits cdnt-preview.dzcdn.net which is a Google
#     Frontend CDN — separate from the API, no per-IP rate limits in
#     practice. 32 parallel workers is comfortable; bump higher only if
#     your link can saturate it.
#
# Idempotency:
#   - Phase 0 skips if seed file exists.
#   - Phase 1 skips if manifest exists with ≥ FETCH_SKIP_THRESHOLD tracks
#     (default 0.9 × intended_target). Override with FORCE_FETCH=1.
#   - Phase 2 loops refresh + download until missing-mp3 count drops below
#     ACCEPT_THRESHOLD (default 200) or stalls for a pass.
#   - Deezer preview URLs are time-signed (~30 min TTL), so a single
#     refresh+download cycle on a large delta hits expirations mid-flight.
#     Each loop iteration shrinks the working set, so URL expiration is
#     less likely on subsequent passes — typically converges in 2-4 passes.
set -uo pipefail   # NO -e: download_previews.py exits non-zero on partial
                   # failures (URL TTL races); the convergence loop handles it.

# Pull venv-related env from the mlspike devshell when invoked inside one;
# otherwise discover dynamically (same shape as spike/track-grading/run_r7_step.sh).
PY="$HOME/.cache/mesh-spike/vllm-env/bin/python"
if [ "${MESH_MLSPIKE_ENV:-}" != "1" ]; then
  ZLIB_LIBDIR=$(ls /nix/store/*zlib-1.3*/lib/libz.so.1 2>/dev/null | sort -V | tail -1 | xargs -r dirname)
  STDCPP_LIBDIR=$(ls /nix/store/*gcc-1*-lib/lib/libstdc++.so.6 2>/dev/null | sort -V | tail -1 | xargs -r dirname)
  [ -n "$ZLIB_LIBDIR" ]   && export LD_LIBRARY_PATH="$ZLIB_LIBDIR:${LD_LIBRARY_PATH:-}"
  [ -n "$STDCPP_LIBDIR" ] && export LD_LIBRARY_PATH="$STDCPP_LIBDIR:${LD_LIBRARY_PATH:-}"
fi
export PYTHONUNBUFFERED=1

OUT_DIR="/home/data01/Music/mesh-track-grading"
LOG_DIR="$OUT_DIR/logs"
mkdir -p "$LOG_DIR"

DEEZER_DIR="$OUT_DIR/deezer"
AUDIO_DIR="$OUT_DIR/audio"
MANIFEST="$DEEZER_DIR/corpus_tracks.json"

# Phase 2 convergence knobs
MAX_PASSES="${MAX_PASSES:-8}"
ACCEPT_THRESHOLD="${ACCEPT_THRESHOLD:-200}"   # accept this many delisted/region-locked tracks

banner() {
  printf '\n\033[1;36m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n'
  printf '\033[1;36m %s\033[0m\n' "$1"
  printf '\033[1;36m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n\n'
}

count_missing() {
  "$PY" -c "
import json
from pathlib import Path
m = json.load(open('$MANIFEST'))
audio = Path('$AUDIO_DIR')
print(sum(1 for t in m if not (audio / f\"dz_{t['deezer_track_id']}.mp3\").exists()))
"
}

count_manifest() {
  "$PY" -c "import json; print(len(json.load(open('$MANIFEST'))))" 2>/dev/null || echo 0
}

# ── Phase 0: seed list ───────────────────────────────────────────────
if [ ! -f "$OUT_DIR/everynoise_dj_genres.json" ]; then
  banner "Phase 0 — scraping everynoise.com → DJ-relevant genre list"
  "$PY" spike/track-grading/scrape_everynoise.py
  "$PY" spike/track-grading/categorize_genres.py
fi

# ── Phase 1: Deezer search + radio ───────────────────────────────────
n_seeds=$("$PY" -c "import json; print(len(json.load(open('$OUT_DIR/everynoise_dj_genres.json'))))" 2>/dev/null || echo 2109)
target_tracks=$((n_seeds * 10))   # default tracks-per-seed=10
fetch_skip_thresh=$(( target_tracks * 9 / 10 ))

n_existing=$(count_manifest)
if [ "${FORCE_FETCH:-0}" != "1" ] && [ "$n_existing" -ge "$fetch_skip_thresh" ]; then
  banner "Phase 1 — SKIP (manifest has $n_existing tracks ≥ $fetch_skip_thresh threshold)"
  echo "  override with FORCE_FETCH=1 to re-fetch"
  echo
  n_tracks="$n_existing"
else
  banner "Phase 1 — Deezer search + radio (~12 min, rate-gated)"
  echo "  log file: $LOG_DIR/fetch.log"
  echo "  output:   $MANIFEST"
  echo
  "$PY" spike/track-grading/fetch_deezer_tracks.py "$@" 2>&1 | tee "$LOG_DIR/fetch.log"
  if [ ! -s "$MANIFEST" ]; then
    echo "[ERROR] no corpus_tracks.json produced — fetch phase failed"
    exit 1
  fi
  n_tracks=$(count_manifest)
  echo
  echo "[summary] phase 1 produced $n_tracks unique tracks"
fi

# ── Phase 2: refresh-and-download convergence loop ───────────────────
banner "Phase 2 — refresh + download convergence loop (max $MAX_PASSES passes, accept ≤$ACCEPT_THRESHOLD missing)"
echo "  refresh log: $LOG_DIR/refresh.log"
echo "  download log: $LOG_DIR/download.log"
echo

prev_missing=-1
for pass in $(seq 1 "$MAX_PASSES"); do
  missing=$(count_missing)
  echo "──────────────────────────────────────────────────────"
  echo " pass $pass  —  missing mp3s: $missing  (manifest: $n_tracks)"
  echo "──────────────────────────────────────────────────────"

  if [ "$missing" -le "$ACCEPT_THRESHOLD" ]; then
    echo "[converge] missing $missing ≤ $ACCEPT_THRESHOLD — accepting"
    break
  fi
  if [ "$missing" = "$prev_missing" ]; then
    echo "[converge] no progress this pass (still $missing missing)"
    echo "  these are likely Deezer-delisted or region-locked tracks; stopping"
    break
  fi
  prev_missing="$missing"

  echo "[pass $pass] refresh → download (URLs ~30 min TTL; smaller deltas converge fast)"
  "$PY" spike/track-grading/refresh_preview_urls.py \
    --manifest "$MANIFEST" \
    --audio-dir "$AUDIO_DIR" 2>&1 | tee -a "$LOG_DIR/refresh.log" || true
  "$PY" spike/track-grading/download_previews.py --workers 32 \
    --manifest "$MANIFEST" \
    --out-dir "$AUDIO_DIR" 2>&1 | tee -a "$LOG_DIR/download.log" || true
done

# ── Phase 3: summary ─────────────────────────────────────────────────
banner "Phase 3 — summary"
n_files=$(find "$AUDIO_DIR" -name 'dz_*.mp3' 2>/dev/null | wc -l)
total_bytes=$(du -sb "$AUDIO_DIR" 2>/dev/null | awk '{print $1}')
total_gb=$(awk -v b="$total_bytes" 'BEGIN { printf "%.2f", b / 1024 / 1024 / 1024 }')

echo "  manifest tracks:   $n_tracks"
echo "  downloaded files:  $n_files"
echo "  missing:           $((n_tracks - n_files))"
echo "  audio dir size:    ${total_gb} GB  ($AUDIO_DIR)"
echo "  fetch  log:        $LOG_DIR/fetch.log"
echo "  refresh log:       $LOG_DIR/refresh.log"
echo "  download log:      $LOG_DIR/download.log"
echo
echo "Next: feed $AUDIO_DIR into the MF caption sweep."
