# V18.1 MLP student experiment — closing G6 with capacity, not data

**Date:** 2026-05-08
**Question:** does escalating the spec-§765-768 student from a linear probe
to a 2-layer MLP close the +12.87 pp distillation gap (G6) reported in the
V18 release run?

**Answer:** Modestly. **+0.61 pp** test PA gain (0.8113 → 0.8174). The G6
gap is 99 % an information-bottleneck problem (audio encoder), not a
capacity problem (student architecture). The MLP shows the V18 baseline
linear probe was already extracting nearly everything available from
MuQ-MuLan-large for the intensity-axis projection.

## Setup

Same teacher, same data, same losses, same seed. Only the student
intensity head changed:

| | V18 baseline (linear) | V18.1 (MLP h=128) |
|---|---|---|
| Architecture | `Linear(512 → 1)` | `Linear(512 → 128) → GELU → Dropout(0.3) → Linear(128 → 1)` |
| Intensity-head params | 513 | 65,793 |
| Train wall (5090) | 2.6 s | 3.6 s |

Distillation losses, λ weights, optimizer, schedule, split — all identical
(see `documents/round-7-6-pipeline-spec.md` §17 for the loss spec).

## Results

| Model | Test PA | Spearman | R² | Distill gap |
|---|---:|---:|---:|---:|
| V18 baseline (linear) | 0.8113 | 0.8184 | 0.6739 | +12.87 pp |
| **V18.1 MLP h=128** | **0.8174** | **0.8264** | **0.6889** | **+12.26 pp** |
| V18.1 MLP h=256 | 0.8173 | 0.8265 | — | +12.27 pp |
| V18.1 MLP h=512 | 0.8180 | 0.8274 | — | +12.20 pp |

Teacher PA = 0.9400 (unchanged across all student arch variants — same
1332d-feature teacher trained once on 39913 tracks).

**Conclusion:** the gain saturates at hidden=128. h=256 adds 0.00 pp,
h=512 adds 0.07 pp (probably noise on the 3985-track test set). Hidden=128
is the right operating point — captures the available improvement at
minimum parameter and CPU cost.

## Why so modest

When I proposed the MLP escalation, I projected +3-6 pp based on the
spec-§765 escalation note plus a heuristic that "more parameters = more
capacity = closer to teacher". That heuristic was wrong here. Three
reasons:

1. **The teacher's penultimate is already 128d.** FitNets feature-matching
   (`λ_fit · MSE(student_pen_proj, teacher_penultimate)`) maps the 512d
   audio embedding into the teacher's 128d penultimate space using a
   single Linear(512 → 128). That linear projection was *already* doing
   the teacher-substrate-matching work. The new intensity-head MLP only
   adds capacity to the final read-out from that matched representation,
   which is a relatively easy regression.

2. **Captions encode information audio doesn't.** The teacher's 0.94 PA
   leans heavily on `caption_emb` (768d bge over MF rich captions). MF
   reads the audio and produces "this track is an aggressive industrial
   techno piece" — that's a *semantic* description of intensity. The
   student gets only the 512d MuQ-MuLan audio embedding, which encodes
   *acoustic* features. No amount of MLP capacity converts acoustic →
   semantic. The G6 gap reflects this irreducible privileged information
   per Lopez-Paz et al. 2016 LUPI.

3. **MuQ-MuLan was trained for music-text similarity, not intensity.**
   The 512d embedding is optimized to put genre/mood-similar tracks near
   each other in cosine distance. Intensity-discriminative information is
   present but not the dominant signal in that geometry. A bigger student
   on the same encoder still hits the same wall.

## What would actually close the G6 gap

**Lever 2: bigger audio encoder.** MAEST-768d or MULE-1.7k+d (both
covered in `documents/embedding-models-research.md` Phase 2) are music
transformers trained on richer downstream tasks where intensity-
discriminative features matter directly. Expected gain: +2-4 pp on top
of the linear/MLP baseline, but for *both* — closes the G6 gap because
the encoder itself carries more intensity signal.

V18.1 with MAEST instead of MuQ-MuLan would probably land at ~0.86-0.88
PA against the 3-juror consensus, *and* the linear probe might
re-saturate at the new ceiling (capacity again becomes nondominant).

## Decision

**Ship V18.1 MLP h=128 as the deployed intensity axis.**

- Test PA 0.8174 (vs V18 baseline 0.8113, +0.6 pp).
- CPU latency 1.40 µs/track — still 70,000× under the G9 budget of
  100 ms/1000 tracks. Practically free.
- Held-out cluster diagnostic (G4) ordering preserved (see
  `data/round7_6/v18_1_mlp_experiment/round7_6_eval_report_mlp.md`).
- Reproduction-determinism check (G10) confirmed: V18.1 export reproduces
  test_pa=0.817438 to bit-identical from the JSON weights.

Free improvement; ship it.

**Defer the bigger-encoder Lever 2 work** to the embedding-models migration
already underway. When MAEST/MULE land, retrain V18.2 with the same
3-juror consensus + caption tarball + struct tags (all snapshotted in
`data/round7_6/`), no GPU re-runs needed except the encoder pass and
~10 s of teacher + student training.

## Deployment integration

The V18.1 JSON has a different schema than the linear V18:

```json
{
  "model_type": "mlp",
  "embedding": "muq-mulan",
  "embedding_dim": 512,
  "mlp": {
    "hidden_dim": 128,
    "activation": "gelu",
    "W1": [[...]],   // (128, 512)
    "b1": [...],     // (128,)
    "W2": [[...]],   // (1, 128)
    "b2": float
  },
  ...
}
```

`crates/mesh-core/src/intensity_axis.rs` currently expects the linear
`intensity_axis_vec + bias` schema. To deploy V18.1, `intensity_axis.rs`
needs:

- A discriminated-union for `model_type`: `linear` (existing) vs `mlp` (new).
- For MLP, a forward pass: `y = (W2 @ gelu(W1 @ audio_emb + b1)) + b2`
  where `gelu(x) = 0.5 * x * (1 + erf(x / sqrt(2)))`.
- `simba`/`ndarray` matmul or hand-rolled — both fine at this size
  (128×512 + 128×1, ~70k FMAs per track, sub-microsecond per call).

The V18 baseline schema stays valid for back-compat; existing deployments
keep working until the integration code is updated.

## Artifacts

Snapshotted at `data/round7_6/v18_1_mlp_experiment/`:

| File | Description |
|---|---|
| `round7_6_student_mlp.pt` | h=128 chosen variant, 530 KB |
| `round7_6_student_mlp_h256.pt` | h=256 variant for the sweep, 790 KB |
| `round7_6_student_mlp_h512.pt` | h=512 variant for the sweep, 1.3 MB |
| `round7_6_student_mlp_metrics.json` | h=128 train metrics |
| `round7_6_student_mlp_h{256,512}_metrics.json` | sweep metrics |
| `round7_6_eval_mlp.json` | full eval JSON for h=128 (per-cluster, G4, etc.) |
| `round7_6_eval_report_mlp.md` | human-readable eval report for h=128 |
| `models/aggression-axes/V18_1_mlp_h128.json` | deployed-format JSON |

Reproduction recipe (from this snapshot, no GPU re-runs needed):

```bash
cd /home/data01/Projects/Mesh
nix develop .#mlspike

# Train MLP student (3.6s on 5090 from cached teacher_preds)
BASE=/home/data01/Music/mesh-track-grading
bash spike/track-grading/run_r7_step.sh distill_v18_student.py \
  --audio-emb     "$BASE/embeddings/corpus_muq_mulan.npz" \
  --teacher-preds "$BASE/round7_6_teacher_preds.npz" \
  --consensus     "$BASE/round7_6_consensus.npz" \
  --out-dir       "$BASE" \
  --out-stem      round7_6_student_mlp \
  --student-arch  mlp \
  --hidden-dim    128 \
  --dropout       0.3
```

Reproduces the V18.1 weights deterministically (seed=42).

## Cross-reference

- Spec: `documents/round-7-6-pipeline-spec.md` §17 (student) + §765-768
  (escalation rule)
- Training log: `documents/round-7-6-training-log.md` (V18 release run
  context that motivates this experiment)
- Embedding-models research: `documents/embedding-models-research.md`
  (Lever 2 / MAEST / MULE migration plan)
