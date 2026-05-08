"""Round-7 joint blend optimization: learn k blend weights so the linear
combination of axis scores reproduces the AGGRESSION ranking.

This is the single-blend-vector deployment path — what mesh-cue / mesh-player
will load. Trained against the aggression-axis BT priors as the target
ranking; uses ListMLE loss for direct ranking optimisation.

Inputs:
  /home/data01/Music/mesh-track-grading/round7_axes.npz       (axes, directions[K, D], biases[K])
  /home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz
  /home/data01/Music/mesh-track-grading/round7_priors.npz     (priors_0_10[K, M] for target axis)

Outputs:
  /home/data01/Music/mesh-track-grading/round7_blend.npz
    axes        : object[K]
    directions  : float32[K, D]   (unchanged)
    biases      : float32[K]      (unchanged)
    weights     : float32[K]      (blend; sum to 1)
    target_axis : str
    metrics     : dict
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--axes-file", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_axes.npz"))
    p.add_argument("--embeddings", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz"))
    p.add_argument("--priors", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_priors.npz"))
    p.add_argument("--target-axis", default="aggression")
    p.add_argument("--out", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_blend.npz"))
    p.add_argument("--epochs", type=int, default=2000)
    p.add_argument("--lr", type=float, default=5e-2)
    p.add_argument("--list-size", type=int, default=64,
                   help="ListMLE permutation size per minibatch")
    p.add_argument("--batches-per-epoch", type=int, default=32)
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    return p.parse_args()


def listmle_loss(scores: torch.Tensor, targets: torch.Tensor) -> torch.Tensor:
    """ListMLE: -sum log P(y_i | y_{i+1..n}) using descending target order."""
    order = torch.argsort(targets, descending=True)
    s_ord = scores[order]
    # log P_i = s_i - log sum_{j >= i} exp(s_j)
    rev_logcumsum = torch.flip(
        torch.logcumsumexp(torch.flip(s_ord, dims=[0]), dim=0), dims=[0])
    return -(s_ord - rev_logcumsum).mean()


def pair_agreement(scores: np.ndarray, y: np.ndarray) -> float:
    n = len(scores)
    if n < 2:
        return 0.0
    ds = scores[:, None] - scores[None, :]
    dy = y[:, None] - y[None, :]
    tri = np.triu(np.ones((n, n), dtype=bool), k=1)
    valid = tri & (ds != 0) & (dy != 0)
    concordant = (valid & ((ds > 0) == (dy > 0))).sum()
    total = valid.sum()
    return float(concordant / total) if total else 0.0


def spearman_rho(a: np.ndarray, b: np.ndarray) -> float:
    n = len(a)
    if n < 2:
        return 0.0
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    sum_d2 = float(np.sum((ra - rb) ** 2))
    return 1 - 6 * sum_d2 / (n * (n * n - 1))


def main() -> int:
    args = parse_args()
    rng = np.random.default_rng(args.seed)
    torch.manual_seed(args.seed)

    axes_npz = np.load(args.axes_file, allow_pickle=True)
    emb = np.load(args.embeddings, allow_pickle=True)
    pri = np.load(args.priors, allow_pickle=True)

    axes = list(axes_npz["axes"])
    directions = axes_npz["directions"].astype(np.float32)  # [K, D]
    biases = axes_npz["biases"].astype(np.float32)          # [K]
    K, D = directions.shape

    if args.target_axis not in axes:
        sys.exit(f"target {args.target_axis!r} not in axes {axes}")
    t_idx = axes.index(args.target_axis)
    print(f"[r7-blend] axes K={K}, target={args.target_axis} (#{t_idx})")

    e_ids = emb["track_ids"].astype(np.int64)
    e_X = emb["embeddings"].astype(np.float32)
    p_ids = pri["track_ids"].astype(np.int64)
    p_priors = pri["priors_0_10"].astype(np.float32)
    p_games = pri["n_games"].astype(np.int32)

    # Build target vector: tracks with games on the target axis.
    e_idx_map = {int(t): i for i, t in enumerate(e_ids)}
    p_idx_map = {int(t): i for i, t in enumerate(p_ids)}
    common = []
    targets = []
    for j, t in enumerate(p_ids):
        ti = int(t)
        if ti not in e_idx_map:
            continue
        if p_games[t_idx, j] == 0:
            continue
        common.append(ti)
        targets.append(p_priors[t_idx, j])
    common_ids = np.array(common, dtype=np.int64)
    y = np.array(targets, dtype=np.float32)
    X = np.array([e_X[e_idx_map[t]] for t in common_ids], dtype=np.float32)
    print(f"[r7-blend] training set: {len(common_ids)} tracks with target priors")

    # Pre-compute axis scores once (since directions are frozen).
    # raw_axis_score = X @ directions.T + biases
    axis_scores = X @ directions.T + biases  # [N, K]
    # Normalise each axis to roughly comparable scale: z-score per axis.
    means = axis_scores.mean(axis=0, keepdims=True)
    stds = axis_scores.std(axis=0, keepdims=True) + 1e-6
    axis_z = (axis_scores - means) / stds

    device = torch.device(args.device)
    A = torch.from_numpy(axis_z).to(device)        # [N, K]
    Y = torch.from_numpy(y).to(device)
    # Learn unconstrained logits; final weights via softmax for stability.
    logits = nn.Parameter(torch.zeros(K, device=device))
    opt = torch.optim.AdamW([logits], lr=args.lr, weight_decay=1e-4)

    N = A.shape[0]
    L = min(args.list_size, N)
    print(f"[r7-blend] {args.epochs} epochs × {args.batches_per_epoch} batches × list_size {L}")

    for ep in range(args.epochs):
        ep_loss = 0.0
        for _ in range(args.batches_per_epoch):
            sel = rng.choice(N, size=L, replace=False)
            sel_t = torch.from_numpy(sel).long().to(device)
            opt.zero_grad()
            w = F.softmax(logits, dim=0)        # K
            scores = A[sel_t] @ w               # L
            loss = listmle_loss(scores, Y[sel_t])
            loss.backward()
            opt.step()
            ep_loss += float(loss)
        if ep % 100 == 0 or ep == args.epochs - 1:
            with torch.no_grad():
                w = F.softmax(logits, dim=0).cpu().numpy()
                preds_all = axis_z @ w
                pa = pair_agreement(preds_all, y)
                rho = spearman_rho(preds_all, y)
            print(f"  ep {ep}: loss={ep_loss/args.batches_per_epoch:.4f}  "
                  f"PA={pa:.4f}  rho={rho:+.4f}  "
                  f"top: " + ", ".join(
                    f"{axes[i]}={w[i]:.2f}" for i in np.argsort(-w)[:4]))

    with torch.no_grad():
        w_final = F.softmax(logits, dim=0).cpu().numpy()
        preds_all = axis_z @ w_final
        pa = pair_agreement(preds_all, y)
        rho = spearman_rho(preds_all, y)

    print(f"\n[r7-blend] FINAL: PA={pa:.4f}  rho={rho:+.4f}")
    for k in np.argsort(-w_final):
        print(f"  {axes[k]:>22}: w={w_final[k]:.4f}")

    # Embed normalisation into directions+biases so the deployment path is:
    #   blended_score(x) = sum_k w_k * ((x @ d_k + b_k) - mean_k) / std_k
    #                    = x @ (sum_k w_k/std_k * d_k) + sum_k w_k*(b_k - mean_k)/std_k
    # We bake it as: effective_direction = sum_k w_k/std_k * d_k
    #                effective_bias = sum_k w_k*(b_k - mean_k)/std_k
    # so deployment is a single scalar projection.
    norm_factors = w_final / stds[0]                                # [K]
    effective_direction = (norm_factors[:, None] * directions).sum(axis=0)  # [D]
    effective_bias = float((w_final * (biases - means[0]) / stds[0]).sum())

    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(args.out,
             axes=np.array(axes, dtype=object),
             directions=directions,
             biases=biases,
             axis_means=means.astype(np.float32),
             axis_stds=stds.astype(np.float32),
             weights=w_final.astype(np.float32),
             effective_direction=effective_direction.astype(np.float32),
             effective_bias=np.float32(effective_bias),
             target_axis=args.target_axis,
             pa=np.float32(pa), rho=np.float32(rho))
    print(f"[r7-blend] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
