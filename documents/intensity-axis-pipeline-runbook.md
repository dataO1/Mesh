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
   1. dump_track_list                  → /home/data01/Music/mesh-track-grading/_track-list.csv
                     ▼
   2. plan_pairs_v2.py                 → /home/data01/Music/mesh-track-grading/round5_plan.csv
       (community detection +
        active-learning queue)
                     ▼
   3. serve_qwen3_omni.sh              → vLLM @ localhost:8000 (long-running)
                     ▼
   4. judge_pairs_vllm.py              → /home/data01/Music/mesh-track-grading/pairs_vllm/*.json
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
   8. dump_embeddings.py               → /home/data01/Music/mesh-track-grading/embeddings.npz
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
    --out /home/data01/Music/mesh-track-grading/_track-list.csv

# 2. Generate pair plan (community detection + active-learning queue).
#    First run: omit --bt-priors so it falls back to BT=5.0 for everyone.
$PY spike/track-grading/plan_pairs_v2.py \
    --features documents/axis-eval-results/V11_neuro_dnb_tuned.PRE-V15.json \
    --bt-priors /home/data01/Music/mesh-track-grading/empty.txt   # OK if missing — handled
# ↑ outputs /home/data01/Music/mesh-track-grading/round5_plan.csv

# 3. Start vLLM server (long-running; leave in another terminal).
nohup bash spike/track-grading/serve_qwen3_omni.sh > /tmp/vllm-serve.log 2>&1 &
# Wait ~3 min for "Application startup complete" or curl http://localhost:8000/health.

# 4. Run pairwise grader (8 parallel workers, ~7 pairs/sec).
$PY spike/track-grading/judge_pairs_vllm.py \
    --plan-file /home/data01/Music/mesh-track-grading/round5_plan.csv \
    --workers 8
# ↑ writes /home/data01/Music/mesh-track-grading/pairs_vllm/<a>_vs_<b>.json (per-pair, resumable)

# 5. Build BT priors (Hunter MM with Gamma(2,1) Bayesian smoothing).
$PY spike/track-grading/build_bt_priors.py \
    --pairs-dir /home/data01/Music/mesh-track-grading/pairs_vllm \
    --meta /home/data01/Music/mesh-track-grading/_track-list.csv \
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
# ↑ writes /home/data01/Music/mesh-track-grading/embeddings.npz

# 9. Train MLP + linear-probe heads with 5-fold CV.
$PY spike/track-grading/train_head_r6.py \
    --priors documents/axis-eval-results/llm-pair-priors-rN.txt
# ↑ writes V14_mlp_head_rN.csv (MLP) + V15_linear_probe_rN.csv (linear)
#   + /home/data01/Music/mesh-track-grading/round6_metrics.json (CV scores)

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
| `spike/track-grading/scrape_everynoise.py` | r7-prep | scrapes 6291 genre cells from everynoise.com → JSON with playlist_id, preview_url, atlas position, example_track |
| `spike/track-grading/categorize_genres.py` | r7-prep | three-tier classifier (HARD_BLOCK / INCLUDE / SOFT_BLOCK) → 2116 DJ-relevant genres |
| `spike/track-grading/fetch_deezer_tracks.py` | r7-prep | generic seed-list adapter (everynoise format or flat list); per seed → Deezer search + `/artist/{id}/radio` → 10 tracks; rate-gated 10 req/s; resume-safe |
| `spike/track-grading/download_previews.py` | r7-prep | parallel HTTP downloader (32 workers default) for any manifest with id+url columns; atomic writes; skip-existing |
| `spike/track-grading/build_corpus.sh` | r7-prep | one-shot wrapper chaining scrape → categorize → fetch → download with banner output + tee'd logs |
| `spike/track-grading/fetch_spotify_tracks.py` | (blocked) | Spotify equivalent of fetch_deezer_tracks.py; blocked by Spotify's Premium-required policy on dev apps; left in place if policy changes or you have Premium |

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

## Round-7 axis discovery — corpus prep

Round 7 needs a multi-genre training corpus far broader than the 909
DnB-heavy Mesh tracks rounds 2-6 used. We chose a **DJ-relevant subset
of everynoise.com + Deezer for audio fetch**. No Spotify (their Web API
gating now requires Premium + ~24h propagation, see "API choices" below).

### Pipeline

```
   1. scrape_everynoise.py            → /home/data01/Music/mesh-track-grading/everynoise_genres.json
                                         (6291 genres × {playlist_id, preview_url,
                                          example_track, atlas position})
              ▼
   2. categorize_genres.py            → /home/data01/Music/mesh-track-grading/everynoise_dj_genres.json
                                         HARD_BLOCK > INCLUDE > SOFT_BLOCK rule;
                                         result: 2116 INCLUDE / 1920 BLOCK / 2255 NEUTRAL
              ▼
   3. fetch_deezer_tracks.py          → /home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json
                                         per seed: search → /artist/{id}/radio →
                                         10 tracks (1 seed + 9 radio); cached per
                                         seed for resume; rate-gated 10 req/s
              ▼
   4. download_previews.py            → /home/data01/Music/mesh-track-grading/audio/dz_<id>.mp3
                                         32 parallel HTTP workers (CDN, separate
                                         from API quota); ~10 GB total
              ▼
            corpus ready for MuQ-MuLan embedding extraction +
            per-axis Qwen3-Omni LLM tournaments (round 7 proper)
```

The whole corpus build is wrapped:
```bash
bash spike/track-grading/build_corpus.sh
# Phase 0 (scrape, if needed) + Phase 1 (Deezer search) + Phase 2 (download)
# Total wall time: ~50-60 min on a residential connection.
```

### API choices and why

**Spotify Web API — abandoned.** Tested with a free dev account in
late-2024-/-2025: every `/playlists/{id}/tracks` call returned HTTP 403
"Active premium subscription required for the owner of the app." The dev
app must be bound to a Spotify Premium account before public-playlist
reads work, and the policy change propagates ~few hours after Premium is
activated. This blocked the cleanest "everynoise → Spotify playlist
expansion → preview MP3" path. Script left at
`spike/track-grading/fetch_spotify_tracks.py` if Premium becomes
available later — it's wired correctly, just blocked by the policy.

**Deezer public API — chosen.** No auth, no Premium, no OAuth required
for read-only. Anonymous limit ~50 req / 5 sec for `api.deezer.com`;
preview MP3s served from a separate Google-Frontend CDN
(`cdnt-preview.dzcdn.net`) with no per-IP throttling in practice. Each
preview is a content-addressed 30 s, ~470 KB MP3 — deterministic per
track ID, so re-fetching is idempotent.

**Audio quality of Deezer previews — empirically verified.** The 30 s
clip is *not* random and *not* always the first 30 s. Probed 8 tracks
across categories (ambient synth, ambient house, afrobeat, afrobeats
pop, alternative Christian) by computing per-frame RMS shape, mean
loudness, and onset rate:

```
ambient synth  Jogging House — Flight              -19.5 dB  flat   1.83 onset/s
ambient synth  Jogging House — Strings             -25.1     flat   1.40
afrobeats      KCee — Pullover (Remix)              -8.9     flat   5.44
afrobeat       Antibalas — Battle of the Spec      -14.6     flat   4.07
ambient house  Khotin — Groove 32                  -13.3     flat   5.74
ambient house  Khotin — WEM Lagoon Jump            -10.5     flat   1.97
ambient house  Khotin — Shopping List              -16.1     flat   0.40
alt-christian  Shane & Shane — Knowing You         -29.7     rising 1.87
```

7/8 had **flat** RMS shape (the signature of chorus / drop / sustained
section); the lone "rising" was a slow worship track with no clear hook.
RMS levels matched expected genre energy (ambient quiet, pop afrobeats
loud, slow worship very quiet). Conclusion: Deezer's selection is
hook-aware on most modern produced music, comparable in spirit to our
own `drop_marker`-centered 30 s clip — different mechanism, same goal.

### Categorisation rule (round-7 corpus)

`spike/track-grading/categorize_genres.py` defines three lists:
- **INCLUDE_TERMS**: house, techno, trance, dnb, dubstep, garage,
  hardcore, hardstyle, electro, edm, idm, ambient, downtempo, phonk,
  hyperpop, synthwave, reggaeton, afrobeats, dub, hip-hop, rap, drill,
  reggae, plus punk/metal/emo/screamo/post-punk/goth (per the
  user-explicit "don't exclude punk and metal per-se").
- **HARD_BLOCK_TERMS**: specific compounds where the include word is a
  modifier of a non-DJ noun (e.g. "garage rock", "indie rock",
  "blues rock", "country rock", "folk rock") — these override INCLUDE.
- **SOFT_BLOCK_TERMS**: broad listening genres only matched when no
  INCLUDE hit (jazz, blues, soul, country, folk, gospel, classical,
  rock, indie, ska, etc.).

Precedence: `HARD_BLOCK > INCLUDE > SOFT_BLOCK > NEUTRAL`. NEUTRAL is
default-excluded. Result on the 6291-genre everynoise scrape:
**2116 INCLUDE / 1920 BLOCK / 2255 NEUTRAL**.

### Sample budget

Default is **10 tracks per genre = ~21k tracks**. Justified by:
- Multi-task linear probes for k=12 axes train cleanly on ~1700 examples
  per axis (after dedup); 10/genre × 2116 = ~21k gives comfortable margin
- ~10 GB of audio at 470 KB/preview — fits anywhere
- Wall time ~1 hour end-to-end (search + download)
- Small enough that re-running with different categorisation rules is cheap

### When round-7 axis discovery runs

The corpus produced by `build_corpus.sh` is the input to round-7's
per-axis LLM tournament: for each candidate axis (distortion, density,
darkness, ...) run a Qwen3-Omni pairwise tournament *only on questions
about that axis*, then learn linear probes per axis, then jointly fit
the final blend. See round-7 plan in
`documents/aggression-axis-eval-round-7.md` for the full design.

When round 7 completes, results replace the placeholder in that doc and
the deployed axis migrates from V15 (single learned axis on Mesh's 909
tracks) to V16 (k learned axes from ~21k multi-genre tracks + learned
blend). Cross-library deployment hooks (round 8) will rely on round 7's
factorisation: the k axes ship as a ~36 KB pretrained blob, per-user
libraries refit only the k blend weights via the existing calibration
UI (no GPU needed user-side).

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

---

## Round 7.6 / V18 operator runbook

Round 7.6 is the multi-source LLM-jury intensity pipeline. Captions ←
Music Flamingo (audio→text). Intensity ← two text LLMs (local Mistral +
remote Spark2 Nemotron) on a fine 0–19 (20-bucket) scale. Multi-source
Dawid-Skene EM gives `consensus_intensity ∈ [0,1]` per track. A teacher
MLP (audio + caption + struct + r7.5 features → intensity + 16 axes)
distils to a 512-d student probe (audio-only, ships in V18.json).

`serve_text_llm.sh` is intentionally model-agnostic — caller picks the
model via `TEXT_LLM_MODEL` (no quietly-wrong default).

### Services in scope (RTX 5090 Mobile, 24 GB)

| Service | URL | Model | Notes |
|---|---|---|---|
| Music Flamingo (caption gen) | `:8001` | `nvidia/audio-flamingo-3` | Optional once captions are cached. ~14 GB bf16. |
| Local text LLM (juror A) | `:8002` | `gghfez/Mistral-Small-3.2-24B-Instruct-hf-AWQ` | True AWQ-Marlin, text-only, no Pixtral. ~14 GB. |
| Remote text LLM (juror B) | `172.16.54.147:8000` | `nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B-NVFP4` | Spark2; remote, no local GPU cost. |

### Start

```bash
# 0. Stop any GPU squatter (e.g. systemd llama-server) before vLLM:
systemctl stop llama-server-qwen36.service        # if it auto-respawns: --user mask first

# 1. Local Mistral juror (foreground or & ; logs to file).
TEXT_LLM_MODEL="gghfez/Mistral-Small-3.2-24B-Instruct-hf-AWQ" \
  bash spike/track-grading/serve_text_llm.sh \
  > /home/data01/Music/mesh-track-grading/logs/vllm_mistral.log 2>&1 &

# 2. (Optional) MF only when re-extracting captions; skip otherwise.
bash spike/track-grading/serve_music_flamingo.sh \
  > /home/data01/Music/mesh-track-grading/logs/vllm_mf.log 2>&1 &

# 3. Smoke pipeline (200 tracks, ~5 min total once services are up).
TEXT_LLM_MODEL="gghfez/Mistral-Small-3.2-24B-Instruct-hf-AWQ" \
TEXT_LLM_TAG=local \
  bash spike/track-grading/run_round7_6_pipeline.sh caption-rate
TEXT_LLM_URL="http://172.16.54.147:8000/v1/chat/completions" \
TEXT_LLM_MODEL="nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B-NVFP4" \
TEXT_LLM_TAG=remote_nemotron \
  bash spike/track-grading/run_round7_6_pipeline.sh caption-rate
bash spike/track-grading/run_round7_6_pipeline.sh v18-smoke
```

`v18-smoke` runs S2→S13 end-to-end and writes everything under
`*_smoke.*` paths. Drop `_smoke` and switch to `v18-train` for production.

### Monitor

```bash
# vLLM serve health + GPU
curl -sf http://localhost:8002/health && echo READY
nvidia-smi --query-gpu=memory.used,memory.free --format=csv | tail -1

# Live log tail (filtered so you don't drown)
tail -F /home/data01/Music/mesh-track-grading/logs/vllm_mistral.log \
  | grep -E "Application startup|Engine.*ready|ERROR|Failed core proc|out of memory"

# Caption-rate progress (count vs corpus)
ls /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/ | wc -l
```

### Stop

```bash
# vLLM (local) + any background spike script:
pkill -f vllm.entrypoints.openai
pkill -f caption_intensity_rating.py

# If GPU still busy after vLLM teardown, find culprit:
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv
```

### Validate after a smoke run

```bash
# Each stage's NPZ shape, range, distinct, raw-response sample
python3 -c '
import numpy as np
d = np.load("/home/data01/Music/mesh-track-grading/round7_6_caption_intensity_smoke_local.npz", allow_pickle=True)
print(f"N={len(d[\"track_ids\"])}  score=[{d[\"score\"].min():.3f},{d[\"score\"].max():.3f}]  "
      f"std={d[\"score\"].std():.3f}  distinct={len(np.unique(np.round(d[\"score\"],4)))}")
print("raw[:6]:", d["raw_first_token"][:6].tolist())
'
# (replace path to validate _remote_nemotron, consensus, teacher_metrics, etc.)
```

### Key findings logged this round

- **20-bucket scale wins.** Held-out CV ridge regression
  (audio_emb → score, 5-fold × 20 seeds, N=118) shows R² saturates at
  0.39 between b20 and b50; b5/b10 lose ~0.08 R² (clearly too coarse);
  b100 anchors to multiples of 5 and drops slightly. Locked in
  `caption_intensity_rating.py` as the production default. See
  `bench_resolution.py`.
- **`gghfez/Mistral-Small-3.2-24B-Instruct-hf-AWQ` is the working
  upload.** True AWQ-GEMM (vLLM auto-promotes to Marlin on SM120),
  text-only (no Pixtral processor), ~14 GB. Other public AWQ variants
  of this model are mislabelled `compressed-tensors` and produce
  gibberish (e.g. `jeffcookio/...-awq-sym`).
- **Cross-juror agreement ρ(local Mistral, remote Nemotron) = 0.944** on
  200 captions; mean delta 5.6%, max 24%. Different anchoring patterns
  (Mistral uses 12/14/15/17, Nemotron clusters at 13) — that's the
  decorrelation we want.
- **EM σ² collapse on smoke corpus.** With only 200/15314 tracks
  captioned, the multi-source EM gives all weight to the highest-
  reliability source (local Mistral, σ²≈0). Re-validate on a full-
  corpus run before drawing conclusions about juror weighting.
- **NixOS runtime quirks (resolved, kept here so future-you doesn't
  rediscover):**
  - vLLM 0.20.1 wheels bundle `triton/backends/nvidia/bin/{ptxas,
    ptxas-blackwell, nvdisasm, cuobjdump}` linked against generic
    glibc; won't run on NixOS without `nix-ld`. **Fix:** patchelf
    `--set-interpreter` to a `/nix/store/*glibc-2.4*/lib/ld-linux-x86-64.so.2`,
    `--set-rpath` to the matching glibc/lib. One-shot per venv;
    redo on every fresh `pip install`.
  - vLLM on Blackwell SM120 needs `--enforce-eager` because
    `torch.compile` + Triton can't always resolve `ptxas-blackwell`
    even after the patchelf fix. Eager mode skips the whole inductor
    path. Negligible perf cost for 1–2 token decode.
  - When invoking python tools *outside* the serve script, prepend
    `LD_LIBRARY_PATH="$(ls /nix/store/*zlib-1.3*/lib/libz.so.1 | sort -V | tail -1 | xargs dirname):$LD_LIBRARY_PATH"`
    or numpy's C-extensions can't find `libz.so.1`.
- **`caption_intensity_rating.py` atomic write was broken (now fixed).**
  `np.savez(tmp_path)` auto-appended `.npz` so `os.replace` failed.
  Fix uses a file handle. Every prior run silently lost the rename —
  recoverable from `*.npz.tmp.npz` files in the directory if you find
  one.
- **Music Flamingo serve needs `--skip-mm-profiling` on vLLM 0.20.1.**
  Without it, startup raises:
  `KeyError: 'MusicFlamingoProcessor output must include rote_timestamps.'`
  Cause: vLLM 0.20.1's MM-profiling pass calls the HF processor on dummy
  audio at startup and validates `rote_timestamps` in the output dict.
  transformers 5.7.0's processor doesn't emit that key on dummy runs,
  so the check fails before inference ever starts. Real audio requests
  go through a different code path and work fine. The flag is in
  `serve_music_flamingo.sh`; if the script ever drops it again, look
  here. NOT a transformers-version problem; do NOT install the
  lashahub fork.
- **Caption corpus is currently 200 tracks** (`round7_6_captions/
  music_flamingo/*.json`). Audio embeddings (`embeddings/
  corpus_muq_mulan.npz`) cover 15314 tracks, 118 of which are
  captioned. The V18 teacher trains on that 118-track intersection
  (93 train / 16 val / 9 test, artist-stratified). Production needs a
  much larger caption sweep before V18 is meaningful at scale.

### V18 smoke artefacts (this round, on disk)

```
/home/data01/Music/mesh-track-grading/
  round7_6_caption_intensity_smoke_local.npz                # juror A (Mistral)
  round7_6_caption_intensity_smoke_remote_nemotron.npz      # juror B (Nemotron)
  round7_6_consensus_smoke.npz                              # multi-source EM
  round7_6_teacher_smoke.pt                                 # teacher MLP
  round7_6_teacher_preds_smoke.npz
  round7_6_teacher_metrics_smoke.json                       # test_pa 0.6389, ρ +0.43
  round7_6_student_smoke.pt
  round7_6_student_metrics_smoke.json                       # student PA 0.28 (undertrained @20 ep)
  round7_6_eval_report_smoke.md
  round7_6_eval_smoke.json                                  # K-means K=5 themes
models/aggression-axes/V18_SMOKE_TEST.json                  # exported V18 (no-deploy)
```

---

## Round 7.6 / V18 — full-corpus production run (15,314 tracks)

The smoke covers 200 tracks. Production targets the entire 15,314-track
r7.5 corpus. Wall time is dominated by **Music Flamingo caption
generation (~7-8 hr)**; the rating + downstream (~1.5 hr combined) can
overlap if you rate against the remote Spark2 Nemotron while MF is
captioning. Local Mistral can't run concurrently with MF on the 24 GB
RTX 5090 (each is ~14 GB) — defer it to after MF stops.

### Phase 0 — pre-flight (once)

```bash
# Free GPU (kill anything that auto-respawns first):
sudo systemctl stop llama-server-qwen36.service
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv

# Confirm the remote Spark2 juror is reachable:
curl -sf -m 5 http://172.16.54.147:8000/v1/models \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"][0]["id"])'

# Make a logs dir if absent:
mkdir -p /home/data01/Music/mesh-track-grading/logs
```

### Phase 1 — Music Flamingo caption sweep (~7-8 hr)

```bash
# Start MF (port 8001).
bash spike/track-grading/serve_music_flamingo.sh \
  > /home/data01/Music/mesh-track-grading/logs/vllm_mf.log 2>&1 &
# Wait for /health to return 200 (~3 min cold start).
until curl -sf http://localhost:8001/health -o /dev/null; do sleep 10; done; echo "MF READY"

# In a SEPARATE terminal — full caption sweep + auto-embed + struct tags.
# Resumable: re-run after a crash and it'll skip captions that already exist.
bash spike/track-grading/run_round7_6_pipeline.sh caption-full \
  2>&1 | tee /home/data01/Music/mesh-track-grading/logs/caption_full.log
```

Expect ~0.6 captions/sec with `--max-tokens 192`. Monitor:

```bash
# Caption count progress (should rise toward 15,314):
ls /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/ | wc -l
# Live errors:
tail -F /home/data01/Music/mesh-track-grading/logs/vllm_mf.log \
  | grep -E "ERROR|Failed|out of memory"
```

### Phase 2 — Concurrent remote rating (overlaps Phase 1, ~46 min wall)

The remote juror runs on Spark2; it costs nothing locally and processes
captions in parallel with MF generating new ones. Run this in a third
terminal once Phase 1 has dropped at least a few hundred captions to
disk:

```bash
TEXT_LLM_URL="http://172.16.54.147:8000/v1/chat/completions" \
TEXT_LLM_MODEL="nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B-NVFP4" \
TEXT_LLM_TAG=remote_nemotron \
TEXT_LLM_NO_THINK=1 \
POLL_SECS=300 STABLE_SECS=1800 \
  bash spike/track-grading/run_round7_6_pipeline.sh caption-rate-streaming \
  2>&1 | tee /home/data01/Music/mesh-track-grading/logs/rate_remote.log
```

The streaming wrapper polls every 5 min; it exits when no new captions
appear for 30 min (so it lasts about as long as Phase 1). Output:
`round7_6_caption_intensity_remote_nemotron.npz` (resume-safe).

### Phase 3 — stop MF, start local Mistral, rate (~46 min)

After Phase 1 ends (caption count plateaus at 15,314 and the
`caption-full` command exits):

```bash
pkill -f "vllm.entrypoints.openai.*8001"
nvidia-smi --query-gpu=memory.used --format=csv | tail -1   # confirm free

TEXT_LLM_MODEL="gghfez/Mistral-Small-3.2-24B-Instruct-hf-AWQ" \
  bash spike/track-grading/serve_text_llm.sh \
  > /home/data01/Music/mesh-track-grading/logs/vllm_mistral.log 2>&1 &
until curl -sf http://localhost:8002/health -o /dev/null; do sleep 10; done; echo "Mistral READY"

TEXT_LLM_TAG=local_mistral \
  bash spike/track-grading/run_round7_6_pipeline.sh caption-rate \
  2>&1 | tee /home/data01/Music/mesh-track-grading/logs/rate_local.log
```

Output: `round7_6_caption_intensity_local_mistral.npz`.

### Phase 4 — V18 train end-to-end (~30 min)

Builds full-corpus consensus, trains teacher, distils student, runs
held-out eval, exports V18 JSON. The orchestrator's `v18-train` stage
auto-discovers every `round7_6_caption_intensity*.npz` (excluding the
`_smoke` variants) and registers them as jury sources:

```bash
bash spike/track-grading/run_round7_6_pipeline.sh v18-train \
  2>&1 | tee /home/data01/Music/mesh-track-grading/logs/v18_train.log
```

Final artefact: `models/aggression-axes/V18_round7_6_consensus_distilled.json`
(no-deploy — copy over `models/muq-mulan-aggression-axis.json` only after
reviewing `round7_6_eval_report.md`).

### Stop / interrupt safety

- Every long stage is **resume-safe**: `caption-full` skips captions
  already on disk; `caption-rate-streaming` skips track_ids already in
  the output NPZ. Crash and re-run — no work lost.
- Kill switches:
  ```bash
  pkill -f vllm.entrypoints.openai           # any vLLM serve
  pkill -f run_judge_caption.py              # MF caption gen
  pkill -f caption_intensity_rating.py       # rater (any juror)
  ```
- If VRAM stays busy after kill, find the squatter:
  `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv`

### Wall-time budget (one box, no parallelism with Phase 1)

| Phase | Duration | Concurrent? |
|---|---|---|
| 1 — MF caption sweep | ~7-8 hr | yes (1 + 2 overlap) |
| 2 — remote Nemotron rating | ~46 min (or as long as Phase 1) | yes |
| 3 — local Mistral rating | ~46 min | sequential (after Phase 1) |
| 4 — V18 train + distill + eval + export | ~30 min | sequential |
| **Total** | **~9-10 hr** | (Phase 1 dominates) |

### Validation pass after the production run

```bash
# Each juror's full output:
python3 -c '
import numpy as np
for tag in ("local_mistral","remote_nemotron"):
  d = np.load(f"/home/data01/Music/mesh-track-grading/round7_6_caption_intensity_{tag}.npz", allow_pickle=True)
  print(f"{tag:>18s}: N={len(d[\"track_ids\"])}  range=[{d[\"score\"].min():.3f},{d[\"score\"].max():.3f}]  std={d[\"score\"].std():.3f}  distinct={len(np.unique(np.round(d[\"score\"],4)))}")
'

# Consensus + EM weighting (the σ² collapse caveat from smoke should
# rebalance here — both jurors at full coverage):
python3 -c '
import numpy as np
d = np.load("/home/data01/Music/mesh-track-grading/round7_6_consensus.npz", allow_pickle=True)
print("sources:", [str(s) for s in d["source_names"]])
print("σ²:     ", [f"{x:.4f}" for x in d["source_sigma2"]])
print("rel:    ", [f"{x:.2e}" for x in d["source_reliabilities"]])
'

# Final teacher + student metrics:
cat /home/data01/Music/mesh-track-grading/round7_6_teacher_metrics.json | python3 -m json.tool | head -20
cat /home/data01/Music/mesh-track-grading/round7_6_eval_report.md | head -40
```
