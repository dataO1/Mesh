#!/usr/bin/env bash
# E4.5 — Verify LoRA-tuned encoder: train teacher + student on LoRA embeddings,
# evaluate against held-out test set, compare to V18.X+A5 baseline.
#
# Usage: bash spike/track-grading/run_e45_verify.sh
set -euo pipefail

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
BASE=/home/data01/Music/mesh-track-grading
AUDIO_EMB="$BASE/embeddings/corpus_muq_mulan_lora.npz"
AUDIO_KEY="embeddings_1024"
CAPTION_EMB="$BASE/round7_6_caption_emb.npz"
STRUCT_TAGS="$BASE/round7_6_caption_struct.npz"
CONSENSUS="$BASE/round7_6_consensus.npz"
SPLIT="$BASE/round7_6_split.npz"

HIDDEN_DIM="${1:-128}"

RUN="nix develop $PROJECT/#mlspike --command python"

echo "=== E4.5: LoRA verification (student h=$HIDDEN_DIM) ==="
echo

# ── Teacher ──────────────────────────────────────────────────────────
echo "--- Teacher ---"
$RUN spike/track-grading/train_v18_teacher.py \
  --audio-emb     "$AUDIO_EMB" \
  --audio-emb-key "$AUDIO_KEY" \
  --caption-emb   "$CAPTION_EMB" \
  --struct-tags   "$STRUCT_TAGS" \
  --consensus     "$CONSENSUS" \
  --split         "$SPLIT" \
  --out-dir       "$BASE"

# ── Student ─────────────────────────────────────────────────────────
echo
echo "--- Student (h=$HIDDEN_DIM) ---"
$RUN spike/track-grading/distill_v18_student.py \
  --audio-emb      "$AUDIO_EMB" \
  --audio-emb-key  "$AUDIO_KEY" \
  --teacher-preds  "$BASE/round7_6_teacher_preds.npz" \
  --consensus      "$CONSENSUS" \
  --out-dir        "$BASE" \
  --student-arch   mlp \
  --hidden-dim     "$HIDDEN_DIM"

# ── Export held-out baseline ────────────────────────────────────────
echo
echo "--- Held-out baseline export ---"
$RUN spike/track-grading/export_heldout_baseline.py \
  --audio-emb      "$AUDIO_EMB" \
  --audio-emb-key  "$AUDIO_KEY" \
  --student-pt     "$BASE/round7_6_student.pt" \
  --consensus      "$CONSENSUS" \
  --split          "$SPLIT" \
  --out-md         "/home/data01/Notes/🗂️ Collection/Mesh — V18.X+LoRA Held-Out Baseline.md"

echo
echo "=== E4.5 done ==="
