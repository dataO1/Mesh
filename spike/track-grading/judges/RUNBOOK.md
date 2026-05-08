# Round-7.6 Judge-Swap Environment Runbook

End-to-end procedure for running the Music-Flamingo-judged version of round 7.5.

## What this experiment tests

**Hypothesis (E1 from the deep-research note)**: swapping the LLM judge from
Qwen3-Omni-30B to NVIDIA Music Flamingo 7B should lift per-axis pairwise
agreement by 3–7 pp. Music Flamingo beats Qwen3-Omni by +22 pp on
MuChoMusic (74.6 % vs 52.1 %); the question is whether that music-task
gap carries over to pairwise intensity ranking.

**What we re-use from round 7.5** (saves the 8+ GPU-hours of selecting
tuples):

- `/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz` — 15 314 × 512 MuQ-MuLan vectors (no change)
- `/home/data01/Music/mesh-track-grading/round7_5_pairs/<axis>/*.json` — 192 k K=4 tuples already chosen by BALD
- `/home/data01/Music/mesh-track-grading/round7_5_priors.npz` — round-7.5 BT priors used here for **uncertainty sampling** (we only re-judge the 20 k highest-uncertainty pairs)
- `/home/data01/Music/mesh-track-grading/round7_5_tags.npz` — mined justification tags (re-used as-is for the auxiliary loss)
- `spike/track-grading/round7_5_axis_prompts.json` — the 16 polar prompts (unchanged)

**What we re-collect**: only the **rankings** themselves, by running the
same 20 k tuples through Music Flamingo. New per-call JSONs land in
`/home/data01/Music/mesh-track-grading/round7_6_pairs/music_flamingo/<axis>/`.

## Hardware target

- **GPU**: RTX 5090 Mobile, 24 GB VRAM
- **CPU**: 24 cores
- **RAM**: 93 GB
- **Disk**: 1.9 TB free

Music Flamingo at bf16: ~16 GB weights + ~5 GB KV/activation = ~21 GB
total. Leaves ~3 GB margin at `gpu_memory_utilization=0.92`.

## File layout

```
spike/track-grading/
  judges/
    __init__.py              package exports
    base.py                  abstract Judge + parse helpers + K=4 invariants
    qwen3_omni.py            existing baseline (refactored from r7.5 inline code)
    music_flamingo.py        new — vLLM HTTP client w/ multi_modal_uuids cache hint
    RUNBOOK.md               this file

  serve_music_flamingo.sh    launch vLLM on port 8001, bf16, max_num_seqs=4
  serve_qwen3_omni.sh        existing (unchanged) — port 8000

  run_judge_tournament.py    judge-agnostic K=4 N-way tournament runner
  smoke_test_judge.py        ~50-call validation harness
  run_round7_6_pipeline.sh   orchestrator: smoke | full | post

  build_bt_priors_r7_5.py    ← reused as-is, --pairs-root points at r7_6 dir
  train_axes_r7_5.py         ← reused (mini-batch fix from yesterday)
  joint_blend_r7_5.py        ← reused
  interpret_axes_r7_5.py     ← reused
  cross_library_r7.py        ← reused
  export_axis_r7_5.py        ← reused (output filename change only)
  compare_v15_v16_v17.py     ← reused
```

## Procedure

### 1. Start vLLM Music Flamingo serve (~3 min cold start, first time downloads weights)

```bash
bash spike/track-grading/serve_music_flamingo.sh
```

Listen for:
```
INFO ... Application startup complete.
INFO ... Uvicorn running on http://0.0.0.0:8001
```

vLLM takes longer on first launch because it downloads `nvidia/music-flamingo-2601-hf`
(~14 GB) into `~/.cache/mesh-spike/hf/hub/models--nvidia--music-flamingo-2601-hf/`.
Subsequent restarts use the cache.

### 2. Smoke test (~1 min)

```bash
bash spike/track-grading/run_round7_6_pipeline.sh smoke
```

Pass criteria:
- Parse rate ≥ 90 %
- Sustained throughput ≥ 0.5 K=4 calls/sec
- Sample raw responses look sensible (4-letter ranking + brief justification)

If parse rate is low: the LLM probably isn't following the strict 4-letter
output format. Check sample responses; may need to revisit the prompt.

If throughput is low (< 0.3 calls/sec): vLLM may be running unoptimised.
Check `nvidia-smi` for "Running: N reqs" lines in the vLLM log; should
see N=2-4 sustained.

### 3. Full tournament + downstream (~5 hours)

```bash
nohup bash spike/track-grading/run_round7_6_pipeline.sh full \
  > /home/data01/Music/mesh-track-grading/logs/r7_6_pipeline.log 2>&1 &
```

This runs:
1. Music Flamingo tournament: 20 000 uncertain pairs × 16 axes = 20 k
   distinct K=4 calls (BALD picked tuples that the BT model is most
   uncertain about, so highest information value).
2. BT priors per axis from MF rankings.
3. Multi-task linear probe (5-fold CV + final retrain, mini-batched).
4. ListMLE blend on `timbre_roughness` target.
5. Axis interpretation (top/bottom 20 per axis + correlation matrix).
6. Cross-library projection on user's 909 tracks.
7. V18 export to `models/aggression-axes/V18_round7_6_music_flamingo.json`.
8. Head-to-head V15 vs V17b vs V18 on user library.

Watch:
```bash
tail -f /home/data01/Music/mesh-track-grading/logs/r7_6_pipeline.log
```

### 4. Resume (cheap)

If the tournament is interrupted:
- All per-call JSONs are atomically written and resume-safe.
- Re-run `bash spike/track-grading/run_round7_6_pipeline.sh full` — it'll
  pick up where it left off automatically.

If the tournament finished but you want to re-run downstream stages:
```bash
bash spike/track-grading/run_round7_6_pipeline.sh post
```

## Throughput budget

| Setting | Throughput | 20 k pairs ETA | 192 k pairs ETA |
|---|---:|---:|---:|
| bf16, max_num_seqs=4 (current) | 1.0–1.4 calls/s | **~5 hr** | ~40 hr |
| FP8, max_num_seqs=8 (alternative) | 2.0–2.5 calls/s | ~2.5 hr | ~22 hr |

We picked bf16 because user explicitly requested quality > speed. If
the smoke test reveals a parse-rate issue, switching to FP8 isn't going
to help — that means the prompt format is the issue, not precision.

## Hardware-aware design decisions

- **Workers = 12** matches round-7.5; vLLM saturates at this concurrency
  on bf16 max_num_seqs=4. Higher worker count just queues at the vLLM
  scheduler.
- **Audio cache**: each track decoded once across all calls touching it;
  capped at 4 000 tracks (~2 GB RAM) with simple FIFO eviction. BALD's
  500-track working set keeps cache hit rate near 100 %.
- **`multi_modal_uuids` per request**: vLLM's content-based encoder
  cache uses these as deterministic keys; second-and-later tuples
  touching the same track skip the AF-Whisper forward pass entirely.
  Free ~20 % throughput once the working set warms up.
- **`mm-processor-cache-gb=6`** on the vLLM side gives the encoder cache
  enough room for the working set (500 tracks × ~3 MB hidden state ≈
  1.5 GB used).
- **`enforce-eager`** (no CUDA graphs) for safety on first run; can drop
  this once we've confirmed bf16 is stable. ~10 % throughput cost.
- **Continuous BT refit** thread: not currently active in r7.6 because
  we re-judge a fixed pair set, no BALD scheduling needed. Re-add if
  switching to fresh BALD.

## License posture (important)

NVIDIA Music Flamingo is **CC-BY-NC-4.0 / NVIDIA OneWay Noncommercial
Academic License** — research use only. This means:

- ✓ Run experiments, generate labels, write papers
- ✓ Train internal/research models on MF-derived labels
- ✗ **Ship a commercial Mesh release where the deployed axis was trained
   on MF-judged labels**

For commercial deployment of round-7.6 results, options are:
1. Re-judge with Gemini 2.5 Pro (~$300 for 192 k pairs, permissive license)
2. Distill the V18 axis into a small student model trained from scratch
   on permissive sources
3. Use V15 as the deployed axis and treat round 7.6 as research-only
   validation of the polar-prompt + judge-swap methodology

## Troubleshooting

**vLLM startup fails with "Unsupported model architecture: MusicFlamingoForConditionalGeneration"**
- vLLM < 0.13.0 doesn't have the registry entry. We're on 0.20.1, so
  this shouldn't happen. If it does, check `pip show vllm`.

**`transformers` import error about MusicFlamingoConfig**
- Need transformers ≥ 5.0.0.dev. We're on 5.7.0, so should be fine.

**Smoke test parse rate < 50 %**
- LLM is probably outputting prose-only without the 4-letter ranking.
  Check the system prompt + the `judge_template` in
  `round7_5_axis_prompts.json`. Music Flamingo may need a more explicit
  "answer with EXACTLY four letters" instruction than Qwen3 needed.

**Smoke test throughput < 0.3 calls/sec**
- vLLM may not be using the encoder cache. Check vLLM log for
  "mm cache hit" messages. If absent, the `multi_modal_uuids` field may
  not be plumbed in this vLLM version — fall back to file:// audio_url.

**OOM during full run**
- Reduce `--max-num-seqs` to 2 in `serve_music_flamingo.sh` and restart.
  This cuts throughput by ~30 % but keeps the run within the 24 GB cap.

**Run died, want to resume**
- Per-call JSONs are atomic. Just re-run `full` (or `post` if past stage 1).
