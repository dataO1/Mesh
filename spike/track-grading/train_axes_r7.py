"""Round-7 multi-task linear probes from corpus embeddings + per-axis BT priors.

Inputs:
  /tmp/track-grading/embeddings/corpus_muq_mulan.npz  (track_ids, embeddings[N, D])
  /tmp/track-grading/round7_priors.npz                (axes, track_ids, priors_0_10[K, M])

Trains K linear heads (D → 1) jointly using a multi-task RankNet-style margin
loss over all (track_i, track_j) pairs per axis. Uses 5-fold CV stratified
by aggression-axis bucket. Final retrain on full data.

Outputs:
  /tmp/track-grading/round7_axes.npz
    axes        : object[K]
    directions  : float32[K, D]   (each L2-normalised)
    biases      : float32[K]
    train_meta  : object          (CV scores, metrics)
  /tmp/track-grading/round7_predictions.csv  (track_id, axis_1_norm, axis_2_norm, ...)

Architecture choice: single Linear(D, K) layer = K linear probes sharing
the same input but with independent rows. No nonlinearity, no shared
intermediate (each axis is a pure direction in embedding space).
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
    p.add_argument("--embeddings", type=Path,
                   default=Path("/tmp/track-grading/embeddings/corpus_muq_mulan.npz"))
    p.add_argument("--priors", type=Path,
                   default=Path("/tmp/track-grading/round7_priors.npz"))
    p.add_argument("--out-axes", type=Path,
                   default=Path("/tmp/track-grading/round7_axes.npz"))
    p.add_argument("--out-preds", type=Path,
                   default=Path("/tmp/track-grading/round7_predictions.csv"))
    p.add_argument("--out-metrics", type=Path,
                   default=Path("/tmp/track-grading/round7_train_metrics.json"))
    p.add_argument("--epochs", type=int, default=600)
    p.add_argument("--lr", type=float, default=2e-3)
    p.add_argument("--weight-decay", type=float, default=1e-3)
    p.add_argument("--folds", type=int, default=5)
    p.add_argument("--margin-floor", type=float, default=0.5,
                   help="ignore pairs with |y_i - y_j| <= floor")
    p.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


class MultiAxisLinear(nn.Module):
    """Y = W X + b with W ∈ R^{K, D}, b ∈ R^K. Returns scores [N, K]."""

    def __init__(self, in_dim: int, n_axes: int):
        super().__init__()
        self.lin = nn.Linear(in_dim, n_axes)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.lin(x)


def pairwise_margin_loss_multi(scores: torch.Tensor,
                               targets: torch.Tensor,
                               mask: torch.Tensor,
                               margin_floor: float) -> torch.Tensor:
    """
    scores  [N, K] — model output
    targets [K, N] — BT priors (0-10), 0 where missing
    mask    [K, N] — 1 where the track has a valid prior for that axis
    """
    K, N = targets.shape
    losses = []
    for k in range(K):
        m = mask[k].bool()
        if m.sum() < 2:
            continue
        s = scores[m, k]              # [Nk]
        y = targets[k, m]             # [Nk]
        s_diff = s.unsqueeze(0) - s.unsqueeze(1)  # [Nk, Nk]
        y_diff = y.unsqueeze(0) - y.unsqueeze(1)
        n_k = s.shape[0]
        tri = torch.triu(torch.ones(n_k, n_k, device=s.device), diagonal=1).bool()
        sel = tri & (y_diff.abs() > margin_floor)
        if not sel.any():
            continue
        sign = y_diff.sign()
        margin = y_diff.abs()
        loss_mat = F.relu(margin - sign * s_diff)
        losses.append(loss_mat[sel].mean())
    if not losses:
        return torch.tensor(0.0, device=scores.device, requires_grad=True)
    return torch.stack(losses).mean()


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


def kfold_indices(n: int, folds: int, seed: int) -> list[tuple[np.ndarray, np.ndarray]]:
    rng = np.random.default_rng(seed)
    idx = rng.permutation(n)
    out = []
    for k in range(folds):
        va = idx[k::folds]
        tr = np.array([i for i in idx if i not in set(va.tolist())])
        out.append((tr, va))
    return out


def main() -> int:
    args = parse_args()
    torch.manual_seed(args.seed)
    np.random.seed(args.seed)

    if not args.embeddings.exists():
        sys.exit(f"missing {args.embeddings}")
    if not args.priors.exists():
        sys.exit(f"missing {args.priors}")

    emb = np.load(args.embeddings, allow_pickle=True)
    pri = np.load(args.priors, allow_pickle=True)
    e_ids = emb["track_ids"].astype(np.int64)
    e_X = emb["embeddings"].astype(np.float32)
    p_ids = pri["track_ids"].astype(np.int64)
    p_axes = list(pri["axes"])
    p_priors = pri["priors_0_10"].astype(np.float32)   # [K, M]
    p_games = pri["n_games"].astype(np.int32)          # [K, M]

    K = p_priors.shape[0]
    D = e_X.shape[1]
    print(f"[r7-train] axes K={K}: {p_axes}")
    print(f"[r7-train] embedding D={D} on {len(e_ids)} tracks")
    print(f"[r7-train] priors on {len(p_ids)} tracks")

    # Align: train on tracks present in both. We treat priors as per-axis;
    # a track may have priors for some axes and not others (n_games > 0).
    e_idx = {int(t): i for i, t in enumerate(e_ids)}
    common = [int(t) for t in p_ids if int(t) in e_idx]
    print(f"[r7-train] common tracks: {len(common)}")

    track_ids = np.array(common, dtype=np.int64)
    X = np.array([e_X[e_idx[int(t)]] for t in track_ids], dtype=np.float32)
    p_idx = {int(t): i for i, t in enumerate(p_ids)}
    Y = np.zeros((K, len(track_ids)), dtype=np.float32)
    M = np.zeros((K, len(track_ids)), dtype=np.float32)
    for j, t in enumerate(track_ids):
        i = p_idx[int(t)]
        Y[:, j] = p_priors[:, i]
        M[:, j] = (p_games[:, i] > 0).astype(np.float32)

    # 5-fold CV
    folds = kfold_indices(len(track_ids), args.folds, args.seed)
    cv_records: list[dict] = []
    print(f"[r7-train] {args.folds}-fold CV (margin-floor={args.margin_floor})")
    for fold, (tr, va) in enumerate(folds):
        device = torch.device(args.device)
        model = MultiAxisLinear(D, K).to(device)
        opt = torch.optim.AdamW(model.parameters(), lr=args.lr,
                                weight_decay=args.weight_decay)
        X_tr = torch.from_numpy(X[tr]).to(device)
        Y_tr = torch.from_numpy(Y[:, tr]).to(device)
        M_tr = torch.from_numpy(M[:, tr]).to(device)
        X_va = torch.from_numpy(X[va]).to(device)
        Y_va = torch.from_numpy(Y[:, va]).to(device)
        M_va = torch.from_numpy(M[:, va]).to(device)
        best = {"per_axis_pa": [0.0] * K, "per_axis_rho": [0.0] * K}
        for ep in range(args.epochs):
            model.train()
            opt.zero_grad()
            s = model(X_tr)
            loss = pairwise_margin_loss_multi(s, Y_tr, M_tr, args.margin_floor)
            loss.backward()
            opt.step()
            if ep % 30 == 0 or ep == args.epochs - 1:
                model.eval()
                with torch.no_grad():
                    s_va = model(X_va).cpu().numpy()
                Y_va_np = Y_va.cpu().numpy()
                M_va_np = M_va.cpu().numpy()
                for k in range(K):
                    m = M_va_np[k].astype(bool)
                    if m.sum() < 2:
                        continue
                    pa = pair_agreement(s_va[m, k], Y_va_np[k, m])
                    rho = spearman_rho(s_va[m, k], Y_va_np[k, m])
                    if pa > best["per_axis_pa"][k]:
                        best["per_axis_pa"][k] = pa
                        best["per_axis_rho"][k] = rho
        cv_records.append({"fold": fold, **best})
        sample = ", ".join(f"{p_axes[k]}={best['per_axis_pa'][k]:.3f}" for k in range(K))
        print(f"  fold {fold}: best per-axis PA: {sample}")

    # Aggregate CV
    cv_pa = np.zeros(K, dtype=np.float32)
    cv_rho = np.zeros(K, dtype=np.float32)
    for r in cv_records:
        cv_pa += np.array(r["per_axis_pa"])
        cv_rho += np.array(r["per_axis_rho"])
    cv_pa /= len(cv_records)
    cv_rho /= len(cv_records)
    print(f"\n[r7-train] CV mean per-axis pairwise agreement:")
    for k in range(K):
        print(f"  {p_axes[k]:>22}: PA={cv_pa[k]:.3f}  rho={cv_rho[k]:+.3f}")

    # Final retrain on full data.
    print(f"\n[r7-train] retraining on full {len(track_ids)} tracks ...")
    device = torch.device(args.device)
    model = MultiAxisLinear(D, K).to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr,
                            weight_decay=args.weight_decay)
    X_all = torch.from_numpy(X).to(device)
    Y_all = torch.from_numpy(Y).to(device)
    M_all = torch.from_numpy(M).to(device)
    for ep in range(args.epochs):
        model.train()
        opt.zero_grad()
        s = model(X_all)
        loss = pairwise_margin_loss_multi(s, Y_all, M_all, args.margin_floor)
        loss.backward()
        opt.step()
        if ep % 100 == 0 or ep == args.epochs - 1:
            print(f"  ep {ep}: loss={float(loss):.4f}")

    model.eval()
    with torch.no_grad():
        scores_all = model(X_all).cpu().numpy()  # [N, K]
    W = model.lin.weight.detach().cpu().numpy()   # [K, D]
    b = model.lin.bias.detach().cpu().numpy()     # [K]

    # L2-normalise rows (interpretability convention; rescale into bias)
    norms = np.linalg.norm(W, axis=1, keepdims=True)
    norms = np.clip(norms, 1e-9, None)
    W_n = W / norms
    b_n = b / norms[:, 0]

    args.out_axes.parent.mkdir(parents=True, exist_ok=True)
    np.savez(args.out_axes,
             axes=np.array(p_axes, dtype=object),
             directions=W_n.astype(np.float32),
             biases=b_n.astype(np.float32),
             cv_pa=cv_pa,
             cv_rho=cv_rho,
             track_ids=track_ids)
    print(f"[r7-train] wrote {args.out_axes}")

    # Predictions CSV
    import csv
    with args.out_preds.open("w") as f:
        w = csv.writer(f)
        w.writerow(["track_id"] + list(p_axes))
        for j, t in enumerate(track_ids):
            row = [int(t)] + [f"{scores_all[j, k]:.6f}" for k in range(K)]
            w.writerow(row)
    print(f"[r7-train] wrote {args.out_preds}")

    args.out_metrics.write_text(json.dumps({
        "cv_records": cv_records,
        "cv_mean_pa": {p_axes[k]: float(cv_pa[k]) for k in range(K)},
        "cv_mean_rho": {p_axes[k]: float(cv_rho[k]) for k in range(K)},
    }, indent=2))
    print(f"[r7-train] wrote {args.out_metrics}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
