"""Round-7.5 multi-task linear probes with auxiliary tag-prediction loss.

Architecture:

  embedding (512) ──┬──► Linear(512, K=16)  →  axis scores (K)
                    │
                    └──► Linear(512, T=13)  →  tag logits (T)

Loss = pairwise_margin(axis_scores, BT_priors) + λ · BCE(tag_logits, tag_targets)

The auxiliary tag head shares the input embedding but has its own weights.
The auxiliary BCE loss acts as a regulariser: the embedding has to retain
features that predict tags from the same justifications that produced the
BT scores, which usually buys 1–2 pp on the main pairwise-agreement metric.

Inputs:
  /tmp/track-grading/embeddings/corpus_muq_mulan.npz
  /tmp/track-grading/round7_5_priors.npz
  /tmp/track-grading/round7_5_tags.npz   (tag_evidence per track)

Outputs:
  /tmp/track-grading/round7_5_axes.npz   (16 directions + biases + CV stats)
  /tmp/track-grading/round7_5_predictions.csv
  /tmp/track-grading/round7_5_train_metrics.json

Per-track tag target: if a track has |evidence_signed| ≥ MIN_EVIDENCE for a
tag, target = 1.0 if evidence > 0 else 0.0. Tracks below the evidence floor
get masked from the BCE loss for that tag (don't penalise unknowns).
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
                   default=Path("/tmp/track-grading/round7_5_priors.npz"))
    p.add_argument("--tags", type=Path,
                   default=Path("/tmp/track-grading/round7_5_tags.npz"))
    p.add_argument("--out-axes", type=Path,
                   default=Path("/tmp/track-grading/round7_5_axes.npz"))
    p.add_argument("--out-preds", type=Path,
                   default=Path("/tmp/track-grading/round7_5_predictions.csv"))
    p.add_argument("--out-metrics", type=Path,
                   default=Path("/tmp/track-grading/round7_5_train_metrics.json"))
    p.add_argument("--epochs", type=int, default=600)
    p.add_argument("--lr", type=float, default=2e-3)
    p.add_argument("--weight-decay", type=float, default=1e-3)
    p.add_argument("--folds", type=int, default=5)
    p.add_argument("--margin-floor", type=float, default=0.5)
    p.add_argument("--lambda-aux", type=float, default=0.3,
                   help="weight of auxiliary tag-prediction BCE loss")
    p.add_argument("--min-tag-evidence", type=float, default=2.0,
                   help="minimum |signed evidence| to use track as a tag-supervision example")
    p.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


class MultiAxisLinearWithTagHead(nn.Module):
    def __init__(self, in_dim: int, n_axes: int, n_tags: int):
        super().__init__()
        self.axis_head = nn.Linear(in_dim, n_axes)
        self.tag_head = nn.Linear(in_dim, n_tags)

    def forward(self, x: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor]:
        return self.axis_head(x), self.tag_head(x)


def pairwise_margin_loss_multi(scores: torch.Tensor,
                               targets: torch.Tensor,
                               mask: torch.Tensor,
                               margin_floor: float) -> torch.Tensor:
    """scores [N, K], targets [K, N], mask [K, N]."""
    K, _ = targets.shape
    losses = []
    for k in range(K):
        m = mask[k].bool()
        if m.sum() < 2:
            continue
        s = scores[m, k]
        y = targets[k, m]
        s_diff = s.unsqueeze(0) - s.unsqueeze(1)
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


def tag_bce_loss(logits: torch.Tensor,
                 tag_targets: torch.Tensor,
                 tag_mask: torch.Tensor) -> torch.Tensor:
    """logits [N, T], targets [N, T] in {0, 1}, mask [N, T] in {0, 1}.
    Only count loss where mask = 1 (track had evidence for that tag)."""
    if tag_mask.sum() == 0:
        return torch.tensor(0.0, device=logits.device, requires_grad=True)
    loss_mat = F.binary_cross_entropy_with_logits(
        logits, tag_targets, reduction="none")
    return (loss_mat * tag_mask).sum() / tag_mask.sum().clamp(min=1.0)


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
    p_priors = pri["priors_0_10"].astype(np.float32)
    p_games = pri["n_games"].astype(np.int32)

    K = p_priors.shape[0]
    D = e_X.shape[1]
    print(f"[r7.5-train] axes K={K}: {p_axes}")
    print(f"[r7.5-train] embedding D={D} on {len(e_ids)} tracks")
    print(f"[r7.5-train] priors on {len(p_ids)} tracks")

    # Tags (optional but expected)
    tag_names: list[str] = []
    track_to_tag_targets: dict[int, np.ndarray] = {}
    track_to_tag_mask: dict[int, np.ndarray] = {}
    if args.tags.exists():
        t = np.load(args.tags, allow_pickle=True)
        tag_names = list(t["tag_names"])
        tag_evidence = t["tag_evidence"].astype(np.float32)
        tag_track_ids = t["track_ids"].astype(np.int64)
        for i, tid in enumerate(tag_track_ids):
            ev = tag_evidence[i]
            mask = (np.abs(ev) >= args.min_tag_evidence).astype(np.float32)
            target = (ev > 0).astype(np.float32)
            track_to_tag_targets[int(tid)] = target
            track_to_tag_mask[int(tid)] = mask
        print(f"[r7.5-train] tags: {len(tag_names)} types, "
              f"{len(track_to_tag_targets)} tracks with evidence "
              f"(min_ev={args.min_tag_evidence})")
    else:
        print(f"[r7.5-train] WARNING: no tag file at {args.tags}; running without aux loss")

    # Align embeddings + priors by track_id
    e_idx = {int(t): i for i, t in enumerate(e_ids)}
    common = [int(t) for t in p_ids if int(t) in e_idx]
    print(f"[r7.5-train] common tracks: {len(common)}")

    track_ids = np.array(common, dtype=np.int64)
    X = np.array([e_X[e_idx[int(t)]] for t in track_ids], dtype=np.float32)
    p_idx = {int(t): i for i, t in enumerate(p_ids)}
    Y = np.zeros((K, len(track_ids)), dtype=np.float32)
    M = np.zeros((K, len(track_ids)), dtype=np.float32)
    for j, t in enumerate(track_ids):
        i = p_idx[int(t)]
        Y[:, j] = p_priors[:, i]
        M[:, j] = (p_games[:, i] > 0).astype(np.float32)

    T = len(tag_names)
    if T > 0:
        Y_tag = np.zeros((len(track_ids), T), dtype=np.float32)
        M_tag = np.zeros((len(track_ids), T), dtype=np.float32)
        for j, t in enumerate(track_ids):
            if int(t) in track_to_tag_targets:
                Y_tag[j] = track_to_tag_targets[int(t)]
                M_tag[j] = track_to_tag_mask[int(t)]
    else:
        Y_tag = np.zeros((len(track_ids), 0), dtype=np.float32)
        M_tag = np.zeros((len(track_ids), 0), dtype=np.float32)

    folds = kfold_indices(len(track_ids), args.folds, args.seed)
    cv_records = []
    print(f"[r7.5-train] {args.folds}-fold CV "
          f"(margin-floor={args.margin_floor}, λ_aux={args.lambda_aux})")
    for fold, (tr, va) in enumerate(folds):
        device = torch.device(args.device)
        model = MultiAxisLinearWithTagHead(D, K, T).to(device)
        opt = torch.optim.AdamW(model.parameters(), lr=args.lr,
                                weight_decay=args.weight_decay)
        X_tr = torch.from_numpy(X[tr]).to(device)
        Y_tr = torch.from_numpy(Y[:, tr]).to(device)
        M_tr = torch.from_numpy(M[:, tr]).to(device)
        Yt_tr = torch.from_numpy(Y_tag[tr]).to(device)
        Mt_tr = torch.from_numpy(M_tag[tr]).to(device)
        X_va = torch.from_numpy(X[va]).to(device)
        Y_va = Y[:, va]
        M_va = M[:, va]

        best = {"per_axis_pa": [0.0] * K, "per_axis_rho": [0.0] * K}
        for ep in range(args.epochs):
            model.train()
            opt.zero_grad()
            s, tg = model(X_tr)
            loss_main = pairwise_margin_loss_multi(s, Y_tr, M_tr, args.margin_floor)
            if T > 0 and args.lambda_aux > 0:
                loss_aux = tag_bce_loss(tg, Yt_tr, Mt_tr)
                loss = loss_main + args.lambda_aux * loss_aux
            else:
                loss = loss_main
            loss.backward()
            opt.step()
            if ep % 30 == 0 or ep == args.epochs - 1:
                model.eval()
                with torch.no_grad():
                    s_va, _ = model(X_va)
                s_va_np = s_va.cpu().numpy()
                for k in range(K):
                    m = M_va[k].astype(bool)
                    if m.sum() < 2:
                        continue
                    pa = pair_agreement(s_va_np[m, k], Y_va[k, m])
                    rho = spearman_rho(s_va_np[m, k], Y_va[k, m])
                    if pa > best["per_axis_pa"][k]:
                        best["per_axis_pa"][k] = pa
                        best["per_axis_rho"][k] = rho
        cv_records.append({"fold": fold, **best})
        sample = ", ".join(f"{p_axes[k]}={best['per_axis_pa'][k]:.3f}" for k in range(K))
        print(f"  fold {fold}: best per-axis PA: {sample}")

    cv_pa = np.zeros(K, dtype=np.float32)
    cv_rho = np.zeros(K, dtype=np.float32)
    for r in cv_records:
        cv_pa += np.array(r["per_axis_pa"])
        cv_rho += np.array(r["per_axis_rho"])
    cv_pa /= len(cv_records)
    cv_rho /= len(cv_records)
    print(f"\n[r7.5-train] CV mean per-axis PA / rho:")
    for k in range(K):
        print(f"  {p_axes[k]:>22}: PA={cv_pa[k]:.3f}  rho={cv_rho[k]:+.3f}")

    # Final retrain on full data
    print(f"\n[r7.5-train] retraining on full {len(track_ids)} tracks ...")
    device = torch.device(args.device)
    model = MultiAxisLinearWithTagHead(D, K, T).to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr,
                            weight_decay=args.weight_decay)
    X_all = torch.from_numpy(X).to(device)
    Y_all = torch.from_numpy(Y).to(device)
    M_all = torch.from_numpy(M).to(device)
    Yt_all = torch.from_numpy(Y_tag).to(device)
    Mt_all = torch.from_numpy(M_tag).to(device)
    for ep in range(args.epochs):
        model.train()
        opt.zero_grad()
        s, tg = model(X_all)
        loss_main = pairwise_margin_loss_multi(s, Y_all, M_all, args.margin_floor)
        if T > 0 and args.lambda_aux > 0:
            loss_aux = tag_bce_loss(tg, Yt_all, Mt_all)
            loss = loss_main + args.lambda_aux * loss_aux
        else:
            loss_aux = torch.tensor(0.0)
            loss = loss_main
        loss.backward()
        opt.step()
        if ep % 100 == 0 or ep == args.epochs - 1:
            print(f"  ep {ep}: main={float(loss_main):.4f} "
                  f"aux={float(loss_aux):.4f}")

    model.eval()
    with torch.no_grad():
        scores_all, _ = model(X_all)
        scores_all = scores_all.cpu().numpy()
    W = model.axis_head.weight.detach().cpu().numpy()
    b = model.axis_head.bias.detach().cpu().numpy()
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
             track_ids=track_ids,
             tag_names=np.array(tag_names, dtype=object))
    print(f"[r7.5-train] wrote {args.out_axes}")

    import csv
    with args.out_preds.open("w") as f:
        w = csv.writer(f)
        w.writerow(["track_id"] + list(p_axes))
        for j, t in enumerate(track_ids):
            row = [int(t)] + [f"{scores_all[j, k]:.6f}" for k in range(K)]
            w.writerow(row)
    print(f"[r7.5-train] wrote {args.out_preds}")

    args.out_metrics.write_text(json.dumps({
        "cv_records": cv_records,
        "cv_mean_pa": {p_axes[k]: float(cv_pa[k]) for k in range(K)},
        "cv_mean_rho": {p_axes[k]: float(cv_rho[k]) for k in range(K)},
        "lambda_aux": args.lambda_aux,
        "n_tracks_with_tags": int((M_tag.sum(axis=1) > 0).sum()),
        "tag_names": tag_names,
    }, indent=2))
    print(f"[r7.5-train] wrote {args.out_metrics}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
