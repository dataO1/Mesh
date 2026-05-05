"""Round-7 cross-library transfer test.

Project the user's mesh-collection (~909 tracks with MuQ-MuLan embeddings
already stored in mesh.db) onto each of the 12 learned axes + the blended
intensity axis, then dump top/bottom per axis with track titles + artists.

Compares blend-axis ranking on user library to V15 ranking (the currently-
deployed axis) using Spearman.

Usage:
  bash run_r7_step.sh cross_library_r7.py
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# zlib + libstdc++ for cozo embedded
os.environ.setdefault("LD_LIBRARY_PATH", "")
ld = os.environ["LD_LIBRARY_PATH"]
extras = [
    "/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib",
    "/nix/store/1xw5xccqqh1xw3mvd70hyil6x418wxcm-gcc-14.3.0-lib/lib",
]
for e in extras:
    if e not in ld:
        ld = (e + ":" + ld) if ld else e
os.environ["LD_LIBRARY_PATH"] = ld

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--db-path",
                   default="/home/data01/Music/mesh-collection/mesh.db")
    p.add_argument("--axes-file", type=Path,
                   default=Path("/tmp/track-grading/round7_axes.npz"))
    p.add_argument("--blend-file", type=Path,
                   default=Path("/tmp/track-grading/round7_blend.npz"))
    p.add_argument("--v15-axis", type=Path,
                   default=Path("models/aggression-axes/V15_linear_probe_r6.json"))
    p.add_argument("--out", type=Path,
                   default=Path("/tmp/track-grading/round7_cross_library.md"))
    p.add_argument("--top-n", type=int, default=15)
    return p.parse_args()


def spearman_rho(a: np.ndarray, b: np.ndarray) -> float:
    n = len(a)
    if n < 2:
        return 0.0
    ra = np.argsort(np.argsort(a))
    rb = np.argsort(np.argsort(b))
    sum_d2 = float(np.sum((ra - rb) ** 2))
    return 1 - 6 * sum_d2 / (n * (n * n - 1))


def load_user_embeddings(db_path: str):
    from pycozo.client import Client
    db = Client("sqlite", db_path, {"dataframe": False})
    rows = db.run("?[track_id, vec] := *ml_embeddings{track_id, vec}")["rows"]
    track_ids = []
    embs = []
    for tid, vec in rows:
        if vec is None:
            continue
        track_ids.append(int(tid))
        embs.append(vec)
    track_ids = np.array(track_ids, dtype=np.int64)
    embs = np.array(embs, dtype=np.float32)

    # Track meta — id is the PK
    rows2 = db.run("?[id, title, artist] := *tracks{id, title, artist}")["rows"]
    meta = {}
    for tid, title, artist in rows2:
        meta[int(tid)] = (title or "", artist or "")
    return track_ids, embs, meta


def main() -> int:
    args = parse_args()
    if not args.axes_file.exists() or not args.blend_file.exists():
        sys.exit(f"missing {args.axes_file} or {args.blend_file}")

    track_ids, embs, meta = load_user_embeddings(args.db_path)
    print(f"[xlib] user library: {len(track_ids)} embeddings, "
          f"{embs.shape[1]}-d")

    axes_npz = np.load(args.axes_file, allow_pickle=True)
    blend = np.load(args.blend_file, allow_pickle=True)
    axes = list(axes_npz["axes"])
    directions = axes_npz["directions"].astype(np.float32)
    biases = axes_npz["biases"].astype(np.float32)

    # Per-axis projection
    axis_scores = embs @ directions.T + biases  # [N, K]

    # Blended (use the same axis_means/stds from training to z-score, then
    # weighted sum) so we match the same path as deployment.
    means = blend["axis_means"].astype(np.float32)  # [1, K]
    stds = blend["axis_stds"].astype(np.float32)    # [1, K]
    weights = blend["weights"].astype(np.float32)
    z = (axis_scores - means) / stds
    blend_score = z @ weights  # [N]

    # Also compute V15 score for comparison
    v15 = json.loads(args.v15_axis.read_text())
    v15_vec = np.array(v15["intensity_axis_vec"], dtype=np.float32)
    v15_score = embs @ v15_vec  # [N]

    rho = spearman_rho(blend_score, v15_score)
    print(f"[xlib] blend vs V15 Spearman on user library: {rho:+.4f}")

    lines = []
    lines.append("# Round-7 cross-library transfer test\n")
    lines.append(f"User library: **{len(track_ids)}** tracks with MuQ-MuLan embeddings.\n")
    lines.append(f"Spearman(blend_score, V15_score) = **{rho:+.4f}**  "
                 "(closer to +1 = blend reproduces V15's ranking; "
                 "lower = blend re-orders)\n")

    # Top/bottom on blend
    order = np.argsort(-blend_score)
    lines.append(f"\n## Blended intensity (target=aggression) — top {args.top_n}\n")
    lines.append("| rank | score | title — artist |")
    lines.append("|---:|---:|---|")
    for r, idx in enumerate(order[:args.top_n]):
        tid = int(track_ids[idx])
        title, artist = meta.get(tid, ("?", "?"))
        lines.append(f"| {r+1} | {blend_score[idx]:+.3f} | {title} — {artist} |")
    lines.append(f"\n## Blended intensity — bottom {args.top_n}\n")
    lines.append("| rank | score | title — artist |")
    lines.append("|---:|---:|---|")
    for r, idx in enumerate(order[-args.top_n:][::-1]):
        tid = int(track_ids[idx])
        title, artist = meta.get(tid, ("?", "?"))
        lines.append(f"| {r+1} | {blend_score[idx]:+.3f} | {title} — {artist} |")

    # Per-axis top/bottom
    for k, name in enumerate(axes):
        s = axis_scores[:, k]
        order = np.argsort(-s)
        lines.append(f"\n## Axis: {name} — top {args.top_n}\n")
        lines.append("| rank | score | title — artist |")
        lines.append("|---:|---:|---|")
        for r, idx in enumerate(order[:args.top_n]):
            tid = int(track_ids[idx])
            title, artist = meta.get(tid, ("?", "?"))
            lines.append(f"| {r+1} | {s[idx]:+.3f} | {title} — {artist} |")
        lines.append(f"\n## Axis: {name} — bottom {args.top_n}\n")
        lines.append("| rank | score | title — artist |")
        lines.append("|---:|---:|---|")
        for r, idx in enumerate(order[-args.top_n:][::-1]):
            tid = int(track_ids[idx])
            title, artist = meta.get(tid, ("?", "?"))
            lines.append(f"| {r+1} | {s[idx]:+.3f} | {title} — {artist} |")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text("\n".join(lines) + "\n")
    print(f"[xlib] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
