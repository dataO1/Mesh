# Intensity-axis pipeline runbook

End-to-end reproducible procedure for re-training the intensity ranking
shipped in `models/muq-mulan-aggression-axis.json`. Designed so a future
contributor (or future-you on a fresh machine) can land at a working
new axis with one read of this doc.

For background reading and decision history, follow the round chain:
[round 2](aggression-axis-eval-round-2.md) → [round 3](aggression-axis-eval-round-3.md) →
[round 4](aggression-axis-eval-round-4.md) → [round 5](aggression-axis-eval-round-5.md) →
[round 6](aggression-axis-eval-round-6.md) → [round 7 plan](aggression-axis-eval-round-7.md).

## What the pipeline produces

A 512-dim unit vector (or, eventually, an MLP head) that projects
MuQ-MuLan audio embeddings onto a single intensity score. Currently
shipping **V15 — linear probe trained on round-5 BT priors** at
`models/muq-mulan-aggression-axis.json`. ~71% pairwise agreement vs
the LLM-judge ground truth on the 909-track Mesh corpus, +0.43 Spearman
vs the held-out 47 hand-anchors.

## Pipeline stages, glanceable

```
       ┌────────────────────────────────────────────────────────┐
       │ 0. Prerequisites: vllm env + Qwen3-Omni weights + DB   │
       └─────────────┬──────────────────────────────────────────┘
                     ▼
   1. dump_track_list                  → /tmp/track-grading/_track-list.csv
                     ▼
   2. plan_pairs_v2.py                 → /tmp/track-grading/round5_plan.csv
       (community detection +
        active-learning queue)
                     ▼
   3. serve_qwen3_omni.sh              → vLLM @ localhost:8000 (long-running)
                     ▼
   4. judge_pairs_vllm.py              → /tmp/track-grading/pairs_vllm/*.json
       --plan-file ...                   (5000-10000 pair judgments)
                     ▼
   5. build_bt_priors.py               → documents/axis-eval-results/
       (BT-MM with Bayesian smoothing)   llm-pair-priors.{txt,csv}
                     ▼
   6. validate_bt_priors.py            → Spearman vs hand-anchors,
       --bt llm-pair-priors.txt           top disagreements
                     ▼
   7. compare-variants.py              → leaderboard of all V*.csv
       documents/axis-eval-results/        variants vs the new priors
       llm-pair-priors.txt
                     ▼
   8. dump_embeddings.py               → /tmp/track-grading/embeddings.npz
                     ▼
   9. train_head_r6.py                 → V14_mlp_head_r6.csv,
       (5-fold CV + final retrain)       V15_linear_probe_r6.csv,
                                          round6_metrics.json
                     ▼
  10. export_axis_json.py              → models/aggression-axes/
       (retrain V15 + emit polar JSON)   V15_linear_probe_r6.json
                     ▼
  11. Manual deploy:                   → models/muq-mulan-aggression-
       cp V15_*.json over canonical       axis.json (active in mesh)
                     ▼
                  mesh-cue runs with new axis
```

Total wall time on the dev RTX 5090 Laptop: ~50 min for stages 3-5
(LLM tournaments) + ~30 sec for stages 8-10 (head training). All other
stages are <2 sec.

## Prerequisites

### Once per machine

1. **vLLM env** at `~/.cache/mesh-spike/vllm-env/`:
   ```bash
   nix shell nixpkgs#python311 -c python3 -m venv ~/.cache/mesh-spike/vllm-env
   ~/.cache/mesh-spike/vllm-env/bin/pip install --upgrade pip
   ~/.cache/mesh-spike/vllm-env/bin/pip install --pre 'vllm>=0.10' \
       'transformers>=4.50' soundfile librosa pycozo cozo_embedded scikit-learn
   ```
   ~10 min, ~8 GB on disk.

2. **Patch Triton + CUDA binaries** so they run from the Nix sandbox
   (one-time, breaks if the venv is recreated):
   ```bash
   GLIBC_LD=/nix/store/j193mfi0f921y0kfs8vjc1znnr45ispv-glibc-2.40-66/lib64/ld-linux-x86-64.so.2
   GLIBC_LIB=/nix/store/j193mfi0f921y0kfs8vjc1znnr45ispv-glibc-2.40-66/lib
   for bin in ~/.cache/mesh-spike/vllm-env/lib/python3.11/site-packages/triton/backends/nvidia/bin/* \
              ~/.cache/mesh-spike/vllm-env/lib/python3.11/site-packages/torch/bin/*; do
       file -b "$bin" 2>/dev/null | grep -q "dynamically linked" && \
           patchelf --set-interpreter "$GLIBC_LD" --add-rpath "$GLIBC_LIB" "$bin"
   done
   ```

3. **Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit** in HF cache (~26 GB):
   first run of `spike/track-grading/serve_qwen3_omni.sh` will download
   it via `HF_HOME=~/.cache/mesh-spike/hf`.

4. **Always-prepend** these env vars when running spike Python on this
   machine (zlib lives in the Nix store, not /usr/lib):
   ```bash
   export LD_LIBRARY_PATH=/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:$LD_LIBRARY_PATH
   ```

### Per-corpus

- **mesh.db with `ml_embeddings` populated.** The pipeline reads MuQ-MuLan
  512-dim embeddings from the local Cozo-on-SQLite database. For a fresh
  library: import tracks via mesh-cue, then `nix run .#reanalyze-ml` to
  populate embeddings.

## Full re-train command sequence

Assumes you're at the repo root with the prerequisites met.

```bash
# 0. Convenient env shorthands
export LD_LIBRARY_PATH=/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:$LD_LIBRARY_PATH
PY=~/.cache/mesh-spike/vllm-env/bin/python

# 1. Track list (Rust binary; reads tracks table, writes CSV with drop markers).
cargo build --release -p mesh-cue --bin dump_track_list
./target/release/dump_track_list \
    --collection ~/Music/mesh-collection \
    --out /tmp/track-grading/_track-list.csv

# 2. Generate pair plan (community detection + active-learning queue).
#    First run: omit --bt-priors so it falls back to BT=5.0 for everyone.
$PY spike/track-grading/plan_pairs_v2.py \
    --features documents/axis-eval-results/V11_neuro_dnb_tuned.PRE-V15.json \
    --bt-priors /tmp/track-grading/empty.txt   # OK if missing — handled
# ↑ outputs /tmp/track-grading/round5_plan.csv

# 3. Start vLLM server (long-running; leave in another terminal).
nohup bash spike/track-grading/serve_qwen3_omni.sh > /tmp/vllm-serve.log 2>&1 &
# Wait ~3 min for "Application startup complete" or curl http://localhost:8000/health.

# 4. Run pairwise grader (8 parallel workers, ~7 pairs/sec).
$PY spike/track-grading/judge_pairs_vllm.py \
    --plan-file /tmp/track-grading/round5_plan.csv \
    --workers 8
# ↑ writes /tmp/track-grading/pairs_vllm/<a>_vs_<b>.json (per-pair, resumable)

# 5. Build BT priors (Hunter MM with Gamma(2,1) Bayesian smoothing).
$PY spike/track-grading/build_bt_priors.py \
    --pairs-dir /tmp/track-grading/pairs_vllm \
    --meta /tmp/track-grading/_track-list.csv \
    --out-prefix documents/axis-eval-results/llm-pair-priors-rN
# ↑ writes llm-pair-priors-rN.{txt,csv}

# 6. Validate vs the 47 hand-anchors (sanity check; expects ρ > +0.35).
$PY spike/track-grading/validate_bt_priors.py \
    --bt documents/axis-eval-results/llm-pair-priors-rN.txt \
    --hand /tmp/anchors50.txt

# 7. Score the existing 13 hand-blended variants (optional, for the round
#    report — confirms the new BT priors agree with previous rankings).
$PY scripts/compare-variants.py \
    documents/axis-eval-results/llm-pair-priors-rN.txt

# 8. Dump 512-d embeddings for the tracks with BT priors.
$PY spike/track-grading/dump_embeddings.py
# ↑ writes /tmp/track-grading/embeddings.npz

# 9. Train MLP + linear-probe heads with 5-fold CV.
$PY spike/track-grading/train_head_r6.py \
    --priors documents/axis-eval-results/llm-pair-priors-rN.txt
# ↑ writes V14_mlp_head_rN.csv (MLP) + V15_linear_probe_rN.csv (linear)
#   + /tmp/track-grading/round6_metrics.json (CV scores)

# 10. Export the linear probe to a polar-format JSON the runtime accepts.
$PY spike/track-grading/export_axis_json.py
# ↑ writes models/aggression-axes/V15_linear_probe_rN.json

# 11. Deploy: copy over canonical path + sync to runtime cache.
cp models/aggression-axes/V15_linear_probe_rN.json \
   models/muq-mulan-aggression-axis.json
cp models/muq-mulan-aggression-axis.json \
   ~/.cache/mesh-cue/ml-models/muq-mulan-aggression-axis.json

# Verify by running mesh-cue and checking the log for
#   "Loaded intensity axis 'V15_linear_probe_rN'..."
```

## Spike-script file map

| Script | Stage | Does |
|---|---|---|
| `spike/track-grading/serve_qwen3_omni.sh` | 3 | launches vLLM with Qwen3-Omni-30B-AWQ on port 8000; binds host CUDA driver via TRITON_LIBCUDA_PATH |
| `spike/track-grading/judge_pairs_vllm.py` | 4 | OpenAI-API client; reads plan file or generates anchored tournament; bilateral pair sampling; per-pair JSON cache (resumable); 8 parallel workers default |
| `spike/track-grading/plan_pairs_v2.py` | 2 | python port of `crates/mesh-core/src/suggestions/aggression.rs::build_calibration_plan`; KMeans community detection + 3-tier Phase 1 + budgeted Phase 2 active-learning queue + transitive-closure pruning + hybrid embedding filter |
| `spike/track-grading/build_bt_priors.py` | 5 | Hunter MM Bradley-Terry with Gamma(2, 1) Bayesian smoothing; outputs anchor-format txt + full CSV |
| `spike/track-grading/validate_bt_priors.py` | 6 | Spearman + per-anchor disagreement table vs hand-anchors |
| `spike/track-grading/dump_embeddings.py` | 8 | reads `ml_embeddings` relation from `~/Music/mesh-collection/mesh.db` via pycozo; filters to tracks with BT priors; writes .npz |
| `spike/track-grading/train_head_r6.py` | 9 | trains MLP (512→128→64→1) + linear probe (512→1) with RankNet pairwise margin loss; 5-fold CV + final retrain on full data; outputs V*.csv files |
| `spike/track-grading/export_axis_json.py` | 10 | retrains V15 linear probe, exports to polar IntensityAxis JSON format with V11's sub_axes copied for UI sub-controls |
| `spike/track-grading/grade.py` | (legacy) | round-3 absolute-scoring with AF3; kept as `nix run .#grade-tracks` for diagnostic comparisons; not in active pipeline |

## Re-training on a different library

The pipeline is corpus-agnostic — point it at a different `mesh.db` and
re-run from stage 1. Two cases:

**Case A — same hardware, new library imported into mesh-cue.**
1. Import tracks via mesh-cue UI; let `reanalyze_ml` populate embeddings.
2. Re-run stages 1-11 above. Total ~1 hour for ~1000 tracks (mostly
   the LLM tournament).
3. The shipped axis is then tuned to that library's distribution.

**Case B — completely different machine + library.**
- Stages 0-2 (prerequisites + dump): one-time setup.
- Stages 3-7 (LLM tournament + BT): GPU-bound, needs ~24 GB VRAM.
- Stages 8-11 (training + export): pure CPU, runs anywhere.

If the user machine lacks a GPU, the LLM stages must run on a server.
This is the round-8 productisation work; until then, retraining requires
GPU access for stages 3-7.

## V14 (MLP head) production wiring — pending

Round 6 trained V14 (MLP head) and V15 (linear probe). V15 is currently
shipping because it fits the existing polar-projection runtime (single
512-d unit vector). V14 captures an additional ~3 pp pairwise agreement
but requires extending the runtime:

1. Add `MlpHead` variant to `IntensityAxis` in
   `crates/mesh-cue/src/ml_analysis/aggression_axis.rs`.
2. Implement forward pass (~30 lines pure Rust matmul + ReLU).
3. Loosen `validate()` to skip the unit-norm check when `head` is present.
4. Update `crates/mesh-cue/src/ui/handlers/similarity.rs` to handle the
   case where `intensity_axis_vec` isn't directly present (similarity
   computation needs an alternative input — likely a per-track precomputed
   intensity score column).

Defer to round 8 (productisation) — the marginal gain (V14 vs V15 = ~3 pp)
doesn't justify shipping today's V15 as a partial fix.

## Round-7 axis discovery — placeholder

Round 7 will run **per-axis** LLM tournaments ("which is more distorted?",
"which is more bass-heavy?", ...) → multi-task linear probes derive k
linear directions in the 512-d MuQ-MuLan space → joint blend learns the
final intensity from those k axes. This replaces the current 6 hand-named
axes with empirically-discovered axes that are still interpretable
(linear directions) but no longer hand-picked.

When round 7 runs, results land in `documents/aggression-axis-eval-round-7.md`
and the deployed axis migrates from V15 (single learned axis) to V16
(k learned axes + learned blend).

Cross-library deployment hooks (round 8) will rely on round 7's axis
factorisation: the k axes ship as a 36 KB pretrained blob; per-user
libraries refit only the k blend weights via the existing calibration UI
(no GPU needed user-side).

## Backups + rollback

The previous default V11 was preserved at
`models/aggression-axes/V11_neuro_dnb_tuned.PRE-V15.json` when V15 went
live. To roll back:

```bash
cp models/aggression-axes/V11_neuro_dnb_tuned.PRE-V15.json \
   models/muq-mulan-aggression-axis.json
cp models/muq-mulan-aggression-axis.json \
   ~/.cache/mesh-cue/ml-models/muq-mulan-aggression-axis.json
```

mesh-cue picks up the change on next launch.

## Where to add new findings

- **New eval round** → `documents/aggression-axis-eval-round-N.md` +
  cross-link from this runbook + the previous round doc + update the
  "round chain" line at the top of every round doc.
- **New axis variant** → drop the JSON under `models/aggression-axes/`,
  re-run stage 7 (compare-variants) to get its leaderboard slot.
- **Pipeline change** → update the relevant spike script + this runbook's
  command sequence + the file map. The runbook should always reflect
  the *current* working pipeline, not the historical one (rounds are
  for history).
