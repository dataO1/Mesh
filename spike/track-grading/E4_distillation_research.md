# E4 LoRA — Student Regression: Research Findings & Proposed Next Steps

## Problem
LoRA fine-tuning of MuQ-MuLan improves the teacher (+0.72pp PA: 0.934→0.941)
but the audio-only MLP student **regresses** (−0.63pp: 0.819→0.813).
Deeper student (h=256) doesn't help (0.8125). The distill gap widens from
+0.115 to +0.128.

## What the Literature Says

### 1. Capacity Gap (Cho et al., ICCV 2019; "Law of Capacity Gap", arXiv 2311.07052)

A stronger teacher can produce a WORSE student — this is a well-documented
phenomenon called the "capacity gap." Our teacher went from 0.934→0.941,
making the gap larger. The student's 131K-param MLP can't emulate the richer
teacher predictions from the LoRA-tuned geometry.

**Key insight**: "Teacher accuracy is a poor predictor of student performance.
Larger teachers do not necessarily make better teachers." Early-stopping the
teacher can help — a slightly weaker teacher may produce a stronger student.

### 2. Feature Space Misalignment (arXiv 2310.17183; AAAI 2024)

Adding a learnable **linear projection layer** between student and teacher
features improves distillation EVEN WHEN dimensions already match. The
projector encodes relational information and bridges the geometry gap
created by encoder fine-tuning. Our FitNets hint loss directly matches
128-d→128-d without a projector — adding one could help bridge the
distribution shift from LoRA.

**Key insight**: "Projectors improve distillation even when dimensions match
— they implicitly encode relational information from past examples."

### 3. Orthogonal Projections (VkD, CVPR 2024)

Constraining the projection to be orthogonal preserves the student's
underlying representation structure while maximizing transferred knowledge.
Could apply to our FitNets hint layer.

### 4. Teacher Adaptation / "Good Teacher Adapts Knowledge" (Qian et al., ICCV 2025)

The teacher should adapt its knowledge to what the student CAN learn.
Methods include:
- **Gap Preserving Distillation (GPD)**: train a dynamic teacher alongside
  the student to maintain a learnable gap.
- **Teacher Privileged Distillation**: teacher has access to student's
  learning progress and adjusts soft targets accordingly.

### 5. Multi-Level Feature Distillation (arXiv 2410.22184)

Distilling from multiple teacher layers simultaneously gives the student
richer supervision. We currently only match the penultimate layer (128-d).
Could add hints from the teacher's first hidden layer (256-d) or the
teacher's input projection.

### 6. Same-Modality Distillation

The consensus teacher uses privileged caption features (1844-d input) that
the student (1024-d audio-only) can never access. The LoRA encoder's own
scoring head (Linear 1024→1, audio-only, val ρ=0.8074) is a same-modality
teacher. Distilling from this instead of the consensus teacher eliminates
the cross-modal capacity gap.

## Proposed Next Steps (ordered by expected impact/effort ratio)

### A. Add FitNets Projection Layer (low effort) — **NEGATIVE**
Linear 128→128 and non-linear 128→256→128 projectors, λ_fit 0.5 and 2.0:
all yield identical PA=0.8129. The hint-matching path is not the bottleneck.

### B. Distill from LoRA Scoring Head (medium effort) — **NEGATIVE**
Same-modality teacher achieves gap≈0 but LoRA head ceiling (ρ=0.81) limits
student to PA=0.8092. Warmup variant (LoRA head 3-10ep → consensus) collapses
PA to 0.766-0.768 — student trapped in compressed distribution, can't recover.

### C. Multi-Level Hint Distillation (medium effort)
Add hints from the teacher's first hidden layer (256-d, post-ReLU) in
addition to the penultimate layer (128-d). Gives the student richer
intermediate supervision.

### D. Early-Stop the Teacher (low effort)
Use the teacher checkpoint from an earlier epoch (e.g., epoch 20 instead
of 76) as the distillation target. A slightly weaker teacher may produce
a stronger student per the capacity gap literature.

### E. Teacher Adaptation Loop (high effort)
Fine-tune the teacher with an auxiliary loss that encourages predictions
the student can match. Requires iterative teacher→student→teacher
training cycles. High compute cost, deferred.

## Sources
- Cho et al., "On the Efficacy of Knowledge Distillation", ICCV 2019
- "Understanding the Effects of Projectors in KD", arXiv 2310.17183 / AAAI 2024
- Miles et al., "VkD: Improving KD using Orthogonal Projections", CVPR 2024
- Qian et al., "A Good Teacher Adapts Their Knowledge for Distillation", ICCV 2025
- "Law of Capacity Gap in LM Distillation", arXiv 2311.07052
- "Multi-Level Feature Distillation of Joint Teachers", arXiv 2410.22184
- "A Comprehensive Overhaul of Feature Distillation", arXiv 1904.01866
- "Gap Preserving Distillation", arXiv 2410.04140
