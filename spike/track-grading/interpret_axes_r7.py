"""Round-7 axis interpretation: top/bottom 20 tracks per learned axis.

Reads:
  /tmp/track-grading/round7_axes.npz          (axes, directions[K, D])
  /tmp/track-grading/embeddings/corpus_muq_mulan.npz  (track_ids, embeddings)
  /tmp/track-grading/deezer/corpus_tracks.json (artist+title+seed lookup)

Writes:
  /tmp/track-grading/round7_interpretation.md

For each axis: project all corpus tracks onto the learned direction, sort,
print top-20 / bottom-20 with artist+title+genre_seed. Also compute the
inter-axis correlation matrix; flags axis pairs with |corr| > 0.85 as
redundant.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--axes-file", type=Path,
                   default=Path("/tmp/track-grading/round7_axes.npz"))
    p.add_argument("--embeddings", type=Path,
                   default=Path("/tmp/track-grading/embeddings/corpus_muq_mulan.npz"))
    p.add_argument("--manifest", type=Path,
                   default=Path("/tmp/track-grading/deezer/corpus_tracks.json"))
    p.add_argument("--out", type=Path,
                   default=Path("/tmp/track-grading/round7_interpretation.md"))
    p.add_argument("--top-n", type=int, default=20)
    return p.parse_args()


def main() -> int:
    args = parse_args()
    axes_npz = np.load(args.axes_file, allow_pickle=True)
    emb = np.load(args.embeddings, allow_pickle=True)
    manifest = json.loads(args.manifest.read_text())

    axes = list(axes_npz["axes"])
    directions = axes_npz["directions"].astype(np.float32)
    biases = axes_npz["biases"].astype(np.float32)
    K, D = directions.shape
    print(f"[r7-interp] {K} axes, D={D}")

    # Manifest lookup
    meta = {}
    for r in manifest:
        tid = r.get("deezer_track_id")
        if tid is None:
            continue
        meta[int(tid)] = {
            "artist": r.get("artist", ""),
            "title": r.get("title", ""),
            "genre_seed": r.get("genre_seed", r.get("category", "")),
        }

    e_ids = emb["track_ids"].astype(np.int64)
    e_X = emb["embeddings"].astype(np.float32)
    print(f"[r7-interp] projecting {len(e_ids)} tracks...")
    scores = e_X @ directions.T + biases   # [N, K]

    lines: list[str] = []
    lines.append("# Round-7 axis interpretation\n")
    lines.append(f"Corpus: {len(e_ids)} tracks, {K} learned axes.\n")

    # Inter-axis correlation matrix
    s_z = (scores - scores.mean(axis=0, keepdims=True)) / (scores.std(axis=0, keepdims=True) + 1e-6)
    corr = (s_z.T @ s_z) / len(e_ids)
    lines.append("\n## Inter-axis Pearson correlation\n")
    lines.append("| | " + " | ".join(axes) + " |")
    lines.append("|---|" + "---|" * K)
    redundant_pairs = []
    for i, ai in enumerate(axes):
        row = "| " + ai + " | " + " | ".join(f"{corr[i, j]:+.2f}" for j in range(K)) + " |"
        lines.append(row)
        for j in range(i + 1, K):
            if abs(corr[i, j]) >= 0.85:
                redundant_pairs.append((ai, axes[j], float(corr[i, j])))
    if redundant_pairs:
        lines.append("\n**Redundant pairs (|corr| ≥ 0.85):**\n")
        for a, b, c in redundant_pairs:
            lines.append(f"- `{a}` ↔ `{b}`: {c:+.3f}")
    else:
        lines.append("\nNo redundant pairs (|corr| < 0.85 between all axes).\n")

    # Per-axis top/bottom
    for k, axis_id in enumerate(axes):
        lines.append(f"\n## {axis_id}\n")
        s = scores[:, k]
        order = np.argsort(-s)
        lines.append(f"Score range: [{s.min():.3f}, {s.max():.3f}]  "
                     f"std={s.std():.3f}\n")
        lines.append(f"\n### Top {args.top_n}\n")
        lines.append("| rank | track_id | score | artist — title (seed) |")
        lines.append("|---:|---:|---:|---|")
        for rk, idx in enumerate(order[:args.top_n]):
            tid = int(e_ids[idx])
            m = meta.get(tid, {"artist": "?", "title": "?", "genre_seed": "?"})
            lines.append(f"| {rk+1} | {tid} | {s[idx]:.3f} | "
                         f"{m['artist']} — {m['title']} _({m['genre_seed']})_ |")
        lines.append(f"\n### Bottom {args.top_n}\n")
        lines.append("| rank | track_id | score | artist — title (seed) |")
        lines.append("|---:|---:|---:|---|")
        for rk, idx in enumerate(order[-args.top_n:][::-1]):
            tid = int(e_ids[idx])
            m = meta.get(tid, {"artist": "?", "title": "?", "genre_seed": "?"})
            lines.append(f"| {rk+1} | {tid} | {s[idx]:.3f} | "
                         f"{m['artist']} — {m['title']} _({m['genre_seed']})_ |")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text("\n".join(lines) + "\n")
    print(f"[r7-interp] wrote {args.out}")

    if redundant_pairs:
        print(f"[r7-interp] {len(redundant_pairs)} redundant axis pairs:")
        for a, b, c in redundant_pairs:
            print(f"  {a} <-> {b}: {c:+.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
