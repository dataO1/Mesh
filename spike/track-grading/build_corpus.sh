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
set -euo pipefail

# zlib path needed by the spike venv (Nix devshell sandbox).
export LD_LIBRARY_PATH="/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:${LD_LIBRARY_PATH:-}"
# Force unbuffered stdout/stderr so progress prints flush through `tee`
# in real time (otherwise Python block-buffers the pipe and you see
# nothing for minutes at a time).
export PYTHONUNBUFFERED=1
PY="$HOME/.cache/mesh-spike/vllm-env/bin/python -u"

OUT_DIR="/tmp/track-grading"
LOG_DIR="$OUT_DIR/logs"
mkdir -p "$LOG_DIR"

DEEZER_DIR="$OUT_DIR/deezer"
AUDIO_DIR="$OUT_DIR/audio"

banner() {
  printf '\n\033[1;36m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n'
  printf '\033[1;36m %s\033[0m\n' "$1"
  printf '\033[1;36m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n\n'
}

# Phase 0: ensure the seed list exists.
if [ ! -f "$OUT_DIR/everynoise_dj_genres.json" ]; then
  banner "Phase 0/3 — scraping everynoise.com → DJ-relevant genre list"
  "$PY" spike/track-grading/scrape_everynoise.py
  "$PY" spike/track-grading/categorize_genres.py
fi

# Phase 1: hit Deezer API → metadata + preview URLs.
banner "Phase 1/3 — Deezer search + radio (~9 min, rate-gated)"
echo "  log file: $LOG_DIR/fetch.log"
echo "  output:   $DEEZER_DIR/corpus_tracks.json"
echo
"$PY" spike/track-grading/fetch_deezer_tracks.py "$@" 2>&1 | tee "$LOG_DIR/fetch.log"

if [ ! -s "$DEEZER_DIR/corpus_tracks.json" ]; then
  echo "[ERROR] no corpus_tracks.json produced — fetch phase failed"
  exit 1
fi

n_tracks=$("$PY" -c "import json; print(len(json.load(open('$DEEZER_DIR/corpus_tracks.json'))))")
echo
echo "[summary] phase 1 produced $n_tracks unique tracks"

# Phase 2: download preview MP3s in parallel.
banner "Phase 2/3 — downloading $n_tracks preview MP3s (32 workers)"
echo "  log file: $LOG_DIR/download.log"
echo "  output:   $AUDIO_DIR/dz_*.mp3"
echo
"$PY" spike/track-grading/download_previews.py --workers 32 \
  --manifest "$DEEZER_DIR/corpus_tracks.json" \
  --out-dir "$AUDIO_DIR" 2>&1 | tee "$LOG_DIR/download.log"

# Phase 3: summary.
banner "Phase 3/3 — summary"
n_files=$(find "$AUDIO_DIR" -name 'dz_*.mp3' 2>/dev/null | wc -l)
total_bytes=$(du -sb "$AUDIO_DIR" 2>/dev/null | awk '{print $1}')
total_gb=$(awk -v b="$total_bytes" 'BEGIN { printf "%.2f", b / 1024 / 1024 / 1024 }')

echo "  manifest tracks:   $n_tracks"
echo "  downloaded files:  $n_files"
echo "  audio dir size:    ${total_gb} GB  ($AUDIO_DIR)"
echo "  fetch  log:        $LOG_DIR/fetch.log"
echo "  download log:      $LOG_DIR/download.log"
echo
echo "Next: feed $AUDIO_DIR into MuQ-MuLan embedding extraction (round-7)."
