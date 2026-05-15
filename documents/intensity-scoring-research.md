# Mesh Intensity Scoring — Research & Methodology

> **TL;DR:** Mesh uses a multi-stage pipeline combining large language model jurors, Music Flamingo captions, and a fine-tuned MuQ-MuLan audio encoder to score track intensity on a 0–1 scale. The current model (V18_round7_7_lora_v2) achieves 82.8% pairwise agreement with a 5-juror consensus on a held-out test set of 3,985 tracks. The system has been refined through six model generations across two development rounds.

---

## How Intensity Scoring Works

### 1. Audio Encoding

Every track is processed through **MuQ-MuLan-large**, a 663M-parameter audio Conformer model from OpenMuQ. The encoder produces a 1024-dimensional vector capturing the track's sonic characteristics — timbre, rhythm, spectral density, and harmonic content. The current model uses a LoRA (Low-Rank Adaptation) fine-tuned variant that has been specifically adapted for intensity discrimination while preserving the original pretrained geometry.

### 2. Intensity Projection

A compact MLP (1024 → 128 → GELU → 1, 131K parameters) projects the audio embedding to a scalar intensity score. This student model is trained via knowledge distillation from a larger teacher that has access to privileged information: Music Flamingo-generated captions describing each track's sonic character, structured genre/energy tags, and a 5-juror consensus label.

### 3. Ground Truth: LLM-as-Judge Consensus

The training signal comes from five independent large language models (Nemotron-30B, Qwen3.6-27B, Mistral-Small-3.2-24B, Gemini Flash 3, DeepSeek V4 Pro) that rate each track's intensity from 1–20 based on its Music Flamingo caption. Their ratings are aggregated via Dawid-Skene expectation maximization to produce a continuous consensus score on [0, 1]. All five jurors agree on the relative intensity ordering of genres (thrash metal at top, ambient at bottom).

### 4. Inference-Time Clip Selection

During deployment, the system uses an energy-pruned clip selection strategy (A5): it identifies the two most energetically dense 30-second windows in the track, extracts three 10-second clips from each, scores them through the MLP, and takes the mean of the higher-scoring window. This matches the training distribution (30-second Deezer previews mean-pooled over 3 × 10s clips).

---

## Model Evolution

| Version | Encoder | Student Input | Held-Out PA | Key Innovation |
|---|---|---|---|---|
| V15 | MuQ-MuLan (frozen) | 512-d projection | 0.701 | Linear probe baseline |
| V17b | MuQ-MuLan (frozen) | polar-blend axes | 0.727 | Multi-axis blend |
| V18.1 | MuQ-MuLan (frozen) | 512-d projection | 0.817 | MLP student + FitNets/Hinton distillation |
| V18.X | MuQ-MuLan (frozen) | **1024-d Conformer hidden** | 0.819 | Switch to pre-projection hidden states (per MuQ paper probe protocol) |
| V18.X+A5 | MuQ-MuLan (frozen) | 1024-d + A5 clip selection | 0.819 | Energy-pruned window selection, train/deploy distribution alignment |
| **V18.X+LoRA** | **MuQ-MuLan + LoRA r=16** | **1024-d + A5** | **0.828** | LoRA fine-tuned encoder, anchor-loss geometry preservation |

Held-out PA = pairwise agreement with 5-juror consensus on 3,985 artist-stratified held-out tracks. Higher is better; random = 0.50.

---

## Verification Methodology

Every model change is evaluated through a standardized 8-step verification suite before shipping:

1. **Held-out test PA** — 3,985 tracks, artist-stratified split, zero train/test overlap. Measures pairwise agreement with 5-juror consensus.
2. **Per-cluster diagnostic** — caption-embedding K-means clusters (K=20) scored to verify genre-level ordering is monotone.
3. **Library re-analysis** — full 909-track DnB library re-encoded through the new ONNX model.
4. **Baseline export** — per-track scores and percentile ranks exported for comparison.
5. **Distribution analysis** — moments, entropy, effective bin count, density-at-median.
6. **kNN-residual outliers** — for each track, compare its score against the mean of its 15 nearest sonic neighbours. Independent of training labels.
7. **Big-shift verdict** — identify tracks shifting ≥10 percentile points and determine direction.
8. **Known-label sanity check** — 122 aggressive + 17 liquid DnB tracks with artist-level labels verify the model correctly separates high-energy from low-energy.

---

## Key Design Decisions

**Caption-as-feature, not direct audio labeling.** LLMs rate captions, not raw audio. This decouples juror diversity (different model architectures, training data, RLHF lineages) from the audio encoder, and allows cheap re-aggregation when new jurors are added.

**Encoder-probe architecture.** The audio encoder and intensity head are separate components. The encoder (663M params, ONNX-exported) runs once per track during library import. The head (131K params, embedded in the binary) projects cached embeddings to scores at query time — sub-millisecond latency.

**Distillation over direct training.** The student MLP learns from a teacher that has access to caption features, structured tags, and consensus labels. This privileged-information distillation (LUPI) produces a deployable audio-only model that inherits the teacher's ranking knowledge without requiring captions at inference time.

**Geometry-preserving fine-tuning.** When fine-tuning the encoder with LoRA, a feature-preservation anchor loss prevents the model from finding degenerate ranking solutions. The Bradley-Terry pairwise loss only cares about rank order — without constraints, the model can rotate the embedding space arbitrarily while maintaining correct rankings. An MSE anchor against the frozen encoder's outputs keeps the geometry stable.

---

## Current Performance (V18_round7_7_lora_v2)

| Metric | Value |
|---|---|
| Held-out pairwise agreement | **82.8%** (vs 5-juror consensus) |
| Spearman rank correlation | **0.844** |
| Library aggressive/liquid separation | **+52.6 pp** (aggressive DnB mean 57.9%ile, liquid 5.3%ile) |
| kNN internal consistency | mean |z| = 0.87σ, 18 strong outliers (|z|>3σ) out of 909 |
| Effective intensity bins | 21 (how many distinct intensity levels the model meaningfully uses) |
| Deployment latency | <2 seconds per track (MuQ-MuLan ONNX + A5 window selection) |
| ONNX model size | 1,212 MB (same as frozen — LoRA adds zero weight overhead) |

---

## References

- [[Mesh — Round 7.7 Improvement Research]] — full planning document with architecture decisions and ablation roadmap
- [[Mesh — Round 7.7 Implementation Log]] — step-by-step implementation log with measured outcomes
- [[Mesh — E4 LoRA-v2 Experiment Summary & Ship Verdict]] — detailed LoRA experiment report
- [[Mesh — LoRA-v2 kNN Outlier Listening Guide]] — listening-verified outlier analysis
- [[Mesh — Intensity-Axis Pipeline]] — pipeline specification and runbook
