# Round-7 autonomous run — final data-backed summary

**Status**: Pipeline ran end-to-end, no manual intervention beyond the
download phase that you re-kicked. All outputs committed and reproducible.

## Headline numbers

| Variant | Spearman vs round-5 BT | Pairwise Agreement | Where trained |
|---|---:|---:|---|
| V11 (text-tower 6-axis, **previous baseline**)  | +0.39 | 62.5 % | hand-named axes |
| **V15 (round-6 single linear probe)** — **deployed** | **+0.60** | **76.5 %** | user's 909 tracks (in-domain) |
| V16 (round-7 12-axis blend) — exported, **NOT deployed**  | +0.50 | 71.7 % | Deezer 1424 tracks (out-of-domain) |

Spearman(V15, V16) on user library = **+0.872** (high agreement; same
underlying perceptual dimension, different training corpora).

## What I changed and what I shipped

- **Kept V15 deployed.** `~/Music/mesh-collection/muq-mulan-aggression-axis.json`
  is unchanged. V15 still wins on your library because it was trained
  on your library. The migration to V15 you completed earlier is intact.
- **Exported V16 at `models/aggression-axes/V16_round7_blend.json`.**
  Schema-validated, unit-norm `intensity_axis_vec`, 12 L2-normalised
  `sub_axes`. Drop-in compatible with the IntensityAxis runtime if you
  ever swap it in. 208 KB on disk (V15 was 114 KB; the extra 94 KB is the
  12 sub-axis directions for round-8 use).

## Build pipeline (~1.5 hr autonomous wall)

| Phase | Output | Wall time |
|---|---|---:|
| 0. Corpus already on disk | 15 314 MP3s × 30 s = 6.9 GB | (yours) |
| 1. MuQ-MuLan embedding | 15 314 × 512 fp32 NPZ (32 MB) | 17.5 min |
| 2. vLLM Qwen3-Omni serve startup | endpoint at :8000 | ~2 min |
| 3. 12-axis tournament | 12 × 1500 directed pairs = 18k judgments, 0 fail | ~53 min |
| 4. BT priors per axis | 1424 unique tracks × 12 axes, all converged ≤34 iter | ~30 s |
| 5. Multi-task linear probes | 12 × 512-d directions, 5-fold CV + final retrain | ~1 min |
| 6. ListMLE blend | 12-d softmax, 2000 ep × 32 batches | ~30 s |
| 7. Interpretation + cross-library + V15-vs-V16 + export | round7_*.md + V16.json | ~10 s |

## What round-7 found (axis discovery result)

The 12 candidate axes are heavily redundant in MuQ-MuLan space. **22
pairs of axes correlate at |r| ≥ 0.85**, the worst being:

- `aggression` ↔ `distortion`: r = +0.991 (essentially the same axis)
- `aggression` ↔ `tempo_intensity`: r = +0.952
- `aggression` ↔ `vocal_intensity`: r = +0.935
- `density` ↔ `tempo_intensity`: r = +0.947

The ListMLE blend optimiser zeroed out 7 of the 12 weights and kept just
**5 effectively-distinct axes**, weighted as:

| Surviving axis | Weight | What it captures |
|---|---:|---|
| `darkness`            | 0.323 | minor key + low-freq emphasis + brooding mood |
| `vocal_intensity`     | 0.213 | screaming/growling/shouted vocals |
| `density`             | 0.199 | layered busy texture |
| `dynamic_compression` | 0.190 | brick-wall mastered, no quiet-loud-quiet |
| `rhythmic_complexity` | 0.075 | breakbeat / polyrhythm vs four-on-floor |

Top examples per axis (sanity-checked, all read correctly):

- **aggression top**: The Prodigy *Ibiza*, Mandidextrous, Tymon, hardstyle, hardcore
- **aggression bottom**: Diana Krall, Mary J. Blige, Michael Bublé, Sabrina Carpenter
- **darkness top**: Cryobiosis, Anenzephalia, Scorn, Merzbow (dark ambient + harsh noise)
- **vocal_intensity top**: Kill Your Idols, Korpiklaani, Death Threat, Sniper 66 (hardcore punk + folk metal screaming)
- **rhythmic_complexity top**: Tim Reaper, Nebula II, Ram Trilogy (jungle/breakbeat)
- **dynamic_compression top**: Kamiyada+, Tymon, Psyko Punkz (modern brick-walled mixes)

So the per-axis directions *are* meaningful — they just collapse into
"general energy" for tracks that are aggressive on multiple dimensions
at once.

## What didn't work and why

The plan target was 80–85 % pairwise agreement. We hit **71.7 %** (V16
on user library). The plan goal was overoptimistic — it assumed the
LLM judge could distinguish 12 fine-grained axes consistently. In
practice, on 30 s Deezer previews:

1. **Per-axis tournament has a 70 % accuracy ceiling.** Each axis's
   stand-alone probe trained on its own BT priors caps at 66.4–69.6 %
   CV pairwise agreement. That's the LLM-judge noise floor for a
   512-d MuQ-MuLan embedding mapped through a Linear(512, 1) probe.
2. **Multi-task blending doesn't beat that ceiling** because the axes
   are correlated → they compete for the same signal in 512 dims.
3. **Cross-domain is harder than in-domain.** V15 trained on the user's
   library hits 76.5 % PA on the user's library; V16 trained elsewhere
   hits 71.7 %. ~5 pp drop is the cross-library cost.

What *would* push past 70 %: human listening-test labels (much higher
SNR than LLM judgments), or audio embeddings trained directly on a
labelled aggression dataset rather than CLAP-style contrastive.

## Recommendation

Three concrete next moves, ordered by ROI:

1. **Round 8 (productisation, sub-axis UI).** V16 is the entry point.
   Per-collection deployment becomes "load V16, run a 100-pair pairwise
   tournament on the new library, refit just the 12 blend weights"
   instead of retraining the whole probe. The runtime already loads
   `sub_axes` so per-axis sliders ("more dark / more rhythmic") would
   need only UI work + a small blend-fit tool. No new ML.
2. **If you want > 70 % PA**: add ~200 of your own ground-truth pairwise
   labels (15 min of clicking) and retrain V15 with the augmented
   labels. That gets you 1–2 pp PA per ~100 labels, capped at maybe
   80 % before saturation.
3. **Don't pursue further LLM-judge axis discovery** without a stronger
   judge. The Qwen3-Omni 30B AWQ judge has plateaued at 70 % per-axis
   accuracy in this regime — adding more axes or more pairs won't
   change that.

## Where to find everything

- This summary: `documents/round-7-summary.md`
- Full round-7 doc with method + results: `documents/aggression-axis-eval-round-7.md`
- Per-axis top/bottom 20 + correlation matrix: `/home/data01/Music/mesh-track-grading/round7_interpretation.md`
- V15 vs V16 + per-axis projection on your library: `/home/data01/Music/mesh-track-grading/round7_cross_library.md`
- Deployable axis: `models/aggression-axes/V16_round7_blend.json`
- All scripts: `spike/track-grading/{embed_corpus_mulan,run_per_axis_tournaments,build_bt_priors_r7,train_axes_r7,joint_blend_r7,interpret_axes_r7,cross_library_r7,compare_v15_v16,export_axis_r7,run_r7_step}.{py,sh}`
- Logs: `/home/data01/Music/mesh-track-grading/logs/{embed,vllm,tournament}.log`
- Raw artefacts (npz/json reusable for future blends): `/home/data01/Music/mesh-track-grading/`
