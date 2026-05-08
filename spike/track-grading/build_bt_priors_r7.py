"""Build per-axis BT priors from round-7 per-axis pair JSONs.

For each axis subdirectory under `round7_pairs/<axis_id>/*.json`, run BT-MM
(Hunter 2004) with Bayesian Gamma(2,1) prior to get per-track scores in
[0, 10]. Outputs a single NPZ with all axes for downstream multi-task
training.

Outputs: /home/data01/Music/mesh-track-grading/round7_priors.npz
  axes        : object[K]   (axis ids in column order)
  track_ids   : int64[N]    (only tracks that appeared in any pair)
  scores      : float32[K, N]  (raw BT score, geomean-normalised)
  priors_0_10 : float32[K, N]  (min-max normalised log-score)
  n_games     : int32[K, N]    (per-track participation count per axis)
  win_rate    : float32[K, N]

Usage:
  python build_bt_priors_r7.py
  python build_bt_priors_r7.py --pairs-root /home/data01/Music/mesh-track-grading/round7_pairs
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
    p.add_argument("--pairs-root", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_pairs"))
    p.add_argument("--manifest", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json"))
    p.add_argument("--out", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_priors.npz"))
    p.add_argument("--max-iter", type=int, default=600)
    p.add_argument("--tol", type=float, default=1e-7)
    return p.parse_args()


def bt_mm(wins: dict[tuple[int, int], float],
          tracks: list[int],
          max_iter: int, tol: float,
          prior_strength: float = 1.0) -> dict[int, float]:
    idx = {t: i for i, t in enumerate(tracks)}
    n = len(tracks)
    W = np.zeros((n, n), dtype=np.float64)
    for (a, b), w in wins.items():
        if a in idx and b in idx:
            W[idx[a], idx[b]] += w
    N = W + W.T
    W_row = W.sum(axis=1)
    a_prior = 1.0 + prior_strength
    b_prior = prior_strength
    s = np.ones(n, dtype=np.float64)
    for it in range(max_iter):
        S = s[:, None] + s[None, :]  # n x n
        S[S == 0] = 1.0
        ratio = N / S
        np.fill_diagonal(ratio, 0.0)
        denom = b_prior + ratio.sum(axis=1)
        numer = W_row + (a_prior - 1)
        new_s = np.where(denom > 0, numer / denom, s)
        gm = math.exp(np.log(np.clip(new_s, 1e-12, None)).mean())
        if gm > 0:
            new_s = new_s / gm
        delta = float(np.max(np.abs(new_s - s)))
        s = new_s
        if delta < tol:
            print(f"  [bt] converged at iter {it+1}, delta={delta:.2e}")
            break
    else:
        print(f"  [bt] hit max_iter={max_iter}, last delta={delta:.2e}")
    return {t: float(s[idx[t]]) for t in tracks}


def load_axis_pairs(d: Path) -> list[dict]:
    out = []
    for f in d.glob("*.json"):
        try:
            out.append(json.loads(f.read_text()))
        except Exception as e:
            print(f"skip {f.name}: {e}", file=sys.stderr)
    return out


def aggregate(records: list[dict]) -> tuple[dict[tuple[int, int], float], int, int]:
    wins: dict[tuple[int, int], float] = defaultdict(float)
    n_eq = n_un = 0
    for r in records:
        a = r["presented_a"]; b = r["presented_b"]; ch = r.get("choice")
        if ch == "A":
            wins[(a, b)] += 1.0
        elif ch == "B":
            wins[(b, a)] += 1.0
        elif ch == "EQUAL":
            wins[(a, b)] += 0.5; wins[(b, a)] += 0.5
            n_eq += 1
        else:
            n_un += 1
    return wins, n_eq, n_un


def per_track_stats(wins: dict[tuple[int, int], float],
                    tracks: list[int]) -> tuple[dict[int, float], dict[int, float]]:
    games: dict[int, float] = defaultdict(float)
    won: dict[int, float] = defaultdict(float)
    for (a, b), w in wins.items():
        games[a] += w
        games[b] += w
        won[a] += w
    return games, won


def main() -> int:
    args = parse_args()
    if not args.pairs_root.exists():
        sys.exit(f"missing {args.pairs_root}")

    axis_dirs = sorted([p for p in args.pairs_root.iterdir()
                        if p.is_dir() and not p.name.startswith("_")])
    if not axis_dirs:
        sys.exit(f"no axis subdirs under {args.pairs_root}")
    print(f"[r7-bt] axes: {[d.name for d in axis_dirs]}")

    # First pass: collect all unique track ids across axes.
    all_tracks: set[int] = set()
    per_axis_records: dict[str, list[dict]] = {}
    for d in axis_dirs:
        recs = load_axis_pairs(d)
        per_axis_records[d.name] = recs
        for r in recs:
            all_tracks.add(int(r["presented_a"]))
            all_tracks.add(int(r["presented_b"]))
        print(f"  [{d.name}] {len(recs)} pair records")
    track_ids = sorted(all_tracks)
    n_tracks = len(track_ids)
    K = len(axis_dirs)
    print(f"[r7-bt] {n_tracks} unique tracks across all axes")

    scores_arr = np.zeros((K, n_tracks), dtype=np.float32)
    priors_arr = np.zeros((K, n_tracks), dtype=np.float32)
    games_arr = np.zeros((K, n_tracks), dtype=np.int32)
    winrate_arr = np.zeros((K, n_tracks), dtype=np.float32)
    axis_meta = []

    track_to_idx = {t: i for i, t in enumerate(track_ids)}

    for ki, d in enumerate(axis_dirs):
        aid = d.name
        recs = per_axis_records[aid]
        wins, n_eq, n_un = aggregate(recs)
        seen = sorted(set(t for pair in wins for t in pair))
        print(f"\n[r7-bt] === {aid} ===  pairs={len(recs)} eq={n_eq} unparsed={n_un} "
              f"unique_tracks={len(seen)}")
        scores_local = bt_mm(wins, seen, args.max_iter, args.tol)
        log_scores = {t: math.log(max(scores_local[t], 1e-12)) for t in seen}
        lo = min(log_scores.values()); hi = max(log_scores.values())
        span = hi - lo if hi > lo else 1.0
        priors_local = {t: 10.0 * (log_scores[t] - lo) / span for t in seen}
        games, won = per_track_stats(wins, seen)
        for t in seen:
            i = track_to_idx[t]
            scores_arr[ki, i] = scores_local[t]
            priors_arr[ki, i] = priors_local[t]
            games_arr[ki, i] = int(games[t])
            winrate_arr[ki, i] = won[t] / games[t] if games[t] > 0 else 0.0
        axis_meta.append({"id": aid, "n_records": len(recs),
                          "n_unique_tracks": len(seen),
                          "n_equal": n_eq, "n_unparsed": n_un})
        # quick top/bottom dump
        ordered = sorted(seen, key=lambda t: -priors_local[t])
        print(f"  top 5: " + ", ".join(f"{t}={priors_local[t]:.2f}" for t in ordered[:5]))
        print(f"  bot 5: " + ", ".join(f"{t}={priors_local[t]:.2f}" for t in ordered[-5:]))

    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(args.out,
             axes=np.array([d.name for d in axis_dirs], dtype=object),
             track_ids=np.array(track_ids, dtype=np.int64),
             scores=scores_arr,
             priors_0_10=priors_arr,
             n_games=games_arr,
             win_rate=winrate_arr,
             axis_meta=np.array(axis_meta, dtype=object))
    print(f"\n[r7-bt] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
