"""Fast ridge regression fallback for round-7.5 multi-task linear probes.

The full PyTorch pairwise-margin training (train_axes_r7_5.py) was taking
hours on CPU due to repeated O(N^2) intermediate tensor allocation per
epoch. This is a closed-form ridge regression alternative: per-axis
Linear(512 -> 1) with L2 regularisation, fit directly via the normal
equation. Loses some pairwise-ranking optimality versus RankNet but
produces a usable axis with consistent output format and a >10 minute
total runtime budget.

Training target = BT priors normalised to [-1, +1] (centred). The blend
optimiser downstream is what makes the final intensity rank-correct
against round-5 BT, so per-axis MSE-vs-prior is fine here.

Inputs:
  /home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz
  /home/data01/Music/mesh-track-grading/round7_5_priors.npz

Outputs (same schema as train_axes_r7_5.py):
  /home/data01/Music/mesh-track-grading/round7_5_axes.npz
  /home/data01/Music/mesh-track-grading/round7_5_predictions.csv
  /home/data01/Music/mesh-track-grading/round7_5_train_metrics.json
"""
from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--embeddings", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz"))
    p.add_argument("--priors", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_5_priors.npz"))
    p.add_argument("--out-axes", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_5_axes.npz"))
    p.add_argument("--out-preds", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_5_predictions.csv"))
    p.add_argument("--out-metrics", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_5_train_metrics.json"))
    p.add_argument("--ridge-lambda", type=float, default=1.0,
                   help="L2 regularisation strength for ridge regression")
    p.add_argument("--folds", type=int, default=5)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def pair_agreement(scores, y):
    n = len(scores)
    if n < 2: return 0.0
    ds = scores[:, None] - scores[None, :]
    dy = y[:, None] - y[None, :]
    tri = np.triu(np.ones((n, n), dtype=bool), k=1)
    valid = tri & (ds != 0) & (dy != 0)
    return float((valid & ((ds > 0) == (dy > 0))).sum() / max(valid.sum(), 1))


def spearman_rho(a, b):
    n = len(a)
    if n < 2: return 0.0
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    return 1 - 6 * float(np.sum((ra - rb) ** 2)) / (n * (n * n - 1))


def ridge_fit(X, y, lam):
    """Closed-form ridge regression: w = (X^T X + lam I)^-1 X^T y."""
    D = X.shape[1]
    XtX = X.T @ X
    XtX += lam * np.eye(D, dtype=X.dtype)
    Xty = X.T @ y
    return np.linalg.solve(XtX, Xty)


def main():
    args = parse_args()
    rng = np.random.default_rng(args.seed)

    emb = np.load(args.embeddings, allow_pickle=True)
    pri = np.load(args.priors, allow_pickle=True)
    e_ids = emb["track_ids"].astype(np.int64)
    e_X = emb["embeddings"].astype(np.float32)
    p_ids = pri["track_ids"].astype(np.int64)
    p_axes = list(pri["axes"])
    p_priors = pri["priors_0_10"].astype(np.float32)
    p_games = pri["n_games"].astype(np.int32)

    K = p_priors.shape[0]
    D = e_X.shape[1]
    print(f"[r7.5-fast] axes K={K}: {p_axes}")
    print(f"[r7.5-fast] embedding D={D}, ridge_lambda={args.ridge_lambda}")

    e_idx = {int(t): i for i, t in enumerate(e_ids)}
    common = [int(t) for t in p_ids if int(t) in e_idx]
    track_ids = np.array(common, dtype=np.int64)
    X = np.array([e_X[e_idx[int(t)]] for t in track_ids], dtype=np.float32)
    p_idx = {int(t): i for i, t in enumerate(p_ids)}
    Y = np.zeros((K, len(track_ids)), dtype=np.float32)
    M = np.zeros((K, len(track_ids)), dtype=bool)
    for j, t in enumerate(track_ids):
        i = p_idx[int(t)]
        Y[:, j] = p_priors[:, i]
        M[:, j] = (p_games[:, i] > 0)
    print(f"[r7.5-fast] training set: {len(track_ids)} tracks")

    # CV
    folds = []
    perm = rng.permutation(len(track_ids))
    for k in range(args.folds):
        va = perm[k::args.folds]
        tr = np.array([i for i in perm if i not in set(va.tolist())])
        folds.append((tr, va))

    cv_pa = np.zeros(K, dtype=np.float32)
    cv_rho = np.zeros(K, dtype=np.float32)
    cv_records = []
    for fold, (tr, va) in enumerate(folds):
        per_axis_pa = np.zeros(K, dtype=np.float32)
        per_axis_rho = np.zeros(K, dtype=np.float32)
        for k in range(K):
            mask_tr = M[k, tr]
            mask_va = M[k, va]
            if mask_tr.sum() < 50 or mask_va.sum() < 10:
                continue
            X_tr = X[tr][mask_tr]
            y_tr = Y[k, tr][mask_tr]
            # Centre y around mean for ridge stability
            y_mean = y_tr.mean()
            w = ridge_fit(X_tr, y_tr - y_mean, args.ridge_lambda)
            X_va = X[va][mask_va]
            y_va = Y[k, va][mask_va]
            preds_va = X_va @ w + y_mean
            per_axis_pa[k] = pair_agreement(preds_va, y_va)
            per_axis_rho[k] = spearman_rho(preds_va, y_va)
        cv_pa += per_axis_pa
        cv_rho += per_axis_rho
        cv_records.append({"fold": fold,
                           "per_axis_pa": per_axis_pa.tolist(),
                           "per_axis_rho": per_axis_rho.tolist()})
        sample = ", ".join(f"{p_axes[k]}={per_axis_pa[k]:.3f}" for k in range(min(K, 5)))
        print(f"  fold {fold}: PA[first5]: {sample} ...")
    cv_pa /= len(folds)
    cv_rho /= len(folds)

    print(f"\n[r7.5-fast] CV mean per-axis PA / rho:")
    for k in range(K):
        print(f"  {p_axes[k]:>22}: PA={cv_pa[k]:.3f}  rho={cv_rho[k]:+.3f}")

    # Final retrain on full data
    print(f"\n[r7.5-fast] retraining on full data ...")
    W_final = np.zeros((K, D), dtype=np.float32)
    b_final = np.zeros(K, dtype=np.float32)
    for k in range(K):
        mask = M[k]
        X_k = X[mask]
        y_k = Y[k][mask]
        y_mean = y_k.mean()
        w = ridge_fit(X_k, y_k - y_mean, args.ridge_lambda)
        W_final[k] = w
        b_final[k] = y_mean
        # Bias rolled into b_final since predictions = X @ w + y_mean

    # Project all training tracks for predictions CSV
    scores_all = X @ W_final.T + b_final  # [N, K]

    # Row-normalise the directions (interpretability convention) — bias adjusts
    norms = np.linalg.norm(W_final, axis=1, keepdims=True)
    norms = np.clip(norms, 1e-9, None)
    W_n = W_final / norms
    b_n = b_final / norms[:, 0]

    args.out_axes.parent.mkdir(parents=True, exist_ok=True)
    np.savez(args.out_axes,
             axes=np.array(p_axes, dtype=object),
             directions=W_n.astype(np.float32),
             biases=b_n.astype(np.float32),
             cv_pa=cv_pa,
             cv_rho=cv_rho,
             track_ids=track_ids,
             tag_names=np.array([], dtype=object))  # empty: no aux loss in fast mode
    print(f"[r7.5-fast] wrote {args.out_axes}")

    with args.out_preds.open("w") as f:
        w = csv.writer(f)
        w.writerow(["track_id"] + list(p_axes))
        for j, t in enumerate(track_ids):
            row = [int(t)] + [f"{scores_all[j, k]:.6f}" for k in range(K)]
            w.writerow(row)
    print(f"[r7.5-fast] wrote {args.out_preds}")

    args.out_metrics.write_text(json.dumps({
        "method": "ridge regression (fast fallback)",
        "ridge_lambda": args.ridge_lambda,
        "cv_records": cv_records,
        "cv_mean_pa": {p_axes[k]: float(cv_pa[k]) for k in range(K)},
        "cv_mean_rho": {p_axes[k]: float(cv_rho[k]) for k in range(K)},
    }, indent=2))
    print(f"[r7.5-fast] wrote {args.out_metrics}")


if __name__ == "__main__":
    main()
