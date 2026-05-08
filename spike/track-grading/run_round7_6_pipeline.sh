#!/usr/bin/env bash
# Round-7.6 end-to-end pipeline orchestrator.
#
# Stages:
#   0. Pre-flight: vLLM Music Flamingo serve + smoke test (~1 min)
#   1. Tournament: re-judge selected pairs with Music Flamingo (long run)
#   2. BT priors per axis from MF rankings
#   3. Multi-task linear probe training (PyTorch, mini-batched per fix from r7.5)
#   4. ListMLE blend (target: timbre_roughness)
#   5. Axis interpretation + cross-library projection
#   6. V18 export (NOT auto-deployed)
#   7. V15 vs V17b vs V18 comparison
#
# Usage:
#   1) Start vLLM serve in another terminal:
#        bash spike/track-grading/serve_music_flamingo.sh
#   2) Wait until /health returns 200 (~3 min cold start, ~14 GB download
#      first time)
#   3) Run smoke (validates env + roughly estimates throughput):
#        bash spike/track-grading/run_round7_6_pipeline.sh smoke
#   4) Run production tournament + downstream (recommended path):
#        bash spike/track-grading/run_round7_6_pipeline.sh full
#      OR run only the post-tournament stages on existing MF data:
#        bash spike/track-grading/run_round7_6_pipeline.sh post
#
# Throughput note: bf16 Music Flamingo on RTX 5090 Mobile sustains
# ~1.0-1.4 K=4 calls/sec. For 20k uncertain pairs (default): ~5 hours.
# For full 192k re-judge: ~40 hours. Smoke is ~50 calls in ~1 min.
set -euo pipefail

cd "$(dirname "$0")/../.."
RUN="bash spike/track-grading/run_r7_step.sh"

STAGE="${1:-help}"

# ─── stages ─────────────────────────────────────────────────────────
# CURRENT (preferred path, plays to MF's strongest mode):
#   caption-smoke / caption-full → MF generates rich captions → embed
#                                  with bge-base → train probe with
#                                  audio + caption_emb features.
#
# DEPRECATED (kept for reproducibility only):
#   pointwise-smoke / pointwise-full → forces MF into uncalibrated
#       0-100 (raw-int) or 5-bucket logprob (likert) rating. The
#       likert variant works but caps at narrow stds on subjective
#       axes; captions are MF's #1 trained task and should yield
#       richer signal. See documents/ML notes for details.
case "$STAGE" in
  caption-smoke)
    if ! curl -sf http://localhost:8001/health -o /dev/null; then
      echo "ERROR: Music Flamingo vLLM serve not responding at :8001" >&2
      exit 1
    fi
    echo "=== caption smoke (200 tracks, ~5-15 min depending on caption length) ==="
    $RUN run_judge_caption.py \
      --tracks-subset smoke-200 \
      --max-tokens 1024 \
      --temperature 0.7 \
      --top-p 0.9 \
      --workers 32

    echo
    echo "=== embedding captions with bge-base-en-v1.5 ==="
    $RUN embed_captions.py \
      --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_caption_emb_smoke.npz

    echo
    echo "=== probe transfer test (5-fold ridge CV) ==="
    $RUN train_probe_caption_smoke.py \
      --caption-emb /home/data01/Music/mesh-track-grading/round7_6_caption_emb_smoke.npz

    echo
    echo "Phase 1 gate: see 'gate criteria' block above."
    echo "If passed, run 'bash $0 caption-full' for the 15314-track sweep."
    exit 0
    ;;

  caption-full)
    if ! curl -sf http://localhost:8001/health -o /dev/null; then
      echo "ERROR: Music Flamingo vLLM serve not responding at :8001" >&2
      exit 1
    fi
    echo "=== caption full corpus (~40k tracks, ~9-11 hr at ~1.2-1.4 c/s with 1024-tok captions, 32 workers) ==="
    $RUN run_judge_caption.py \
      --tracks-subset all \
      --max-tokens 1024 \
      --temperature 0.7 \
      --top-p 0.9 \
      --workers 32

    echo
    echo "=== embedding captions ==="
    $RUN embed_captions.py \
      --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_caption_emb.npz

    echo
    echo "=== extracting structured caption tags (S3) ==="
    $RUN extract_caption_tags.py \
      --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_caption_struct.npz

    echo
    echo "Captions + features ready. To run V18 training:"
    echo "  1) stop the Music Flamingo serve (or it'll OOM with text-LLM)"
    echo "  2) bash spike/track-grading/serve_text_llm.sh   (port 8002)"
    echo "  3) bash $0 v18-train"
    exit 0
    ;;

  caption-rate)
    # S4 — caption→text-LLM intensity rating.
    # Default: local vLLM serve at :8002 (bash spike/track-grading/serve_text_llm.sh)
    # Remote: set TEXT_LLM_URL, TEXT_LLM_MODEL, TEXT_LLM_API_KEY env vars.
    #   Example for a Spark-hosted Qwen3 endpoint:
    #     TEXT_LLM_URL=https://spark.local:8000/v1/chat/completions
    #     TEXT_LLM_HEALTH_URL=https://spark.local:8000/health   # or skip via --no-health-check
    #     TEXT_LLM_MODEL=Qwen/Qwen3-32B-Instruct                # or whatever's served
    #     TEXT_LLM_API_KEY=...
    REMOTE_FLAGS=""
    # Default workers: 24 for local vLLM (3-7B model fits well at higher
    # concurrency), 16 for remote endpoints (per the 2026-05-07 throughput
    # bench — beyond 16 the server queues without tput gain). Override via
    # TEXT_LLM_WORKERS.
    if [ -n "${TEXT_LLM_URL:-}" ]; then
      WORKERS="${TEXT_LLM_WORKERS:-16}"
    else
      WORKERS="${TEXT_LLM_WORKERS:-24}"
    fi
    if [ -n "${TEXT_LLM_URL:-}" ]; then
      echo "[caption-rate] using remote text-LLM at: $TEXT_LLM_URL"
      echo "[caption-rate] model: ${TEXT_LLM_MODEL:-<env default>}"
      echo "[caption-rate] workers: $WORKERS"
      # Probe health if a health URL is set; skip otherwise.
      if [ -n "${TEXT_LLM_HEALTH_URL:-}" ]; then
        if ! curl -sf "$TEXT_LLM_HEALTH_URL" -o /dev/null; then
          echo "[caption-rate] WARNING: $TEXT_LLM_HEALTH_URL not 200 — proceeding with --no-health-check"
          REMOTE_FLAGS="--no-health-check"
        fi
      else
        REMOTE_FLAGS="--no-health-check"
      fi
      # Qwen3-style models reason before answering; suppress thinking so
      # the answer comes immediately. Set TEXT_LLM_NO_THINK=0 to disable.
      if [ "${TEXT_LLM_NO_THINK:-1}" = "1" ]; then
        REMOTE_FLAGS="$REMOTE_FLAGS --no-think"
      fi
    else
      if ! curl -sf http://localhost:8002/health -o /dev/null; then
        echo "ERROR: text-LLM serve not responding at :8002" >&2
        echo "  Start it first: bash spike/track-grading/serve_text_llm.sh" >&2
        echo "  OR export TEXT_LLM_URL=<remote endpoint> to use a remote model" >&2
        exit 1
      fi
    fi
    # Output filename gets a stable stem so multiple LLM sources don't
    # collide. If TEXT_LLM_TAG is set (e.g., "nemotron", "local_3b"), the
    # output goes to round7_6_caption_intensity_<tag>.npz; otherwise the
    # legacy round7_6_caption_intensity.npz is used.
    OUT_PATH="/home/data01/Music/mesh-track-grading/round7_6_caption_intensity${TEXT_LLM_TAG:+_$TEXT_LLM_TAG}.npz"
    echo "=== S4: caption → text-LLM intensity rating → $OUT_PATH ==="
    $RUN caption_intensity_rating.py \
      --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \
      --out "$OUT_PATH" \
      --workers "$WORKERS" \
      $REMOTE_FLAGS
    exit 0
    ;;

  caption-rate-streaming)
    # Polls the captions dir + reruns caption-rate every $POLL_SECS until
    # captions stop arriving for $STABLE_SECS in a row. Designed to run
    # alongside `caption-full` in another terminal: as MF writes new
    # captions, this rates them on the remote text-LLM. Total wall is
    # bounded by caption-full (since the rater is much faster).
    POLL_SECS="${POLL_SECS:-300}"        # 5 minutes
    STABLE_SECS="${STABLE_SECS:-1200}"   # 20 minutes of no new captions = done
    OUT_PATH="/home/data01/Music/mesh-track-grading/round7_6_caption_intensity${TEXT_LLM_TAG:+_$TEXT_LLM_TAG}.npz"
    CAPS_DIR="/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo"
    REMOTE_FLAGS=""
    if [ -n "${TEXT_LLM_URL:-}" ]; then
      WORKERS="${TEXT_LLM_WORKERS:-128}"
      REMOTE_FLAGS="--no-health-check"
      [ "${TEXT_LLM_NO_THINK:-1}" = "1" ] && REMOTE_FLAGS="$REMOTE_FLAGS --no-think"
    else
      WORKERS="${TEXT_LLM_WORKERS:-24}"
    fi
    echo "[stream-rate] polling $CAPS_DIR every ${POLL_SECS}s, exit when "
    echo "[stream-rate] caption count stable for ${STABLE_SECS}s"
    echo "[stream-rate] writing → $OUT_PATH (resume-safe)"
    last_count=-1
    last_change=$(date +%s)
    while true; do
      if [ -d "$CAPS_DIR" ]; then
        cur=$(ls "$CAPS_DIR"/*.json 2>/dev/null | wc -l)
      else
        cur=0
      fi
      now=$(date +%s)
      if [ "$cur" -gt "$last_count" ]; then
        last_count=$cur
        last_change=$now
      fi
      stable_for=$((now - last_change))
      echo "[stream-rate] $(date +%H:%M:%S) caps=$cur stable_for=${stable_for}s"
      if [ "$cur" -gt 0 ]; then
        $RUN caption_intensity_rating.py \
          --captions-root "$CAPS_DIR" \
          --out "$OUT_PATH" \
          --workers "$WORKERS" \
          $REMOTE_FLAGS || echo "[stream-rate] rater errored; will retry next poll"
      fi
      if [ "$stable_for" -ge "$STABLE_SECS" ] && [ "$cur" -gt 0 ]; then
        echo "[stream-rate] caption count stable for ${stable_for}s; done"
        break
      fi
      sleep "$POLL_SECS"
    done
    exit 0
    ;;

  v18-smoke)
    # Full-pipeline smoke test on the 200-track subset already captioned by
    # `caption-smoke`. Runs every downstream stage end-to-end with smaller
    # K-means K and write to *_smoke.* paths so it doesn't clobber a real
    # production run. Use to verify the pipeline plumbing before committing
    # the 8 hr full caption sweep.
    BASE=/home/data01/Music/mesh-track-grading
    CAPS="$BASE/round7_6_captions/music_flamingo"
    [ -d "$CAPS" ] || { echo "ERROR: $CAPS missing — run caption-smoke first" >&2; exit 1; }
    n_caps=$(ls "$CAPS"/*.json 2>/dev/null | wc -l)
    echo "[v18-smoke] $n_caps captions present"
    if [ "$n_caps" -lt 100 ]; then
      echo "ERROR: only $n_caps captions; need at least 100 for a smoke test" >&2
      exit 1
    fi

    echo "=== S2: re-embed captions to smoke filename ==="
    $RUN embed_captions.py \
      --captions-root "$CAPS" \
      --out "$BASE/round7_6_caption_emb_smoke.npz"

    echo
    echo "=== S3: extract structured tags ==="
    $RUN extract_caption_tags.py \
      --captions-root "$CAPS" \
      --out "$BASE/round7_6_caption_struct_smoke.npz"

    echo
    echo "=== S4: caption → text-LLM intensity ==="
    REMOTE_FLAGS=""
    SMOKE_WORKERS="${TEXT_LLM_WORKERS:-8}"
    if [ -n "${TEXT_LLM_URL:-}" ]; then
      echo "[v18-smoke] using remote text-LLM at: $TEXT_LLM_URL  (workers=$SMOKE_WORKERS)"
      if [ -n "${TEXT_LLM_HEALTH_URL:-}" ]; then
        curl -sf "$TEXT_LLM_HEALTH_URL" -o /dev/null || REMOTE_FLAGS="--no-health-check"
      else
        REMOTE_FLAGS="--no-health-check"
      fi
      if [ "${TEXT_LLM_NO_THINK:-1}" = "1" ]; then
        REMOTE_FLAGS="$REMOTE_FLAGS --no-think"
      fi
    else
      if ! curl -sf http://localhost:8002/health -o /dev/null; then
        echo "ERROR: text-LLM not at :8002 and no TEXT_LLM_URL set" >&2
        exit 1
      fi
    fi
    $RUN caption_intensity_rating.py \
      --captions-root "$CAPS" \
      --out "$BASE/round7_6_caption_intensity_smoke.npz" \
      --workers "$SMOKE_WORKERS" \
      $REMOTE_FLAGS

    echo
    echo "=== S6+S7+S8: build consensus (jury, smoke subset) ==="
    $RUN aggregate_consensus.py \
      --bt-priors "$BASE/round7_5_priors.npz" \
      --bt-tags   "$BASE/round7_5_tags.npz" \
      --cap-intensity "$BASE/round7_6_caption_intensity_smoke.npz" \
      --likert-root  "$BASE/round7_6_likert/music_flamingo" \
      --out "$BASE/round7_6_consensus_smoke.npz"

    echo
    echo "=== S9: artist-stratified split ==="
    $RUN make_split.py \
      --corpus "$BASE/deezer/corpus_tracks.json" \
      --consensus "$BASE/round7_6_consensus_smoke.npz" \
      --out "$BASE/round7_6_split_smoke.npz"

    echo
    echo "=== S10: train teacher (small smoke subset → fast) ==="
    $RUN train_v18_teacher.py \
      --audio-emb     "$BASE/embeddings/corpus_muq_mulan.npz" \
      --caption-emb   "$BASE/round7_6_caption_emb_smoke.npz" \
      --struct-tags   "$BASE/round7_6_caption_struct_smoke.npz" \
      --r75-priors    "$BASE/round7_5_priors.npz" \
      --r75-tags      "$BASE/round7_5_tags.npz" \
      --consensus     "$BASE/round7_6_consensus_smoke.npz" \
      --split         "$BASE/round7_6_split_smoke.npz" \
      --out-dir       "$BASE" \
      --epochs        20 \
      --patience       5

    # Move teacher outputs to smoke filenames so production run is not stomped
    mv "$BASE/round7_6_teacher.pt"          "$BASE/round7_6_teacher_smoke.pt"
    mv "$BASE/round7_6_teacher_metrics.json" "$BASE/round7_6_teacher_metrics_smoke.json"
    mv "$BASE/round7_6_teacher_preds.npz"    "$BASE/round7_6_teacher_preds_smoke.npz"

    echo
    echo "=== S11: distill student ==="
    $RUN distill_v18_student.py \
      --audio-emb      "$BASE/embeddings/corpus_muq_mulan.npz" \
      --teacher-preds  "$BASE/round7_6_teacher_preds_smoke.npz" \
      --consensus      "$BASE/round7_6_consensus_smoke.npz" \
      --out-dir        "$BASE" \
      --epochs         20 \
      --patience        5
    mv "$BASE/round7_6_student.pt"          "$BASE/round7_6_student_smoke.pt"
    mv "$BASE/round7_6_student_metrics.json" "$BASE/round7_6_student_metrics_smoke.json"

    echo
    echo "=== S12: held-out eval (K-means K=5 for smoke test set) ==="
    $RUN eval_v18.py \
      --audio-emb      "$BASE/embeddings/corpus_muq_mulan.npz" \
      --caption-emb    "$BASE/round7_6_caption_emb_smoke.npz" \
      --captions-root  "$CAPS" \
      --teacher-preds  "$BASE/round7_6_teacher_preds_smoke.npz" \
      --student-pt     "$BASE/round7_6_student_smoke.pt" \
      --consensus      "$BASE/round7_6_consensus_smoke.npz" \
      --split          "$BASE/round7_6_split_smoke.npz" \
      --out-dir        "$BASE" \
      --n-clusters     5
    mv "$BASE/round7_6_eval_report.md" "$BASE/round7_6_eval_report_smoke.md"
    mv "$BASE/round7_6_eval.json"      "$BASE/round7_6_eval_smoke.json"

    echo
    echo "=== S13: V18 export (smoke) ==="
    $RUN export_v18.py \
      --student-pt   "$BASE/round7_6_student_smoke.pt" \
      --eval         "$BASE/round7_6_eval_smoke.json" \
      --consensus    "$BASE/round7_6_consensus_smoke.npz" \
      --audio-emb    "$BASE/embeddings/corpus_muq_mulan.npz" \
      --split        "$BASE/round7_6_split_smoke.npz" \
      --out          models/aggression-axes/V18_SMOKE_TEST.json \
      --no-deploy

    echo
    echo "=== v18-smoke END-TO-END SUCCESS ==="
    echo "Artifacts (all *_smoke.*):"
    ls -la "$BASE"/*_smoke* 2>/dev/null
    echo "V18 smoke export: models/aggression-axes/V18_SMOKE_TEST.json"
    exit 0
    ;;

  v18-train)
    # End-to-end teacher → student pipeline, assumes captions + caption-emb +
    # caption-struct + caption-intensity already exist on disk.
    BASE=/home/data01/Music/mesh-track-grading
    for f in \
      "$BASE/embeddings/corpus_muq_mulan.npz" \
      "$BASE/round7_6_caption_emb.npz" \
      "$BASE/round7_6_caption_struct.npz" \
      "$BASE/round7_5_priors.npz" \
      "$BASE/round7_5_tags.npz"; do
      [ -f "$f" ] || { echo "ERROR: missing $f" >&2; exit 1; }
    done
    # Glob-pickup all caption-intensity NPZs (one per text-LLM source).
    # If you ran `caption-rate` once, you'll have round7_6_caption_intensity.npz.
    # If you also ran `caption-rate-streaming` with TEXT_LLM_TAG=nemotron etc.,
    # additional round7_6_caption_intensity_<tag>.npz files exist and become
    # additional jury sources automatically.
    INT_NPZS=()
    for p in "$BASE"/round7_6_caption_intensity*.npz; do
      [ -e "$p" ] || continue
      # Skip the smoke variant unless explicitly requested
      case "$p" in
        *_smoke.npz) continue ;;
      esac
      INT_NPZS+=(--cap-intensity "$p")
    done
    if [ "${#INT_NPZS[@]}" -eq 0 ]; then
      echo "ERROR: no round7_6_caption_intensity*.npz on disk" >&2
      exit 1
    fi
    echo "=== S6+S7+S8: build consensus intensity (Dawid-Skene jury) ==="
    echo "[v18-train] caption-intensity sources:"
    for ((i=1; i<${#INT_NPZS[@]}; i+=2)); do
      echo "  ${INT_NPZS[i]}"
    done
    $RUN aggregate_consensus.py \
      --bt-priors "$BASE/round7_5_priors.npz" \
      --bt-tags   "$BASE/round7_5_tags.npz" \
      "${INT_NPZS[@]}" \
      --likert-root  "$BASE/round7_6_likert/music_flamingo" \
      --out "$BASE/round7_6_consensus.npz"

    echo
    echo "=== S9: artist-stratified split (NOT genre-stratified, per G7) ==="
    $RUN make_split.py \
      --corpus "$BASE/deezer/corpus_tracks.json" \
      --consensus "$BASE/round7_6_consensus.npz" \
      --out "$BASE/round7_6_split.npz"

    echo
    echo "=== S10: train teacher (privileged: audio + caption + struct + r75 tags) ==="
    $RUN train_v18_teacher.py \
      --audio-emb     "$BASE/embeddings/corpus_muq_mulan.npz" \
      --caption-emb   "$BASE/round7_6_caption_emb.npz" \
      --struct-tags   "$BASE/round7_6_caption_struct.npz" \
      --r75-priors    "$BASE/round7_5_priors.npz" \
      --r75-tags      "$BASE/round7_5_tags.npz" \
      --consensus     "$BASE/round7_6_consensus.npz" \
      --split         "$BASE/round7_6_split.npz" \
      --out-dir       "$BASE"

    echo
    echo "=== S11: distill student (linear probe over MuQ-MuLan only) ==="
    $RUN distill_v18_student.py \
      --audio-emb      "$BASE/embeddings/corpus_muq_mulan.npz" \
      --teacher-preds  "$BASE/round7_6_teacher_preds.npz" \
      --consensus      "$BASE/round7_6_consensus.npz" \
      --out-dir        "$BASE"

    echo
    echo "=== S12: held-out eval + caption-emb K-means cluster diagnostic ==="
    $RUN eval_v18.py \
      --audio-emb      "$BASE/embeddings/corpus_muq_mulan.npz" \
      --caption-emb    "$BASE/round7_6_caption_emb.npz" \
      --captions-root  "$BASE/round7_6_captions/music_flamingo" \
      --teacher-preds  "$BASE/round7_6_teacher_preds.npz" \
      --student-pt     "$BASE/round7_6_student.pt" \
      --consensus      "$BASE/round7_6_consensus.npz" \
      --split          "$BASE/round7_6_split.npz" \
      --out-dir        "$BASE"

    echo
    echo "=== S13: V18 export ==="
    $RUN export_v18.py \
      --student-pt   "$BASE/round7_6_student.pt" \
      --eval         "$BASE/round7_6_eval.json" \
      --consensus    "$BASE/round7_6_consensus.npz" \
      --audio-emb    "$BASE/embeddings/corpus_muq_mulan.npz" \
      --split        "$BASE/round7_6_split.npz" \
      --out          models/aggression-axes/V18_round7_6_consensus_distilled.json \
      --no-deploy

    echo
    echo "=== round-7.6 V18 done ==="
    echo "Eval report: $BASE/round7_6_eval_report.md"
    echo "V18 JSON:    models/aggression-axes/V18_round7_6_consensus_distilled.json"
    exit 0
    ;;

  pointwise-smoke)
    echo "[DEPRECATED] pointwise-smoke uses Likert+logprobs. Captions yield" >&2
    echo "[DEPRECATED] richer signal at half the compute. Prefer caption-smoke." >&2
    echo "[DEPRECATED] Continuing in 5s; Ctrl-C to abort." >&2
    sleep 5
    if ! curl -sf http://localhost:8001/health -o /dev/null; then
      echo "ERROR: Music Flamingo vLLM serve not responding at :8001" >&2
      exit 1
    fi
    echo "=== pointwise LIKERT smoke (200 tracks × 16 axes ≈ 3200 calls, ~12 min) ==="
    echo "Output: /home/data01/Music/mesh-track-grading/round7_6_likert/music_flamingo"
    $RUN run_judge_pointwise.py \
      --judge music_flamingo \
      --mode likert \
      --tracks-subset smoke-200 \
      --out-dir /home/data01/Music/mesh-track-grading/round7_6_likert \
      --workers 8
    echo
    echo "=== stacking smoke priors + per-axis distribution audit ==="
    $RUN build_pointwise_priors.py \
      --pairs-root /home/data01/Music/mesh-track-grading/round7_6_likert/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_priors_smoke_likert.npz
    echo
    echo "Win signal: dead axes (dynamic_envelope, melodic_complexity, harmonic_motion)"
    echo "now have std > 10 AND unique > 15 (continuous via logprob softmax)."
    echo "Run 'bash $0 pointwise-full' to commit to the 245k-cell sweep (~13 hr)."
    exit 0
    ;;
  pointwise-full)
    echo "[DEPRECATED] pointwise-full uses Likert+logprobs. See caption-full instead." >&2
    echo "[DEPRECATED] Continuing in 5s; Ctrl-C to abort." >&2
    sleep 5
    if ! curl -sf http://localhost:8001/health -o /dev/null; then
      echo "ERROR: Music Flamingo vLLM serve not responding at :8001" >&2
      exit 1
    fi
    echo "=== pointwise LIKERT full sweep (~245k cells, ~13 hr at 5.2 c/s) ==="
    $RUN run_judge_pointwise.py \
      --judge music_flamingo \
      --mode likert \
      --tracks-subset all \
      --out-dir /home/data01/Music/mesh-track-grading/round7_6_likert \
      --workers 8

    echo "=== stacking full pointwise priors ==="
    $RUN build_pointwise_priors.py \
      --pairs-root /home/data01/Music/mesh-track-grading/round7_6_likert/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_priors.npz

    echo "=== multi-task linear probe ==="
    $RUN train_axes_r7_5.py \
      --priors /home/data01/Music/mesh-track-grading/round7_6_priors.npz \
      --tags /home/data01/Music/mesh-track-grading/round7_5_tags.npz \
      --out-axes /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --out-preds /home/data01/Music/mesh-track-grading/round7_6_predictions.csv \
      --out-metrics /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json

    echo "=== ListMLE blend ==="
    $RUN joint_blend_r7_5.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --priors /home/data01/Music/mesh-track-grading/round7_6_priors.npz \
      --target-axis timbre_roughness \
      --out /home/data01/Music/mesh-track-grading/round7_6_blend.npz

    echo "=== axis interpretation + cross-library ==="
    $RUN interpret_axes_r7_5.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --out /home/data01/Music/mesh-track-grading/round7_6_interpretation.md
    $RUN cross_library_r7.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --blend-file /home/data01/Music/mesh-track-grading/round7_6_blend.npz \
      --out /home/data01/Music/mesh-track-grading/round7_6_cross_library.md

    echo "=== V18 export (NOT auto-deployed) ==="
    $RUN export_axis_r7_5.py \
      --blend /home/data01/Music/mesh-track-grading/round7_6_blend.npz \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --metrics /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json \
      --out models/aggression-axes/V18_round7_6_DEPRECATED_likert.json \
      --no-deploy
    echo
    echo "=== round-7.6 pointwise done ==="
    exit 0
    ;;
esac

# ─── pre-flight checks ──────────────────────────────────────────────
preflight() {
  echo "=== pre-flight ==="
  if ! curl -sf http://localhost:8001/health -o /dev/null; then
    echo "ERROR: Music Flamingo vLLM serve not responding at :8001" >&2
    echo "  Start it first: bash spike/track-grading/serve_music_flamingo.sh" >&2
    exit 1
  fi
  echo "  vLLM Music Flamingo: OK (port 8001)"
  if [ ! -f "/home/data01/Music/mesh-track-grading/round7_5_priors.npz" ]; then
    echo "ERROR: missing /home/data01/Music/mesh-track-grading/round7_5_priors.npz" >&2
    echo "  Round-7.5 BT priors are needed for uncertainty sampling." >&2
    exit 1
  fi
  echo "  Round-7.5 BT priors: OK"
  ls /home/data01/Music/mesh-track-grading/round7_5_pairs >/dev/null 2>&1 || {
    echo "ERROR: missing /home/data01/Music/mesh-track-grading/round7_5_pairs (the existing tuples to re-judge)" >&2
    exit 1
  }
  echo "  Round-7.5 pairs cache: OK"
  echo "  GPU:"
  nvidia-smi --query-gpu=memory.used,memory.free,utilization.gpu --format=csv | tail -1
  echo
}

# ─── stage: smoke ───────────────────────────────────────────────────
case "$STAGE" in
  smoke)
    preflight
    echo "=== stage 0: smoke test (50 K=4 tuples, ~1 min) ==="
    $RUN smoke_test_judge.py \
      --judge music_flamingo \
      --n 50 \
      --axes timbre_roughness mood_polarity
    echo
    echo "If parse rate ≥ 90% and sustained throughput ≥ 0.5 calls/sec, env is good."
    echo "Run 'bash $0 full' to start the production tournament."
    ;;

  full)
    preflight
    echo "=== stage 1/7: tournament (Music Flamingo, 20k uncertain pairs) ==="
    $RUN run_judge_tournament.py \
      --judge music_flamingo \
      --pairs-source reuse-existing \
      --pairs-subset uncertain-20000 \
      --workers 12

    echo "=== stage 2/7: BT priors from MF rankings ==="
    $RUN build_bt_priors_r7_5.py \
      --pairs-root /home/data01/Music/mesh-track-grading/round7_6_pairs/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_priors.npz

    echo "=== stage 3/7: multi-task linear probe (PyTorch, mini-batched) ==="
    $RUN train_axes_r7_5.py \
      --priors /home/data01/Music/mesh-track-grading/round7_6_priors.npz \
      --tags /home/data01/Music/mesh-track-grading/round7_5_tags.npz \
      --out-axes /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --out-preds /home/data01/Music/mesh-track-grading/round7_6_predictions.csv \
      --out-metrics /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json

    echo "=== stage 4/7: ListMLE blend (target: timbre_roughness) ==="
    $RUN joint_blend_r7_5.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --priors /home/data01/Music/mesh-track-grading/round7_6_priors.npz \
      --target-axis timbre_roughness \
      --out /home/data01/Music/mesh-track-grading/round7_6_blend.npz

    echo "=== stage 5/7: interpret + cross-library ==="
    $RUN interpret_axes_r7_5.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --out /home/data01/Music/mesh-track-grading/round7_6_interpretation.md
    $RUN cross_library_r7.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --blend-file /home/data01/Music/mesh-track-grading/round7_6_blend.npz \
      --out /home/data01/Music/mesh-track-grading/round7_6_cross_library.md

    echo "=== stage 6/7: V18 export (NOT auto-deployed) ==="
    $RUN export_axis_r7_5.py \
      --blend /home/data01/Music/mesh-track-grading/round7_6_blend.npz \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --metrics /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json \
      --out models/aggression-axes/V18_round7_6_DEPRECATED_likert.json \
      --no-deploy

    echo "=== stage 7/7: V15 vs V17b vs V18 head-to-head ==="
    # compare_v15_v16_v17.py covers V15/V16/V17 today; we extend it
    # by symlinking V18 over V17 location? No — write a small variant:
    $RUN compare_v15_v16_v17.py
    echo
    echo "ALSO compare V18 specifically:"
    LD_LIBRARY_PATH="/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:/nix/store/1xw5xccqqh1xw3mvd70hyil6x418wxcm-gcc-14.3.0-lib/lib" \
    "$HOME/.cache/mesh-spike/vllm-env/bin/python" -c "
import json
import numpy as np
from pathlib import Path
def spearman(a,b):
    n=len(a); ra=np.argsort(np.argsort(a)); rb=np.argsort(np.argsort(b))
    return 1 - 6*float(np.sum((ra-rb)**2))/(n*(n*n-1))
def pa(s,y):
    n=len(s); ds=s[:,None]-s[None,:]; dy=y[:,None]-y[None,:]
    tri=np.triu(np.ones((n,n),dtype=bool),k=1); valid=tri & (ds!=0) & (dy!=0)
    return float((valid & ((ds>0)==(dy>0))).sum()/max(valid.sum(),1))

import sys; sys.path.insert(0,'spike/track-grading')
from pycozo.client import Client
db = Client('sqlite','/home/data01/Music/mesh-collection/mesh.db',{'dataframe':False})
rows = db.run('?[track_id, vec] := *ml_embeddings{track_id, vec}')['rows']
tids, embs = [], []
for tid, vec in rows:
    if vec is not None:
        tids.append(int(tid)); embs.append(vec)
tids = np.array(tids,dtype=np.int64); embs = np.array(embs,dtype=np.float32)
bt = {}
for line in open('documents/axis-eval-results/llm-pair-priors-r5.txt'):
    line=line.strip()
    if not line or line.startswith('#'): continue
    parts=line.split('|',2)
    if len(parts)==3: bt[int(parts[0])]=float(parts[2])
mask = np.array([t in bt for t in tids])
tids = tids[mask]; embs=embs[mask]
y = np.array([bt[t] for t in tids],dtype=np.float32)
for label, path in [
    ('V15','models/aggression-axes/V15_linear_probe_r6.json'),
    ('V17b','models/aggression-axes/V17_round7_5_polar_blend.json'),
    ('V18','models/aggression-axes/V18_round7_6_DEPRECATED_likert.json'),
]:
    if not Path(path).exists():
        print(f'{label} MISSING: {path}'); continue
    v = json.loads(open(path).read())
    vec = np.array(v['intensity_axis_vec'],dtype=np.float32)
    score = embs @ vec
    print(f'{label:>5}  rho={spearman(score,y):+.4f}  PA={pa(score,y):.4f}')
"

    echo
    echo "=== round-7.6 done ==="
    echo "Outputs:"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_priors.npz"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_axes.npz"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_blend.npz"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_predictions.csv"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_interpretation.md"
    echo "  /home/data01/Music/mesh-track-grading/round7_6_cross_library.md"
    echo "  models/aggression-axes/V18_round7_6_DEPRECATED_likert.json"
    echo
    echo "V15 stays deployed at <collection>/muq-mulan-aggression-axis.json."
    ;;

  post)
    # Skip the tournament; run only the post-tournament stages on
    # whatever round-7.6 pair data is already on disk.
    preflight
    if [ ! -d /home/data01/Music/mesh-track-grading/round7_6_pairs/music_flamingo ]; then
      echo "ERROR: no Music Flamingo pair data yet — run 'full' or 'smoke' first" >&2
      exit 1
    fi
    echo "=== post-tournament stages only ==="
    $RUN build_bt_priors_r7_5.py \
      --pairs-root /home/data01/Music/mesh-track-grading/round7_6_pairs/music_flamingo \
      --out /home/data01/Music/mesh-track-grading/round7_6_priors.npz
    $RUN train_axes_r7_5.py \
      --priors /home/data01/Music/mesh-track-grading/round7_6_priors.npz \
      --tags /home/data01/Music/mesh-track-grading/round7_5_tags.npz \
      --out-axes /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --out-preds /home/data01/Music/mesh-track-grading/round7_6_predictions.csv \
      --out-metrics /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json
    $RUN joint_blend_r7_5.py \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --priors /home/data01/Music/mesh-track-grading/round7_6_priors.npz \
      --target-axis timbre_roughness \
      --out /home/data01/Music/mesh-track-grading/round7_6_blend.npz
    $RUN export_axis_r7_5.py \
      --blend /home/data01/Music/mesh-track-grading/round7_6_blend.npz \
      --axes-file /home/data01/Music/mesh-track-grading/round7_6_axes.npz \
      --metrics /home/data01/Music/mesh-track-grading/round7_6_train_metrics.json \
      --out models/aggression-axes/V18_round7_6_DEPRECATED_likert.json \
      --no-deploy
    ;;

  help|*)
    cat <<EOF
Usage: bash $0 {caption-smoke|caption-full|caption-rate|v18-train|pointwise-smoke|pointwise-full|smoke|full|post}

Round-7.6 V18 pipeline (per documents/round-7-6-pipeline-spec.md):
  caption-smoke    Caption 200 tracks (~5-15 min) → embed with bge-base
                   → ridge CV transfer test against V15/V17b/r7.5 BT priors.

  caption-full     S1+S2+S3: caption all corpus tracks (~8 hr at 0.5 c/s
                   with 256-token captions, T=0.7), embed with bge-base,
                   extract structured tags. Stop the MF serve afterward.

  caption-rate     S4: caption → text-LLM 1-5 intensity rating. Requires
                   `serve_text_llm.sh` running on port 8002 (~30 min).

  v18-train        S6-S13: build 4-source consensus → Dawid-Skene aggregate
                   → artist-stratified split → teacher (privileged) →
                   student distill (linear probe) → held-out eval +
                   caption-emb K-means cluster diagnostic → V18 export.
                   ~2 hr GPU + 30 min downstream.

DEPRECATED — Likert pointwise (works but yields narrow signal):
  pointwise-smoke  200×16 Likert smoke (~12 min). Spreads OK on concrete
                   axes (timbre/mood/vocal), narrow on subjective ones.
  pointwise-full   245k Likert cells (~13 hr).

DEPRECATED — K=4 N-way (architecturally infeasible for MF):
  smoke / full / post — Qwen3-Omni-only path; MF rejects K>1 audios.

Pre-requisite: vLLM Music Flamingo running on port 8001:
  bash spike/track-grading/serve_music_flamingo.sh
EOF
    ;;
esac
