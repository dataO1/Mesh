# Aggression axis via MuQ-MuLan text tower polar prompts — plan

Status: design, ready to implement. No branch cut yet.
Branch parent: `muq-mulan-integration` head (or `main` after merge).

## TL;DR

Replace the broken `compute_aggression_weights` Pearson-fit (currently
~50 % accuracy because its label source — `compute_track_aggression(genre)` —
returns 0 for every track now that MAEST genres are gone) with a
**precomputed 512-d aggression direction vector** derived from MuQ-MuLan's
text tower at design time. Per-track score becomes one dot product against
the audio embedding, identical math to today, completely different
provenance.

End-user changes: zero. Player runtime: identical (one dot product).
Schema: zero changes. Aggression axis goes from "calibrated against an
empty signal" to "anchored in the joint audio-text space the model was
trained on."

## Premise — why this works

MuQ-MuLan is a CLIP-style dual encoder. Audio tower output and text
tower output land in the **same** 512-d unit hypersphere. Cosine
similarity between an audio embedding and a text embedding answers
"how much does this track sound like that text describes."

So if we encode two polar prompts:

- `T_pos = text_tower("aggressive heavy distorted hard techno with pounding kicks")`
- `T_neg = text_tower("calm peaceful ambient drone with soft pads")`

their difference defines a direction in the embedding space:

```
D = (T_pos - T_neg) / ||T_pos - T_neg||
```

For any track with audio embedding `A_i ∈ S^511`, the projection
`A_i · D` is a scalar in roughly [-1, 1] that **measures aggression
exactly as MuQ-MuLan understood it during training**. No supervised
labels, no calibration pairs, no per-library fit. It's a constant of the
model.

This is a known shortcut from the CLIP literature ("text-anchored
linear probes") — works because contrastive pretraining shapes the
joint space such that semantic axes are linear directions, not
non-linear manifolds. MuQ-MuLan's training corpus included DJ-relevant
text descriptions (genre, mood, instrumentation) so the axis is
expected to be musically meaningful out of the box.

## Why no text tower ships to the user

The polar prompts are **fixed design-time data**. Once `T_pos` and
`T_neg` are computed on the dev box, the result is a single 512-d
vector. Shipping it is shipping ~2 KB of floats. The text tower itself
never runs at user side.

What this saves us:
- No second ONNX export (text tower has XLM-Roberta + custom transformer
  → exportable, but adds another 500–800 MB artifact).
- No tokenizer in Rust (XLM-Roberta uses sentencepiece — would need
  `tokenizers` crate + the `.spm` model file).
- No HuggingFace dependency at runtime.
- No per-user computation on first launch.

The cost: prompt iteration requires running the dev script and
re-shipping the axis vector. That's a one-time activity, not user-facing.

## Architecture overview

```
DEV BOX (one-time, gated by Nix app)
─────────────────────────────────────
nix run .#derive-aggression-axis
  ├─ load MuQ-MuLan from HF cache (already there from audio export)
  ├─ tokenize + run text tower over N polar prompt pairs
  ├─ average each polar group → mean_pos, mean_neg
  ├─ axis = (mean_pos - mean_neg) / ||...||
  └─ write models/muq-mulan-aggression-axis.json
        { "axis": [f32; 512],
          "prompts_pos": [...],
          "prompts_neg": [...],
          "model": "OpenMuQ/MuQ-MuLan-large",
          "generated_at": "..." }

GIT
───
commit models/muq-mulan-aggression-axis.json (~3 KB)


USER BOX (every launch, zero new dependencies)
──────────────────────────────────────────────
mesh-cue / mesh-player startup:
  ├─ MlModelManager locates muq-mulan-aggression-axis.json next to ONNX
  ├─ parse JSON → static AGGRESSION_AXIS: Vec<f32>
  └─ aggression scoring path:
       project_aggression(audio_embed, &AGGRESSION_AXIS) → f32
```

## Pipeline detail (dev side)

### 1. Prompt design

Single-pair vs averaged-pair:
- Single pair (one positive, one negative) is the simplest. Vulnerable
  to whichever phrasing happened to land best in the encoder.
- **Averaged pair (N=5–10 each side)** is recommended. Averages out
  prompt-vocabulary noise; the resulting axis converges to "what these
  prompts mean *on average*" rather than "what this exact sentence
  triggered."

Concrete starter prompts (techno/DnB-leaning since that's the user's
catalog — broadens later if needed):

POSITIVE (high aggression):
```
- "aggressive heavy distorted hard techno with pounding kicks"
- "harsh industrial noise with abrasive screeching textures"
- "fast intense gabber kicks with overdriven distortion"
- "hard-hitting drum and bass with menacing bass and ripping snares"
- "brutal dark techno with crushing reese bass and metallic stabs"
- "aggressive hardcore with relentless 4/4 kicks and screaming leads"
- "intense peak-time techno with driving hypnotic energy"
- "heavy distorted electronic music with violent dynamics"
```

NEGATIVE (low aggression):
```
- "calm peaceful ambient drone with soft warm pads"
- "gentle introspective downtempo with delicate piano"
- "slow meditative chillout with airy ethereal textures"
- "soft minimal techno with sparse warm bass and clean kicks"
- "smooth deep house with mellow chords and relaxed groove"
- "relaxed lo-fi beats with dreamy atmospheres"
- "tranquil soundscape with floating tones and no percussion"
- "warm soothing electronic ambient with no harshness"
```

Provenance principle: store the actual prompt strings *in the axis
artifact* so the axis is fully reproducible from the JSON alone. Anyone
re-running the script with the same prompts on the same model gets the
same axis (modulo float precision).

### 2. Text-tower extraction script

`nix/apps/derive-aggression-axis/derive.py` (new). Mirrors the structure
of `convert-muq-mulan/export.py` — same MuQ load, same Nix wrapper.

```python
from muq import MuQMuLan
import torch, json, time
from pathlib import Path

POSITIVE_PROMPTS = [...]  # see above
NEGATIVE_PROMPTS = [...]

def main():
    mulan = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large").eval()
    if torch.cuda.is_available():
        mulan = mulan.cuda()

    with torch.no_grad():
        # extract_text_latents handles tokenization + forward + l2norm
        pos = mulan(texts=POSITIVE_PROMPTS)  # (N_pos, 512), unit-norm
        neg = mulan(texts=NEGATIVE_PROMPTS)  # (N_neg, 512), unit-norm

    mean_pos = pos.mean(dim=0)              # (512,) — NOT unit-norm
    mean_neg = neg.mean(dim=0)
    raw_axis = mean_pos - mean_neg          # direction
    axis = raw_axis / raw_axis.norm()       # unit-norm direction

    out = {
        "axis": axis.cpu().tolist(),
        "prompts_positive": POSITIVE_PROMPTS,
        "prompts_negative": NEGATIVE_PROMPTS,
        "model": "OpenMuQ/MuQ-MuLan-large",
        "embedding_dim": 512,
        "method": "polar-prompt-averaged-difference",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    Path("models/muq-mulan-aggression-axis.json").write_text(json.dumps(out, indent=2))
```

GPU not strictly required — text tower is small (XLM-Roberta-base +
8-layer transformer + linear). CPU run takes a few seconds. Reuses the
HF cache the audio converter already populates, so no extra download.

### 3. Nix wrapper

`nix/apps/derive-aggression-axis.nix`. Trivial copy of
`convert-muq-mulan-model.nix` — same Python/cuda env (or a
CPU-only variant), runs `derive.py`. Output writes to `models/`. Same
zlib/LD_LIBRARY_PATH workarounds we already had to add for the audio
exporter.

Invocation: `nix run .#derive-aggression-axis`. Run once when prompts
change, commit the resulting JSON.

### 4. Artifact storage

Two viable shapes:

**A — sidecar JSON next to the ONNX** (recommended).
- Path: `models/muq-mulan-aggression-axis.json` in the repo, also
  uploaded to the GitHub `models` release alongside the ONNX +
  `.norm.json`.
- mesh-cue's `MlModelManager` already searches `models/`,
  `<exe>/models/`, `~/.cache/mesh-cue/ml-models/` — add the axis
  JSON to its required-files list, mirror the install_to_cache flow.
- Pro: prompt iteration doesn't require a Mesh rebuild. User can
  sideload an axis variant if they want.
- Con: one more file to track, one more "missing artifact" failure
  mode.

**B — embedded in binary via include_bytes!.**
- `static AGGRESSION_AXIS_JSON: &[u8] = include_bytes!("../../../models/muq-mulan-aggression-axis.json");`
- Pro: bulletproof — no I/O, no missing-file path, no install dance.
- Con: prompt iteration requires Mesh rebuild and re-release.

Recommendation: start with **A**. Same pattern as `.norm.json` already
ships, and it lets us iterate prompts on a slow loop without spinning a
Mesh release per attempt. If we ever decide to lock the axis as a
versioned model constant, switch to B in one commit.

## Integration into Mesh

### Files touched

1. `crates/mesh-cue/src/ml_analysis/models.rs`
   - Add `MUQ_MULAN_AGGRESSION_AXIS_FILENAME = "muq-mulan-aggression-axis.json"`.
   - Add `aggression_axis_path(model)` resolver, parallel to `norm_path`.
   - Update `is_available` and `install_to_cache` to include the axis JSON.
   - Update error messages: "run `nix run .#derive-aggression-axis`".

2. `crates/mesh-cue/src/ml_analysis/inference.rs` (or new tiny module
   `aggression_axis.rs`)
   - Parse the axis JSON at `MlAnalyzer::new` time. Cache as
     `pub aggression_axis: Vec<f32>` (length == 512, asserted at parse).
   - Expose `pub fn aggression_axis(&self) -> &[f32]`.

3. `crates/mesh-core/src/suggestions/aggression.rs`
   - `compute_aggression_weights` no longer fed by genre proxy. Two
     options:
     - **(a)** Delete it; the dev-script JSON is the only source of weights.
     - **(b)** Repurpose: keep as a *secondary* fit — an additive
       correction on top of the polar axis using the calibration pairs
       (logistic regression on `axis_score(A) - axis_score(B)` vs user
       choice). Final weights = `polar_axis + λ·correction`.
   - `project_aggression(pca_vec, weights)` is unchanged — same dot
     product, same shape.

4. `crates/mesh-cue/src/ui/handlers/similarity.rs:78–105`
   - The "Build Similarity Index" flow currently calls
     `compute_aggression_weights` after PCA build. Rewrite to:
     ```rust
     // No more genre-derived estimates. Aggression axis is loaded from
     // the model artifact at startup; we just persist it here so the
     // suggestion path's `db.get_aggression_weights()` keeps working
     // unchanged for both sources (local + USB DBs).
     let axis = analyzer.aggression_axis();  // from MlAnalyzer
     db.store_aggression_weights(axis, /*correlation=*/ 1.0)?;
     ```
   - Drop the genre-iteration loop (`compute_track_aggression(genre)`
     line 89) — it's now dead code.

5. `crates/mesh-core/src/suggestions/query.rs:589–602`
   - Already does `pca_vec.len() == weights.len()` guard. After this
     change `weights.len() == 512` and `pca_vec.len() == 512` (assuming
     `PCA_REDUCTION_ENABLED = false`, which we just shipped). Match.
   - If the user re-enables PCA reduction later, dim mismatch silently
     drops aggression scoring — graceful, but worth a log warning at
     load time.

### Storage / schema

**Zero schema changes.** The `pca_aggression_axis` relation is
already `weights: [Float], correlation: Float` — variable-length list,
fits 512 floats as easily as 27. The migration script doesn't even
re-fire (relation already exists).

The `correlation` field becomes semantically odd (no Pearson is being
computed). Two reasonable values to store:
- **1.0** — "this is a model-anchored axis, treat as ground truth."
- **A computed agreement score** — fraction of stored calibration
  pairs whose ordering agrees with the polar axis. Useful as a
  diagnostic in `aggression_inspect.rs`.

Recommend: store the calibration-pair agreement rate. It's a free
quality signal and gives the existing `aggression_inspect` binary
something meaningful to print.

### USB / cross-DB behaviour

Each DB (local + each USB) gets the same axis written into its own
`pca_aggression_axis` row. mesh-player's existing fallback chain
(`query.rs:574–587`) still works — but since the axis is identical
across all DBs, the fallback path is never exercised in practice.
Cross-source aggression comparability becomes trivially correct:
everyone is projecting onto the same direction.

USB sync code path is unchanged — `pca_aggression_axis` is already in
the export set (`export/service.rs:198`).

## Calibration UI — what role does it have going forward?

The current modal flow (`crates/mesh-cue/src/ui/handlers/calibration.rs`)
asks the user "is A or B more aggressive?" and stores pairs in
`aggression_calibration_pairs`. With a polar-prompt axis those pairs
are no longer the *fit data* — but they're not useless either.

Three honest options, pick one:

**1. Decommission entirely.** Drop the modal, drop the relation, drop
   the trigger from `graph.rs:191`. Cleanest. Loses a useful eval
   signal.

**2. Keep as eval-only.** Modal still surfaces, pairs still stored,
   but they no longer fit weights. Instead they feed a single number:
   *"polar axis agrees with N/M of your stored pairs (X %)."* Surface
   that in the diagnostics binary. Provides cheap regression detection
   when prompts change.

**3. Keep as fine-tune layer.** Use pairs to fit a small additive
   correction Δ in 512-d, minimizing logistic loss on the axis-score
   differences. Final axis = `(polar + λ·Δ) / ||...||` for some small
   λ ∈ [0, 0.3]. Tunable strength of personalization.

Recommend (2) for the first ship — it's the smallest change, preserves
optionality, and surfaces a quality number. (3) becomes interesting
later if (2)'s agreement rate is uncomfortably low.

## Validation methodology

### Pre-ship (dev box)

1. **Sanity by extreme.** Pick 5 tracks you know are obvious aggression
   peaks (peak-time hard techno) and 5 obvious troughs (intro/ambient).
   Project all 10 onto the axis. Peaks should sort at top, troughs at
   bottom. If the ordering is wrong, the prompt set is wrong.

2. **Self-consistency check.** Ablation: drop one positive prompt at
   a time, recompute axis, compute Spearman ρ vs the full-axis
   ordering across your library. If any single prompt swings the
   ordering by >0.1 ρ, that prompt is a load-bearing outlier — either
   replace it or add more prompts to the same side to dilute it.

3. **Existing calibration pairs.** Run the agreement check against
   `aggression_calibration_pairs` (whatever pairs the user has
   accumulated). Report agreement rate. <60 % = axis is worse than
   chance, prompts need rework. >75 % = ship.

### Post-ship (real use)

1. Open the suggestions view, change the energy slider end-to-end,
   confirm the recommended tracks change in the expected direction.
2. Use the graph view's energy color coding (if it uses aggression
   scores) — confirm the gradient looks right.
3. Run the existing `aggression_inspect` binary on the new axis;
   the top-N and bottom-N tracks should both look right.

## Step-by-step implementation order

Each step is independently reviewable and committable.

1. **Spike**: write `derive.py` standalone (no Nix wrapper yet), run
   it in the existing `~/.cache/mesh-spike/site-packages-gpu-cu124/`
   env, generate a candidate axis JSON. Eyeball the prompt outputs,
   sanity-check on 10 known tracks. Iterate prompts.

2. **Wrap in Nix**: package `derive.py` as `nix/apps/derive-aggression-axis.nix`.
   Mirror the existing audio converter app's deps.

3. **Commit the axis JSON** to `models/`. Same pattern as the
   `.norm.json` sidecar (committed at 930f30a).

4. **Wire into `MlModelManager`**: add the filename, the resolver, the
   install_to_cache step, and the availability check. Update error
   messages.

5. **Add axis loader to `MlAnalyzer`**: parse JSON at construction,
   expose `aggression_axis() -> &[f32]`.

6. **Replace `similarity.rs` aggression-fit block**: write the loaded
   axis into `pca_aggression_axis` instead of running the
   genre-Pearson fit. Compute and store calibration-pair agreement
   rate as the `correlation` field.

7. **Delete dead code**: `compute_track_aggression`,
   `compute_aggression_weights` (or both — depending on whether we
   want to keep the function as a vestigial regression target for
   future use).

8. **Calibration UI decision**: implement option (2) — eval-only
   surfacing. Drop the weight-rebuild side-effect from the modal's
   completion path.

9. **CI**: upload the axis JSON to the GitHub `models` release
   alongside the ONNX + `.norm.json`. mesh-cue's auto-install path
   downloads it the same way.

10. **Open-questions doc**: add an entry on prompt-set governance
    (when/why we'd re-derive the axis), and the validation summary
    from step 1.

Estimated effort:
- Steps 1–3 (axis derivation): 1 evening.
- Steps 4–9 (Mesh integration + calibration UI re-scope): 1 day.
- Step 10 (docs): 1 hour.

Total ~1.5 days for the headline change. Compare to Option 3 from the
research doc (LLM labeller path): ~10–20 GPU-hours plus a multi-week
tag-pass infrastructure build.

## Open questions for user (decide before step 1)

1. **Prompt language.** All English vs include German/multilingual?
   XLM-Roberta is multilingual — could include the German equivalents
   ("aggressiv harter Techno mit knallenden Bässen") for free. Might
   help or might dilute. Defer until prompt iteration starts.

2. **Calibration UI fate.** Decommission, keep-as-eval, or keep-as-fine-tune?
   Recommendation: keep-as-eval (option 2 above) for first ship.

3. **Axis storage.** Sidecar JSON (recommend) or embedded constant?

4. **Single axis or multiple?** Aggression is the headline gap, but
   the same trick generalizes to other axes (dark↔bright, hypnotic↔melodic,
   minimal↔dense). If we're already shipping the artifact format,
   trivially extensible. Decide whether to scope this PR to aggression
   only or include 2–3 axes.

5. **Re-derivation policy.** When a new MuQ-MuLan version drops, the
   axis is invalid. Add a model-version check at load time (axis JSON
   carries `"model": "OpenMuQ/MuQ-MuLan-large"`, ONNX presumably has
   the same identity)? Worth it as a single sanity assert.

## Out of scope for this plan

- **LLM-labelled tag heads** (research doc Option 3). Bigger lift.
  Worth doing *after* this lands — at that point we have a deterministic
  baseline to measure improvement against.
- **Distillation to a smaller model** (Option 6). Long-term concern.
- **Multi-axis tag JSON output for graph community labels.** The
  current "Other" macro-label degradation (open-questions item 3) could
  be addressed by running the same polar-prompt trick over a tag
  vocabulary. Independent of aggression — separate plan if desired.
