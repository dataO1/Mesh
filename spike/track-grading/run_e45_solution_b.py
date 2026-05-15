#!/usr/bin/env python3
"""
Solution B: Create teacher predictions from LoRA scoring head and distill student.

Loads the LoRA checkpoint head (Linear 1024→1), scores all tracks in the
LoRA NPZ, writes a teacher_preds NPZ, then runs student distillation with
--lambda-fit 0 (no FitNets — same-modality teacher has no penultimate geometry
to match; the student learns purely from the output scores).

Usage: python spike/track-grading/run_e45_solution_b.py
"""
from __future__ import annotations

import sys, argparse
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

PROJECT = Path(__file__).resolve().parent.parent.parent
BASE = Path("/home/data01/Music/mesh-track-grading")
AUDIO_EMB = BASE / "embeddings/corpus_muq_mulan_lora.npz"
CKPT_DIR  = BASE / "round7_7_lora"
CONSENSUS = BASE / "round7_6_consensus.npz"
SPLIT     = BASE / "round7_6_split.npz"
OUT_PREDS = BASE / "round7_7_lora_teacher_preds.npz"

HIDDEN_DIM = 1024


def main() -> int:
    # Load LoRA head weights
    epoch = int((CKPT_DIR / "latest_epoch.txt").read_text().strip())
    state = torch.load(CKPT_DIR / f"epoch_{epoch:03d}_training_state.pt", map_location="cpu")
    best_rho = state.get("best_val_rho", "?")
    head_w = state["head_state_dict"]["head.weight"].numpy()  # (1, 1024)
    head_b = state["head_state_dict"]["head.bias"].numpy()    # (1,)
    print(f"[B] LoRA head from epoch {epoch} (val ρ={best_rho})")

    # Load LoRA embeddings
    e = np.load(AUDIO_EMB, allow_pickle=True)
    emb = e["embeddings_1024"].astype(np.float32)            # (N, 1024)
    tids = e["track_ids"].astype(np.int64)
    print(f"[B] Loaded {len(tids)} embeddings from {AUDIO_EMB.name}")

    # Score all tracks through LoRA head
    scores = (emb @ head_w.T).squeeze(-1) + head_b.squeeze()  # (N,)
    print(f"[B] Scored {len(scores)} tracks — range [{scores.min():.3f}, {scores.max():.3f}]")

    # Load consensus + split for alignment
    c = np.load(CONSENSUS, allow_pickle=True)
    cons_map = {int(t): c["consensus_intensity"][i] for i, t in enumerate(c["track_ids"])}

    s = np.load(SPLIT, allow_pickle=True)
    split_map = {int(t): str(lbl) for t, lbl in zip(s["track_ids"], s["split"])}

    # Align: only tracks in all three (embeddings, consensus, split)
    common = sorted(set(int(t) for t in tids) & set(cons_map) & set(split_map))
    print(f"[B] Aligned {len(common)} tracks across embeddings + consensus + split")

    idx_map = {int(t): i for i, t in enumerate(tids)}
    out_tids = np.array(common, dtype=np.int64)
    out_intensity = np.array([scores[idx_map[t]] for t in common], dtype=np.float32)
    out_consensus = np.array([cons_map[t] for t in common], dtype=np.float32)
    out_split     = np.array([split_map[t] for t in common])

    # Teacher penultimate: use raw 1024-d embedding (student will project)
    out_pen = np.array([emb[idx_map[t]] for t in common], dtype=np.float32)

    np.savez_compressed(
        OUT_PREDS,
        track_ids=out_tids,
        teacher_intensity=out_intensity,
        teacher_axes=np.zeros((len(common), 0), dtype=np.float32),
        teacher_penultimate=out_pen,
        split=out_split,
        consensus_intensity=out_consensus,
    )
    print(f"[B] Wrote {OUT_PREDS}")

    # Spearman ρ vs consensus for reference
    from bt_pair_sampler import spearman_rho
    rho = spearman_rho(out_intensity, out_consensus)
    print(f"[B] LoRA head vs consensus Spearman ρ = {rho:+.4f}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
