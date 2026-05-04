"""Round 6 — train MLP and linear-probe heads on MuQ-MuLan embeddings.

Inputs:
  - /tmp/track-grading/embeddings.npz   (track_ids[N], embeddings[N, 512])
  - documents/axis-eval-results/llm-pair-priors-r5.txt  (BT priors as labels)

Outputs:
  - /tmp/track-grading/predictions_mlp.csv     (track_id, predicted_intensity)
  - /tmp/track-grading/predictions_linear.csv  (linear-probe baseline)
  - documents/axis-eval-results/V14_mlp_head_r6.csv     (in V*.csv format
       for compare-variants.py)
  - documents/axis-eval-results/V15_linear_probe_r6.csv (linear-probe baseline)
  - /tmp/track-grading/round6_metrics.json    (CV scores, both models)

Loss: pairwise margin (RankNet-style). For all (i,j) with |y_i - y_j| > 0.5,
penalise predictions that disagree on the order of i,j by margin proportional
to the BT delta.

5-fold CV stratified by BT prior bucket. Final model retrained on full data.
"""
from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from sklearn.model_selection import StratifiedKFold


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--embeddings", type=Path,
                   default=Path("/tmp/track-grading/embeddings.npz"))
    p.add_argument("--priors", type=Path,
                   default=Path("documents/axis-eval-results/llm-pair-priors-r5.txt"))
    p.add_argument("--out-dir", type=Path,
                   default=Path("/tmp/track-grading"))
    p.add_argument("--variant-dir", type=Path,
                   default=Path("documents/axis-eval-results"))
    p.add_argument("--epochs", type=int, default=400)
    p.add_argument("--lr", type=float, default=1e-3)
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--folds", type=int, default=5)
    return p.parse_args()


class MLPHead(nn.Module):
    def __init__(self, in_dim: int = 512):
        super().__init__()
        self.net = nn.Sequential(
            nn.Linear(in_dim, 128),
            nn.ReLU(),
            nn.Dropout(0.2),
            nn.Linear(128, 64),
            nn.ReLU(),
            nn.Linear(64, 1),
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.net(x).squeeze(-1)


class LinearProbe(nn.Module):
    def __init__(self, in_dim: int = 512):
        super().__init__()
        self.lin = nn.Linear(in_dim, 1)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.lin(x).squeeze(-1)


def load_data(emb_path: Path, prior_path: Path):
    npz = np.load(emb_path)
    ids = npz["track_ids"]
    embs = npz["embeddings"]

    priors: dict[int, float] = {}
    with prior_path.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"): continue
            parts = line.split("|", 2)
            if len(parts) == 3:
                priors[int(parts[0])] = float(parts[2])

    # Align embeddings + priors by track_id
    mask = np.array([int(t) in priors for t in ids])
    ids = ids[mask]
    embs = embs[mask]
    y = np.array([priors[int(t)] for t in ids], dtype=np.float32)
    return ids, embs, y


def pairwise_margin_loss(scores: torch.Tensor, y: torch.Tensor,
                         margin_floor: float = 0.5) -> torch.Tensor:
    """For all i<j with |y_i-y_j|>margin_floor, penalize if model disagrees."""
    n = scores.shape[0]
    # Pairwise sign of label difference (+1 or -1) and magnitude
    y_diff = y.unsqueeze(0) - y.unsqueeze(1)              # [n, n]
    s_diff = scores.unsqueeze(0) - scores.unsqueeze(1)    # [n, n]
    # Triangular mask (only i<j) and threshold filter
    mask_tri = torch.triu(torch.ones(n, n, device=y.device), diagonal=1).bool()
    mask_thresh = y_diff.abs() > margin_floor
    mask = mask_tri & mask_thresh
    if not mask.any():
        return torch.tensor(0.0, device=y.device, requires_grad=True)
    # margin loss: max(0, margin - sign(y_i-y_j) * (s_i-s_j))
    sign = y_diff.sign()
    margin = y_diff.abs()  # margin proportional to label gap
    loss_mat = F.relu(margin - sign * s_diff)
    return loss_mat[mask].mean()


def pair_agreement(scores: np.ndarray, y: np.ndarray) -> float:
    n = len(scores)
    rank_s = np.argsort(np.argsort(scores))
    rank_y = np.argsort(np.argsort(y))
    concordant = discordant = 0
    for i in range(n):
        for j in range(i+1, n):
            ds = rank_s[i] - rank_s[j]
            dy = rank_y[i] - rank_y[j]
            if ds == 0 or dy == 0: continue
            if (ds > 0) == (dy > 0): concordant += 1
            else: discordant += 1
    total = concordant + discordant
    return concordant / total if total else 0.0


def spearman_rho(a: np.ndarray, b: np.ndarray) -> float:
    n = len(a)
    if n < 2: return 0.0
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    sum_d2 = float(np.sum((ra - rb) ** 2))
    return 1 - 6 * sum_d2 / (n * (n * n - 1))


def train_model(model: nn.Module, X_train: torch.Tensor, y_train: torch.Tensor,
                X_val: torch.Tensor, y_val: torch.Tensor,
                epochs: int, lr: float) -> tuple[float, float]:
    """Returns (best_val_pairwise_agreement, best_val_spearman)."""
    opt = torch.optim.AdamW(model.parameters(), lr=lr, weight_decay=1e-4)
    best_pa = 0.0
    best_rho = 0.0
    for ep in range(epochs):
        model.train()
        opt.zero_grad()
        s = model(X_train)
        loss = pairwise_margin_loss(s, y_train)
        loss.backward()
        opt.step()
        if ep % 20 == 0 or ep == epochs - 1:
            model.eval()
            with torch.no_grad():
                s_val = model(X_val).cpu().numpy()
            pa = pair_agreement(s_val, y_val.cpu().numpy())
            rho = spearman_rho(s_val, y_val.cpu().numpy())
            if pa > best_pa:
                best_pa = pa; best_rho = rho
    return best_pa, best_rho


def main() -> int:
    args = parse_args()
    torch.manual_seed(args.seed)
    np.random.seed(args.seed)

    ids, X, y = load_data(args.embeddings, args.priors)
    print(f"[r6] {len(ids)} tracks, embedding dim={X.shape[1]}")

    # Stratify by BT bucket (rounded prior)
    bucket = np.clip(y.astype(int), 0, 10)
    skf = StratifiedKFold(n_splits=args.folds, shuffle=True, random_state=args.seed)

    results = {"mlp": [], "linear": []}
    for kind, ctor in [("linear", LinearProbe), ("mlp", MLPHead)]:
        print(f"\n[r6] === {kind} ===")
        for fold, (tr, va) in enumerate(skf.split(X, bucket)):
            X_tr = torch.from_numpy(X[tr]).float()
            X_va = torch.from_numpy(X[va]).float()
            y_tr = torch.from_numpy(y[tr]).float()
            y_va = torch.from_numpy(y[va]).float()
            model = ctor(in_dim=X.shape[1])
            pa, rho = train_model(model, X_tr, y_tr, X_va, y_va, args.epochs, args.lr)
            results[kind].append({"fold": fold, "pa": pa, "rho": rho})
            print(f"  fold {fold}: pa={pa:.4f} rho={rho:+.4f}")
        mean_pa = np.mean([r["pa"] for r in results[kind]])
        mean_rho = np.mean([r["rho"] for r in results[kind]])
        print(f"  CV mean: pa={mean_pa:.4f} rho={mean_rho:+.4f}")

    # Final retrain on full data, output predictions
    args.out_dir.mkdir(parents=True, exist_ok=True)
    args.variant_dir.mkdir(parents=True, exist_ok=True)

    title_lookup: dict[int, tuple[str, str]] = {}
    track_list = Path("/tmp/track-grading/_track-list.csv")
    if track_list.exists():
        with track_list.open() as f:
            for r in csv.DictReader(f):
                title_lookup[int(r["track_id"])] = (r["title"], r["artist"])

    for kind, ctor in [("linear", LinearProbe), ("mlp", MLPHead)]:
        model = ctor(in_dim=X.shape[1])
        X_t = torch.from_numpy(X).float()
        y_t = torch.from_numpy(y).float()
        opt = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=1e-4)
        for ep in range(args.epochs):
            model.train()
            opt.zero_grad()
            loss = pairwise_margin_loss(model(X_t), y_t)
            loss.backward()
            opt.step()
        model.eval()
        with torch.no_grad():
            preds = model(X_t).numpy()
        # Normalize predictions to [0,10] to match V*.csv intensity column
        pmin, pmax = float(preds.min()), float(preds.max())
        norm = (preds - pmin) / (pmax - pmin + 1e-9)
        intensity_0_10 = norm * 10.0

        # Sort by intensity desc → rank 1..N
        order = np.argsort(-intensity_0_10)
        # V*.csv format: rank, track_id, title, artist, intensity, then sub-axes
        # We don't have sub-axes here so we emit just the intensity column.
        out_v = args.variant_dir / (
            "V14_mlp_head_r6.csv" if kind == "mlp" else "V15_linear_probe_r6.csv"
        )
        with out_v.open("w") as f:
            w = csv.writer(f)
            w.writerow(["rank", "track_id", "title", "artist", "intensity"])
            for r, idx in enumerate(order):
                tid = int(ids[idx])
                title, artist = title_lookup.get(tid, ("", ""))
                w.writerow([r+1, tid, title, artist, f"{intensity_0_10[idx]:.6f}"])
        print(f"[r6] wrote {out_v}")

        # Also write a simple track_id -> intensity prediction CSV
        out_p = args.out_dir / f"predictions_{kind}.csv"
        with out_p.open("w") as f:
            w = csv.writer(f)
            w.writerow(["track_id", "intensity_0_10"])
            for tid, p in zip(ids, intensity_0_10):
                w.writerow([int(tid), f"{p:.6f}"])

    metrics_path = args.out_dir / "round6_metrics.json"
    metrics_path.write_text(json.dumps({
        "linear_cv": results["linear"],
        "mlp_cv": results["mlp"],
        "linear_mean_pa": float(np.mean([r["pa"] for r in results["linear"]])),
        "mlp_mean_pa": float(np.mean([r["pa"] for r in results["mlp"]])),
        "linear_mean_rho": float(np.mean([r["rho"] for r in results["linear"]])),
        "mlp_mean_rho": float(np.mean([r["rho"] for r in results["mlp"]])),
    }, indent=2))
    print(f"[r6] wrote {metrics_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
