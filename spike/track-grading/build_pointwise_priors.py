"""Stack pointwise (track, axis) score JSONs into the prior matrix used
by `train_axes_r7_5.py`.

Output schema mirrors the old BT priors NPZ so the existing trainer
plugs in unchanged:

    track_ids: int64[N]
    axis_ids:  list[str] of length A
    scores:    float32[N, A]   # was 'priors' in BT case; renamed for clarity
    coverage:  bool[N, A]      # True if a score is present, False if NaN
                                # (linear probe masks loss on missing cells)

Tracks with zero coverage on any axis are dropped. The trainer reads
'scores' under the key 'priors' for backward compat; we write both.

Usage:
    bash spike/track-grading/run_r7_step.sh build_pointwise_priors.py \
         --pairs-root /home/data01/Music/mesh-track-grading/round7_6_pointwise/music_flamingo \
         --out /home/data01/Music/mesh-track-grading/round7_6_priors.npz
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--pairs-root", type=Path, required=True,
                   help="dir containing <axis>/<track_id>.json subtrees "
                        "(name kept as 'pairs-root' for symmetry with "
                        "build_bt_priors_r7_5)")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--prompts-file", type=Path,
                   default=Path("spike/track-grading/round7_6_pointwise_prompts.json"))
    return p.parse_args()


def main(args) -> int:
    cfg = json.loads(args.prompts_file.read_text())
    axis_ids = [a["id"] for a in cfg["axes"]]
    A = len(axis_ids)
    axis_idx = {aid: i for i, aid in enumerate(axis_ids)}
    print(f"[stack] {A} axes")

    # Walk all per-cell JSONs, collect by track
    raw: dict[int, dict[str, float]] = defaultdict(dict)
    parse_fails = 0
    n_files = 0
    for axis_dir in sorted(args.pairs_root.iterdir()):
        if not axis_dir.is_dir():
            continue
        if axis_dir.name not in axis_idx:
            print(f"[stack] WARNING: ignoring unknown axis dir {axis_dir.name}",
                  file=sys.stderr)
            continue
        for f in axis_dir.glob("*.json"):
            n_files += 1
            try:
                rec = json.loads(f.read_text())
            except Exception:
                continue
            tid = int(rec["track_id"])
            if rec.get("error") == "parse_fail":
                parse_fails += 1
                continue
            score = rec.get("score")
            if score is None or (isinstance(score, float) and math.isnan(score)):
                parse_fails += 1
                continue
            raw[tid][axis_dir.name] = float(score)
    print(f"[stack] read {n_files} per-cell JSONs from {args.pairs_root}")
    print(f"[stack] {parse_fails} cells had parse failures and were skipped")
    print(f"[stack] {len(raw)} tracks have at least one axis score")

    # Build matrices
    track_ids = np.array(sorted(raw.keys()), dtype=np.int64)
    N = len(track_ids)
    scores = np.full((N, A), np.nan, dtype=np.float32)
    coverage = np.zeros((N, A), dtype=bool)
    for i, tid in enumerate(track_ids):
        for aid, sc in raw[tid].items():
            j = axis_idx[aid]
            scores[i, j] = sc
            coverage[i, j] = True

    cov_per_axis = coverage.mean(axis=0)
    print(f"[stack] coverage per axis (% tracks with score):")
    for aid, c in zip(axis_ids, cov_per_axis):
        print(f"   {aid:25s}  {100*c:5.1f}%")
    full_rows = coverage.all(axis=1).sum()
    print(f"[stack] {full_rows}/{N} tracks have full 16-axis coverage")

    # Score distribution sanity per axis (helps catch a model that collapses
    # to a single attractor like '25 for everything')
    print(f"[stack] per-axis score distribution (across covered tracks):")
    print(f"   {'axis':25s}  {'n':>6s}  {'mean':>6s}  {'std':>6s}  "
          f"{'p10':>5s}  {'p50':>5s}  {'p90':>5s}  {'unique':>7s}")
    for aid in axis_ids:
        j = axis_idx[aid]
        col = scores[:, j]
        col = col[~np.isnan(col)]
        if len(col) == 0:
            print(f"   {aid:25s}    -    (no data)")
            continue
        print(f"   {aid:25s}  {len(col):>6d}  {col.mean():>6.1f}  "
              f"{col.std():>6.1f}  {np.percentile(col,10):>5.1f}  "
              f"{np.percentile(col,50):>5.1f}  {np.percentile(col,90):>5.1f}  "
              f"{len(np.unique(col)):>7d}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    # Store under both 'scores' (new pointwise schema) and 'priors' (back-compat
    # so train_axes_r7_5 reads it without changes).
    np.savez(
        args.out,
        track_ids=track_ids,
        axis_ids=np.array(axis_ids, dtype=object),
        scores=scores,
        priors=scores,  # alias for backward-compat with BT-era trainer
        coverage=coverage,
    )
    print(f"[stack] wrote {args.out} ({args.out.stat().st_size/1e6:.1f} MB)")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
