"""Direct V15 vs V16 head-to-head: which better matches the user's
round-5 BT-derived intensity ranking?

The round-5 BT priors are the gold standard from the user's mesh-collection
(909 tracks). V15 was trained on these directly (so will overfit). V16 was
trained on a different 1424-track Deezer corpus (zero overlap), so its
performance on the user's 909 tracks is a clean transfer-learning measure.

Outputs Spearman + pairwise-agreement for both axes, and the difference.
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

os.environ["LD_LIBRARY_PATH"] = (
    "/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib:"
    "/nix/store/1xw5xccqqh1xw3mvd70hyil6x418wxcm-gcc-14.3.0-lib/lib:"
    + os.environ.get("LD_LIBRARY_PATH", "")
)

import numpy as np


def spearman_rho(a: np.ndarray, b: np.ndarray) -> float:
    n = len(a)
    if n < 2:
        return 0.0
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    sum_d2 = float(np.sum((ra - rb) ** 2))
    return 1 - 6 * sum_d2 / (n * (n * n - 1))


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


def main():
    from pycozo.client import Client

    # Load user library embeddings.
    db = Client("sqlite", "/home/data01/Music/mesh-collection/mesh.db", {"dataframe": False})
    rows = db.run("?[track_id, vec] := *ml_embeddings{track_id, vec}")["rows"]
    track_ids, embs = [], []
    for tid, vec in rows:
        if vec is not None:
            track_ids.append(int(tid))
            embs.append(vec)
    track_ids = np.array(track_ids, dtype=np.int64)
    embs = np.array(embs, dtype=np.float32)
    print(f"[cmp] user library: {len(track_ids)} tracks")

    # Round-5 BT priors as gold standard.
    priors_path = Path("documents/axis-eval-results/llm-pair-priors-r5.txt")
    if not priors_path.exists():
        sys.exit(f"missing {priors_path}")
    bt = {}
    with priors_path.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split("|", 2)
            if len(parts) == 3:
                bt[int(parts[0])] = float(parts[2])
    print(f"[cmp] round-5 BT priors: {len(bt)} tracks")

    # Align
    mask = np.array([t in bt for t in track_ids])
    track_ids = track_ids[mask]
    embs = embs[mask]
    y = np.array([bt[t] for t in track_ids], dtype=np.float32)
    print(f"[cmp] aligned: {len(track_ids)} tracks")

    # V15
    v15 = json.loads(Path("models/aggression-axes/V15_linear_probe_r6.json").read_text())
    v15_vec = np.array(v15["intensity_axis_vec"], dtype=np.float32)
    v15_score = embs @ v15_vec

    # V16
    v16 = json.loads(Path("models/aggression-axes/V16_round7_blend.json").read_text())
    v16_vec = np.array(v16["intensity_axis_vec"], dtype=np.float32)
    v16_score = embs @ v16_vec

    print()
    print("Axis | Spearman ρ vs round-5 BT | Pairwise Agreement")
    print("-----|-------------------------|--------------------")
    for name, s in [("V15 (round-6 single probe, in-domain)", v15_score),
                    ("V16 (round-7 blend, out-of-domain)", v16_score)]:
        rho = spearman_rho(s, y)
        pa = pair_agreement(s, y)
        print(f"{name}: rho={rho:+.4f}  PA={pa:.4f}")

    rho_v15v16 = spearman_rho(v15_score, v16_score)
    print(f"\nSpearman(V15, V16) on user library: {rho_v15v16:+.4f}")
    print("(closer to +1.0 = same ranking; lower = different orderings)")

    # Difference per track for delta spotcheck
    diffs = v16_score - v15_score
    order_d = np.argsort(np.abs(diffs))[::-1]
    print(f"\nLargest |V16 - V15| disagreement tracks (top 10):")
    print("track_id | V15 | V16 | delta")
    for idx in order_d[:10]:
        print(f"  {int(track_ids[idx])} | {v15_score[idx]:+.3f} | {v16_score[idx]:+.3f} | {diffs[idx]:+.3f}")


if __name__ == "__main__":
    main()
