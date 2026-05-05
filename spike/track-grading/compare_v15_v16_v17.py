"""V15 vs V16 vs V17 head-to-head on the user's round-5 BT priors.

Round-5 BT is the gold standard for the user's library. V15 was trained on
those (in-domain). V16 (round-7) and V17 (round-7.5) were trained on Deezer
corpus (out-of-domain). We compute Spearman + PA for each, plus pairwise
disagreement: which tracks does V17 agree with V15 that V16 missed, and
vice versa.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--db", default="/home/data01/Music/mesh-collection/mesh.db")
    p.add_argument("--priors",
                   default="documents/axis-eval-results/llm-pair-priors-r5.txt")
    p.add_argument("--top-n", type=int, default=15,
                   help="how many top-disagreement tracks to print")
    return p.parse_args()


# argparse first; only set LD_LIBRARY_PATH and import cozo after we know we
# need them (so --help works without zlib/libstdc++ on path).
ARGS = parse_args() if __name__ == "__main__" else None

os.environ["LD_LIBRARY_PATH"] = (
    "/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:"
    "/nix/store/1xw5xccqqh1xw3mvd70hyil6x418wxcm-gcc-14.3.0-lib/lib:"
    + os.environ.get("LD_LIBRARY_PATH", "")
)

import numpy as np


def spearman_rho(a, b):
    n = len(a)
    if n < 2: return 0.0
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    return 1 - 6 * float(np.sum((ra - rb) ** 2)) / (n * (n * n - 1))


def pair_agreement(scores, y):
    n = len(scores)
    if n < 2: return 0.0
    ds = scores[:, None] - scores[None, :]
    dy = y[:, None] - y[None, :]
    tri = np.triu(np.ones((n, n), dtype=bool), k=1)
    valid = tri & (ds != 0) & (dy != 0)
    concordant = (valid & ((ds > 0) == (dy > 0))).sum()
    total = valid.sum()
    return float(concordant / total) if total else 0.0


def main():
    args = ARGS
    from pycozo.client import Client
    db = Client("sqlite", args.db, {"dataframe": False})
    rows = db.run("?[track_id, vec] := *ml_embeddings{track_id, vec}")["rows"]
    track_ids, embs = [], []
    for tid, vec in rows:
        if vec is not None:
            track_ids.append(int(tid)); embs.append(vec)
    track_ids = np.array(track_ids, dtype=np.int64)
    embs = np.array(embs, dtype=np.float32)

    priors_path = Path(args.priors)
    bt = {}
    with priors_path.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"): continue
            parts = line.split("|", 2)
            if len(parts) == 3:
                bt[int(parts[0])] = float(parts[2])

    mask = np.array([t in bt for t in track_ids])
    track_ids = track_ids[mask]
    embs = embs[mask]
    y = np.array([bt[t] for t in track_ids], dtype=np.float32)
    print(f"[cmp] aligned: {len(track_ids)} tracks (user library × round-5 BT)")

    variants = []
    for path, label in [
        ("models/aggression-axes/V15_linear_probe_r6.json", "V15 (round-6 in-domain)"),
        ("models/aggression-axes/V16_round7_blend.json", "V16 (round-7 single-word out-of-domain)"),
        ("models/aggression-axes/V17_round7_5_polar_blend.json", "V17 (round-7.5 polar+BALD out-of-domain)"),
    ]:
        if not Path(path).exists():
            print(f"[cmp] skip {label}: {path} missing"); continue
        v = json.loads(Path(path).read_text())
        vec = np.array(v["intensity_axis_vec"], dtype=np.float32)
        score = embs @ vec
        variants.append((label, vec, score))

    print()
    print(f"{'Variant':<55} | {'Spearman ρ':>10} | {'PA':>6}")
    print("-" * 80)
    for label, vec, score in variants:
        print(f"{label:<55} | {spearman_rho(score, y):+10.4f} | {pair_agreement(score, y):6.4f}")

    print("\nInter-variant Spearman:")
    for i in range(len(variants)):
        for j in range(i + 1, len(variants)):
            la, _, sa = variants[i]
            lb, _, sb = variants[j]
            print(f"  {la} ↔ {lb}: {spearman_rho(sa, sb):+.4f}")

    # Disagreement: V17 - V15 and V17 - V16
    if len(variants) == 3:
        s15, s16, s17 = variants[0][2], variants[1][2], variants[2][2]
        d_v17_v15 = s17 - s15
        d_v17_v16 = s17 - s16
        print(f"\nTracks where V17 most disagrees with V15 (V17 thinks more aggressive):")
        order = np.argsort(-np.abs(d_v17_v15))
        for idx in order[: args.top_n]:
            tid = int(track_ids[idx])
            r = db.run("?[title, artist] := *tracks{id: $id, title, artist}", {"id": tid})["rows"]
            t, a = (r[0] if r else ("?", "?"))
            print(f"  {tid:>20}  V15={s15[idx]:+.3f}  V17={s17[idx]:+.3f}  Δ={d_v17_v15[idx]:+.3f}   "
                  f"{a} — {t}")


if __name__ == "__main__":
    main()
