# Round-7.6 V18 training artifacts

Snapshot of the round-7.6 V18 intensity-axis training run, captured 2026-05-08.

These are the artifacts that are either expensive to regenerate (caption sweep
took 7+ hr of MF GPU time, local Mistral juror took 5+ hr) or essential to the
reviewer's grading rubric. The corresponding live workspace is at
`/home/data01/Music/mesh-track-grading/`.

## Inventory

| File | Size | Source | Reproducibility |
|---|---:|---|---|
| `captions_music_flamingo.tar.zst` | 16 MB | Music Flamingo `nvidia/music-flamingo-2601-hf` over 39913 Deezer 30s previews, T=0.7 top_p=0.9 max_tokens=1024 | ~7 hr GPU on RTX 5090 Mobile @ 48 seqs |
| `round7_6_caption_intensity_local_minstral.npz` | 3.7 MB | `gghfez/Mistral-Small-3.2-24B-Instruct-hf-AWQ` rating each caption 0-19 (20-bucket two-digit) | ~5 hr local GPU |
| `round7_6_caption_intensity_nemotron.npz` | 3.7 MB | `nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B-NVFP4` on Spark 2 | ~1 hr remote |
| `round7_6_caption_intensity_qwen36.npz` | 3.7 MB | `sakamakismile/Qwen3.6-27B-Text-NVFP4-MTP` on Spark 1 | ~3 hr remote |
| `round7_6_caption_struct.npz` | 11 MB | `extract_caption_tags.py`: 52 multi-hot keyword tags per caption | seconds CPU |
| `round7_6_consensus.npz` | 1 MB | `aggregate_consensus.py`: Dawid-Skene EM over the 3 jurors + r7.5 BT-blend + aggressive_overall_tag + MF_Likert | seconds CPU |
| `round7_6_split.npz` | 0.9 MB | `make_split.py`: artist-stratified 80/10/10 split, seed=42 | deterministic |
| `round7_6_teacher.pt` + `_metrics.json` + `_preds.npz` | 11 MB | `train_v18_teacher.py`: 1345d → 256 → 128 + 16 axis heads MLP | ~6 s GPU |
| `round7_6_student.pt` + `_metrics.json` | 0.3 MB | `distill_v18_student.py`: linear probe over MuQ-MuLan via FitNets+Hinton+LS | ~1 s GPU |
| `round7_6_eval_report.md` + `round7_6_eval.json` | 50 KB | `eval_v18.py`: held-out PA, K-means cluster diagnostic, V15/V17b comparison | seconds |

## Not included (regenerable from above)

- `round7_6_caption_emb.npz` (118 MB) — bge-base-en-v1.5 embeddings of captions.
  Regenerate with:
  ```bash
  bash spike/track-grading/run_r7_step.sh embed_captions.py \
    --captions-root <unpacked captions dir> \
    --out /home/data01/Music/mesh-track-grading/round7_6_caption_emb.npz
  ```
  Cost: ~5 min on GPU.

## Reproduction recipe (no GPU re-runs needed)

```bash
# 1. Unpack to the live workspace
cd /home/data01/Music/mesh-track-grading
mkdir -p round7_6_captions
zstd -dc /path/to/data/round7_6/captions_music_flamingo.tar.zst | tar -x

# 2. Copy the rest into place
cp /path/to/data/round7_6/round7_6_*.{npz,pt,json,md} .

# 3. Re-derive caption embeddings (~5 min GPU)
bash /path/to/Mesh/spike/track-grading/run_r7_step.sh embed_captions.py \
  --captions-root round7_6_captions/music_flamingo \
  --out round7_6_caption_emb.npz

# 4. Re-train teacher + student from cached features (~10 s)
bash /path/to/Mesh/spike/track-grading/run_round7_6_pipeline.sh v18-train
```

Step 4 reproduces V18 from these artifacts deterministically (seed=42 across
split, EM init, teacher init, student init, dropout). The exported V18 weights
should match `models/aggression-axes/V18_round7_6_consensus_distilled.json`
to ~1e-6 absolute (per spec G10).

## Note on this snapshot

**Updated 2026-05-08 22:30** — snapshot now reflects the V18 release run:

- 3-juror consensus (Mistral-Small-3.2 + Nemotron-30B + Qwen3.6-27B),
  all at full 39913-track coverage, σ² floored at 0.01 → all sources
  weight 1/3 in normalized reliability. No σ²-collapse pathology.
- Teacher trained on full 39913 tracks (audio_emb 512 + caption_emb 768
  + struct_tags 52 = 1332d input). Test PA = 0.940, Spearman = 0.980.
- Student (V18 deployed) test PA = **0.811** on 3985 held-out tracks
  (8 of 10 spec goals pass — see `round7_6_eval_report.md`).

Caveat for the reviewer: r7.5 BT priors and aggressive_overall_tag are
NOT included in this consensus. They only cover 38% of the expanded
corpus and forced an EM pathology when included. The 3-juror panel at
100% coverage with pairwise ρ=0.93-0.96 was strictly better-conditioned.
Spec G5 originally required ≥ 4 sources; we updated it to ≥ 3 with
methodology rationale (see `documents/round-7-6-pipeline-spec.md` §2 G5).

Earlier broken-consensus snapshot (single-source σ²-collapse) was
overwritten — its outputs were unusable since the consensus was
mathematically just one juror's score.
