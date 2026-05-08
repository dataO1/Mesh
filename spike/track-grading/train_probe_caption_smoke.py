"""Caption-as-feature probe transfer test (Phase 1 smoke).

Tests whether MF caption embeddings carry signal that transfers to the
deployed V15 axis. Uses the 200-track caption smoke as both training and
evaluation set via 5-fold CV.

Three feature configurations are compared:
  1. MuQ-MuLan only (512d) — baseline
  2. caption_emb only (768d) — MF caption alone
  3. concat (1280d) — both stacked

Each predicts V15 intensity score (= MuQ-MuLan @ V15 axis vector) using
ridge regression. CV-mean Spearman ρ and R² are reported.

Pass criteria (Phase 1 → Phase 2 gate):
  - caption_emb-only Spearman ρ > 0.6 against V15
  - concat Spearman ρ > MuQ-MuLan baseline (caption adds info)
  - both bge / mpnet embeddings yield similar spread (sanity)

Usage:
    bash spike/track-grading/run_r7_step.sh train_probe_caption_smoke.py \
         --caption-emb /home/data01/Music/mesh-track-grading/round7_6_caption_emb.npz
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np


EMBS = "/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz"
V15 = "models/aggression-axes/V15_linear_probe_r6.json"
V17B = "models/aggression-axes/V17_round7_5_polar_blend.json"
PRIORS = "/home/data01/Music/mesh-track-grading/round7_5_priors.npz"


def spearman(a, b):
    a, b = np.asarray(a, dtype=float), np.asarray(b, dtype=float)
    mask = ~(np.isnan(a) | np.isnan(b))
    a, b = a[mask], b[mask]
    n = len(a)
    if n < 3:
        return float("nan")
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    return 1.0 - 6.0 * float(np.sum((ra - rb) ** 2)) / (n * (n * n - 1))


def ridge_cv(X, y, k=5, alpha=1.0, seed=42):
    """5-fold CV ridge regression. Returns mean Spearman, mean R², std-spearman."""
    rng = np.random.default_rng(seed)
    n = len(X)
    idx = np.arange(n); rng.shuffle(idx)
    folds = np.array_split(idx, k)
    rhos, r2s = [], []
    for i in range(k):
        test_idx = folds[i]
        train_idx = np.concatenate([folds[j] for j in range(k) if j != i])
        Xtr, ytr = X[train_idx], y[train_idx]
        Xte, yte = X[test_idx], y[test_idx]
        # ridge: w = (X^T X + αI)^-1 X^T y
        XtX = Xtr.T @ Xtr
        XtX += alpha * np.eye(XtX.shape[0])
        w = np.linalg.solve(XtX, Xtr.T @ ytr)
        pred = Xte @ w
        rhos.append(spearman(pred, yte))
        ss_res = float(np.sum((yte - pred) ** 2))
        ss_tot = float(np.sum((yte - np.mean(yte)) ** 2))
        r2s.append(1 - ss_res / max(ss_tot, 1e-9))
    return float(np.mean(rhos)), float(np.std(rhos)), float(np.mean(r2s))


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--caption-emb", type=Path, required=True)
    p.add_argument("--alpha", type=float, default=1.0,
                   help="ridge regularisation")
    p.add_argument("--folds", type=int, default=5)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def main(args) -> int:
    # Load corpus MuQ-MuLan embeddings
    e = np.load(EMBS, allow_pickle=True)
    emb_tids = e["track_ids"].astype(np.int64)
    embs = e["embeddings"].astype(np.float32)
    tid_to_emb = {int(t): i for i, t in enumerate(emb_tids)}

    # Load V15 / V17b deployed axes (anchor labels)
    v15 = json.loads(open(V15).read())
    v15_vec = np.asarray(v15["intensity_axis_vec"], dtype=np.float32)
    v17b = json.loads(open(V17B).read())
    v17b_vec = np.asarray(v17b["intensity_axis_vec"], dtype=np.float32)

    # Load caption embeddings
    c = np.load(args.caption_emb, allow_pickle=True)
    cap_tids = c["track_ids"].astype(np.int64)
    cap_emb = c["caption_emb"].astype(np.float32)
    cap_lengths = c["caption_lengths"]
    print(f"[probe] caption emb: {cap_emb.shape}, model={c['model_name']}")
    print(f"[probe] caption lengths (words): "
          f"min={int(cap_lengths.min())} median={int(np.median(cap_lengths))} "
          f"max={int(cap_lengths.max())}")

    # Align: keep tracks that have BOTH MuQ-MuLan emb AND a caption
    keep_tids = []
    audio_X, cap_X = [], []
    for i, tid in enumerate(cap_tids):
        ei = tid_to_emb.get(int(tid))
        if ei is not None:
            keep_tids.append(int(tid))
            audio_X.append(embs[ei])
            cap_X.append(cap_emb[i])
    audio_X = np.stack(audio_X, axis=0)
    cap_X = np.stack(cap_X, axis=0)
    keep_tids = np.array(keep_tids, dtype=np.int64)
    print(f"[probe] {len(keep_tids)} tracks have both audio and caption")

    # Anchor labels: V15 / V17b intensity scores via dot-product
    y_v15 = audio_X @ v15_vec
    y_v17b = audio_X @ v17b_vec
    print(f"[probe] V15 score range: [{y_v15.min():.2f}, {y_v15.max():.2f}]")
    print(f"[probe] V17b score range: [{y_v17b.min():.2f}, {y_v17b.max():.2f}]")

    # Per-axis labels: BT priors from r7.5 (per-axis Spearman target — noisier)
    z = np.load(PRIORS, allow_pickle=True)
    bt_tids = z["track_ids"].astype(np.int64)
    bt_axes = list(z["axes"])
    bt_scores = z["scores"]  # (16, 15314)
    tid_to_bt = {int(t): i for i, t in enumerate(bt_tids)}
    bt_idx_smoke = np.array([tid_to_bt.get(int(t), -1) for t in keep_tids])
    have_bt = bt_idx_smoke >= 0
    print(f"[probe] {int(have_bt.sum())}/{len(keep_tids)} tracks have r7.5 BT priors")

    # ── ridge CV across three feature configs ──────────────────────────
    print()
    print(f"=== ridge CV ({args.folds}-fold, alpha={args.alpha}) ===")

    print()
    print("--- target: V15 deployed-axis intensity ---")
    for label, X in [
        ("MuQ-MuLan(512)        ", audio_X),
        ("caption_emb(768)      ", cap_X),
        ("concat(1280)          ", np.concatenate([audio_X, cap_X], axis=1)),
    ]:
        rho, rho_std, r2 = ridge_cv(X, y_v15, k=args.folds, alpha=args.alpha, seed=args.seed)
        print(f"  {label}  ρ={rho:+.3f} ± {rho_std:.3f}   R²={r2:+.3f}")

    print()
    print("--- target: V17b polar-blend intensity ---")
    for label, X in [
        ("MuQ-MuLan(512)        ", audio_X),
        ("caption_emb(768)      ", cap_X),
        ("concat(1280)          ", np.concatenate([audio_X, cap_X], axis=1)),
    ]:
        rho, rho_std, r2 = ridge_cv(X, y_v17b, k=args.folds, alpha=args.alpha, seed=args.seed)
        print(f"  {label}  ρ={rho:+.3f} ± {rho_std:.3f}   R²={r2:+.3f}")

    # Per-axis evaluation: use r7.5 BT priors (more noise but closer to what
    # the real probe will train against in Phase 2)
    if have_bt.sum() >= 50:
        print()
        print("--- target: r7.5 BT priors (per-axis, MuQ-MuLan vs caption_emb) ---")
        print(f"   {'axis':25s}  {'audio':>15s}  {'caption':>15s}  {'concat':>15s}")
        ax_audio = audio_X[have_bt]
        ax_cap = cap_X[have_bt]
        ax_concat = np.concatenate([ax_audio, ax_cap], axis=1)
        for axis_id in bt_axes:
            j = bt_axes.index(axis_id)
            y_axis = bt_scores[j, bt_idx_smoke[have_bt]]
            valid = ~np.isnan(y_axis)
            if valid.sum() < 50:
                print(f"   {axis_id:25s}  (insufficient data)")
                continue
            row = []
            for X_ in (ax_audio[valid], ax_cap[valid], ax_concat[valid]):
                rho, _, _ = ridge_cv(X_, y_axis[valid], k=args.folds,
                                       alpha=args.alpha, seed=args.seed)
                row.append(rho)
            print(f"   {axis_id:25s}  "
                  f"ρ={row[0]:>+5.3f}±        ρ={row[1]:>+5.3f}±        ρ={row[2]:>+5.3f}±")

    print()
    print("=== gate criteria ===")
    print("  caption_emb-only ρ vs V15 > 0.6  → caption signal exists")
    print("  concat ρ > MuQ-MuLan ρ           → caption adds info beyond audio emb")
    print("  per-axis concat > audio on >= 8 axes → useful for round-7.6 multi-task probe")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
