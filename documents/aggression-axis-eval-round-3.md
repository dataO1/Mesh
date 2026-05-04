# Aggression-axis evaluation, round 3 — full library LLM-derived priors

Companion to `documents/aggression-axis-eval-round-2.md` and the round-2
notes in `documents/muq-mulan-integration-open-questions.md`.

## TL;DR

**The hypothesis was: better, track-level priors will lift the V11 Spearman
ceiling from ~+0.36 (against hand-priors) toward ~+0.6, telling us whether
the polar axis is capturing intensity correctly.**

**Result: the LLM (Audio Flamingo 3) couldn't produce a useful intensity
ranking for our DnB-heavy library. Of 909 tracks, 970 (97%) scored 7 or 8.
Only 16 tracks landed below 7. That's a near-binary classifier, not a 0-10
scale.** The Spearman of all 13 variants against the LLM priors *dropped*
from the hand-prior baseline — not because the variants got worse, but
because the LLM ruler is worse than the human ruler.

We can't conclude "polar axis is fine" or "polar axis is broken" from this
data. The next-step recommendation pivots: rather than trying another
captioner, return to expanding hand-priors via pairwise comparisons (faster
than absolute scoring) and keep V11 as the default.

## What we built (committed)

- **Audio Flamingo 3 captioner pipeline** — `nix run .#grade-tracks`
  (Nix wrapper at `nix/apps/grade-tracks.nix` + Python at
  `nix/apps/grade-tracks/grade.py` and `spike/track-grading/grade.py`).
  Reads track list via `dump_track_list` (new Cargo bin in
  `crates/mesh-cue/src/bin/dump_track_list.rs`), decodes 30 s of audio
  centered around `tracks.drop_marker`, runs AF3 with a single-line
  text-format prompt, parses `INTENSITY: N | GENRE: X | NOTES: Y`.
  Resumable via per-track JSON files in `/tmp/track-grading/<id>.json`.
  Aggregates to `documents/axis-eval-results/llm-priors.csv` and
  `llm-grading-raw.jsonl`.

- **Throughput**: 909 tracks in 22.6 minutes on RTX 5090 Laptop (24 GB).
  ~0.68 tracks/s including model load.
  Zero failed inference, zero parse errors.

## Eval results

### LLM-prior distribution (the smoking gun)

```
intensity  4:    1
intensity  5:    6   █
intensity  6:    9   █
intensity  7:  301   ████████████████████████████████████████████████████████████
intensity  8:  580   ██████████████████████████████████████████████████████████████████████████████████████████████████████████████████
intensity  9:   10   ██
intensity 10:    2
total:        909
```

**This is the problem.** Of 909 tracks, AF3 placed 881 (97%) at intensity 7
or 8. The bottom (1-3) and top (10) are both essentially empty. There is
no useful resolution at the extremes where intensity scoring matters most
for "make this set chiller / harder" suggestions.

For comparison, the round-2 hand-priors (47 anchors) spanned 2.5 → 9
relatively uniformly.

### Spearman scoreboard against LLM priors (909 anchors)

| Rank | Variant | Spearman ρ | vs hand-priors (round 2) |
|---|---|---|---|
| 1 | V4_blend_equal_3 | +0.274 | +0.331 (round 2) |
| 2 | V3_pure_density | +0.262 | +0.044 (huge swing) |
| 3 | V5_aggression_led | +0.241 | +0.319 |
| 4 | V6_five_axis_weighted | +0.239 | +0.372 |
| 5 | V9_v6_with_atonal | +0.226 | +0.364 |
| 6 | V10_balanced_six_axis | +0.216 | +0.363 |
| 7 | V12_peak_techno_tuned | +0.212 | +0.358 |
| 8 | V2_pure_distortion | +0.208 | +0.435 (best in round 2) |
| 9 | V7_dark_noisy_emphasis | +0.200 | +0.360 |
| 10 | V1_pure_aggression | +0.197 | +0.286 |
| 11 | V8_v7_with_distortion_bump | +0.193 | +0.372 |
| 12 | V11_neuro_dnb_tuned | +0.190 | +0.358 (current default) |
| 13 | V13_distortion_atonal_dominant | +0.189 | +0.353 |

Every variant scored *worse* against LLM priors than against hand-priors.
The leaderboard reordered slightly, but the spread is tighter (+0.19 to
+0.27 vs +0.36 to +0.43 in round 2) — consistent with a degenerate
ground-truth signal.

V3_pure_density jumped from last (+0.044, round 2) to #2 (+0.262, round
3). That's because density correlates with the LLM's "all DnB at 7-8" bias
— DnB tracks are dense, so V3's density-only ranking happens to match the
LLM's collapsed distribution by accident. **Not a sign V3 is good.**

## Smoke-test diagnostic

Eight smoke iterations on 5–18 anchor tracks during prompt development:

| Run | Prompt | Mean \|Δ\| vs hand-priors | Pathology |
|---|---|---|---|
| 5 | JSON schema with examples | parse_error on all 5 | Model parroted exemplar text verbatim |
| 6 | Lean schema, no examples | (5 tracks) Charlotte 7→8, Hyper 8→9 mostly within ±1 | Butternuts hallucinated as hard-techno (off by 3) |
| 7 | Detailed prompt with "warm jazzy" vocabulary | 3.78 | Anchored on "liquid DnB → 2", scored everything 2 |
| 8 | Lean prompt + "different tracks should get different scores" | 1.67 | Systematic 7-8 bias on all DnB tracks |

The "best" prompt (run 8 = run 6 + one nudge) still showed strong DnB-bias.
That bias scaled from 18 anchors to the full 909 — and got worse. The
model's intensity vocabulary is just narrow.

## What this tells us

### About AF3 specifically

AF3 was trained on general audio understanding (speech, environmental
sound, music). Its music distribution skews Western pop/rock and
introduces strong genre-categorical priors. When asked to numerically
rate intensity:

- It anchors on coarse genre buckets ("DnB" = 7-8, "deep house" = 5,
  "ambient" = 1)
- Within a bucket, it doesn't distinguish much (Pythius brutal-neuro and
  Random Movement liquid-funk both got 7-8)
- It hallucinates genre when asked to score (Butternuts described as
  "DnB, hard techno" instead of liquid funk)

This isn't a prompting problem. We tried JSON schema, flat-text format,
example-anchored, scale-anchored, with and without temperature.
The differentiation just isn't there in AF3.

### About the polar axes

Inconclusive. The LLM ruler is too compressed to distinguish good axes
from bad. But two soft signals:

1. **V11 (current default) ranked #12** out of 13 against LLM priors.
   But V11 was tied for top of V6/V7 lineage in round 2 (against
   hand-priors). Without a better ruler, we can't tell whether V11 is
   genuinely off or whether the LLM happens to disagree with V11 in
   ways that don't matter musically.
2. **V3 jumped to #2** against LLM priors after being last in round 2.
   That's a bad sign for the LLM ruler — V3 was definitively bad in
   listening tests (Switch Disco "React" at rank 305 of 883) and the
   only way it can rank highly against LLM priors is if the LLM also
   makes the same mistakes V3 makes (i.e. classifies high-density
   tracks as intense regardless of harshness).

## Recommendation

**Stop trying to use LLM scoring as ground truth for our intensity
metric.** Two viable paths forward, in priority order:

### Option A (cheap, recommended): pairwise calibration as the real ruler

Re-enable the calibration UI in **eval-only mode** (collect pairs, never
write weights). Each pair = "user judged A more intense than B". 200-300
pairs = enough for a Bradley-Terry-derived global ranking that's the
honest ground truth.

We already have:
- `aggression_calibration_pairs` schema (in DB)
- The modal UI (currently disabled)
- The `compute_pair_agreement` helper (`crates/mesh-core/src/suggestions/aggression.rs:312`)

Need:
- Re-enable the UI but strip its `store_aggression_weights` calls
- Extend `compare-variants.py` to handle pairwise data + Bradley-Terry
  conversion
- Spend ~2 hours doing 200 pairs

This is the most reliable next step. The pair-based ruler is what we
should have built first.

### Option B (expensive, deprioritized): try a different captioner

Music Flamingo (~36 GB FP16, no quantized release as of May 2026) is the
purpose-built music captioner — genuinely tuned for fine-grained music
analysis. AF3 was a stand-in. Music Flamingo would likely produce a
better intensity distribution but:
- Won't fit in 24 GB without aggressive quantization (none exists yet)
- Research-only license (same as AF3, fine for our use)
- Adds another GPU-hour and another model-class learning curve

Defer until we've tried Option A.

### Option C: fall back to hand-priors (status quo)

The 47-anchor hand-priors ARE actually a better ground truth than the
909-anchor LLM-priors, despite being smaller. Round 2's V11 + +0.358
Spearman is the best honest measurement we have. Ship V11 as the default
(already done), accept the +0.36 ceiling, move on to other features.

## Don't go for Option 3 (LLM-labeled heads) yet

The original gap-filling argument for LLM-labeled heads was: *"track-level
labels would let a small head learn intensity per-track instead of per-
artist"*. But AF3's labels are NOT track-level — they're effectively
genre-bucket labels. Training a regression head on those gives us a
2-class classifier dressed up as a regression: not the calibrated 0-10
scalar we wanted.

If we ever revisit this:
- Use Music Flamingo (when 4-bit is available) or Qwen3-Omni-Instruct
  (different bias profile)
- Or use multi-modal pairwise judgments (compare two clips, judge which
  is more intense) — sidesteps the absolute-scoring failure mode
  entirely

## Files / artifacts

- `documents/axis-eval-results/llm-priors.csv` — committed, 909 rows
- `documents/axis-eval-results/llm-grading-raw.jsonl` — committed, full
  per-track JSON (intensity + genre + notes + raw_response + drop_marker)
- `nix/apps/grade-tracks.nix` + `nix/apps/grade-tracks/grade.py` — the
  pipeline. `nix run .#grade-tracks` to re-run anytime.
- `spike/track-grading/grade.py` — same script in the spike location
- `crates/mesh-cue/src/bin/dump_track_list.rs` — Rust→CSV bridge
- `nvidia/audio-flamingo-3-hf` — model in HF cache
  (`~/.cache/mesh-spike/hf/`), ~16 GB

## Cross-references

The Option A recommendation here was implemented across rounds 4 and 5
with vLLM + Qwen3-Omni instead of the user-driven calibration UI:

- [Round 4](aggression-axis-eval-round-4.md) — first pairwise pipeline
  via vLLM Qwen3-Omni-30B-AWQ, anchored tournament, 5446 directed pairs
- [Round 5](aggression-axis-eval-round-5.md) — community-aware + active
  learning. BT-vs-hand Spearman improved from +0.36 → +0.41; saturated
  tracks dropped 282 → 83.
- [Round 6 plan](aggression-axis-eval-round-6.md) — first attempt to
  break the 63% pairwise-agreement ceiling via a learned head.
- [Round 7 plan](aggression-axis-eval-round-7.md) — LLM-supervised axis
  discovery, the answer to whether the polar axes are correct.
