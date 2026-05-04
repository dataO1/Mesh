"""Convert per-pair JSON judgments → BT-MLE intensity scores → priors file.

Inputs:  /tmp/track-grading/pairs_vllm/*.json  (one per directed pair)
Outputs: documents/axis-eval-results/llm-pair-priors.txt   (anchors format
         tid|name|prior, prior ∈ [0, 10])
         documents/axis-eval-results/llm-pair-priors.csv   (full table:
         track_id, title, artist, bt_score, prior_0_10, n_pairs, win_rate)

Bradley-Terry MLE via the Minorization-Maximization (MM) iteration of
Hunter (2004). EQUAL outcomes counted as 0.5 win for each side.
Bilateral pair (A→B and B→A) presentations both contribute, which cancels
positional bias to first order.

After convergence, log-scores are min-max normalised to [0, 10] to feed
scripts/compare-variants.py the same way hand-priors do.

Usage:
  python build_bt_priors.py
  python build_bt_priors.py --pairs-dir /tmp/track-grading/pairs_vllm \\
                            --meta /tmp/track-grading/_track-list.csv \\
                            --out-prefix documents/axis-eval-results/llm-pair-priors
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from collections import defaultdict
from pathlib import Path


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--pairs-dir", type=Path,
                   default=Path("/tmp/track-grading/pairs_vllm"))
    p.add_argument("--meta", type=Path,
                   default=Path("/tmp/track-grading/_track-list.csv"))
    p.add_argument("--out-prefix", type=Path,
                   default=Path("documents/axis-eval-results/llm-pair-priors"))
    p.add_argument("--max-iter", type=int, default=500)
    p.add_argument("--tol", type=float, default=1e-7)
    return p.parse_args()


def load_meta(p: Path) -> dict[int, dict]:
    out = {}
    with p.open() as f:
        for r in csv.DictReader(f):
            tid = int(r["track_id"])
            out[tid] = {"title": r["title"], "artist": r["artist"]}
    return out


def load_pairs(d: Path) -> list[dict]:
    out = []
    for f in sorted(d.glob("*.json")):
        try:
            out.append(json.loads(f.read_text()))
        except Exception as e:
            print(f"skip {f.name}: {e}", file=sys.stderr)
    return out


def bt_mm(wins: dict[tuple[int, int], float],
          tracks: list[int],
          max_iter: int, tol: float,
          prior_strength: float = 1.0) -> dict[int, float]:
    """Bayesian Hunter MM iteration for BT scores.

    Adds a Gamma(a=1+prior_strength, b=prior_strength) prior on each score
    to handle saturated tracks (always-won / always-lost) which would
    otherwise diverge to inf/0. With anchored sampling 282/909 tracks beat
    every opponent — without smoothing they pin at 10/10 and lose ranking
    information. The prior pulls undefeated tracks toward a finite score
    proportional to their margin over peers.

    Reduces to plain MLE when prior_strength=0.
    """
    idx = {t: i for i, t in enumerate(tracks)}
    n = len(tracks)
    W = [[0.0] * n for _ in range(n)]
    for (a, b), w_ab in wins.items():
        if a in idx and b in idx:
            W[idx[a]][idx[b]] += w_ab
    N = [[W[i][j] + W[j][i] for j in range(n)] for i in range(n)]
    W_row = [sum(W[i]) for i in range(n)]
    a_prior = 1.0 + prior_strength  # Gamma shape
    b_prior = prior_strength        # Gamma rate
    s = [1.0] * n
    for it in range(max_iter):
        new_s = [0.0] * n
        for i in range(n):
            denom = b_prior
            for j in range(n):
                if i == j or N[i][j] == 0: continue
                denom += N[i][j] / (s[i] + s[j])
            numer = W_row[i] + a_prior - 1
            new_s[i] = numer / denom if denom > 0 else s[i]
        gm = math.exp(sum(math.log(v) for v in new_s if v > 0) / n)
        if gm > 0:
            new_s = [v / gm for v in new_s]
        delta = max(abs(new_s[i] - s[i]) for i in range(n))
        s = new_s
        if delta < tol:
            print(f"[bt] converged at iter {it+1}, delta={delta:.2e}")
            break
    else:
        print(f"[bt] hit max_iter={max_iter}, last delta={delta:.2e}")
    return {t: s[idx[t]] for t in tracks}


def main() -> int:
    args = parse_args()
    if not args.meta.exists():
        sys.exit(f"missing {args.meta}")
    if not args.pairs_dir.exists():
        sys.exit(f"missing {args.pairs_dir}")

    meta = load_meta(args.meta)
    records = load_pairs(args.pairs_dir)
    print(f"[bt] loaded {len(records)} pair records, {len(meta)} tracks in meta")

    # Aggregate wins per (winner, loser); EQUAL = 0.5/0.5
    wins: dict[tuple[int, int], float] = defaultdict(float)
    n_equal = n_unparsed = 0
    seen: set[int] = set()
    for r in records:
        a = r["presented_a"]; b = r["presented_b"]; ch = r.get("choice")
        seen.add(a); seen.add(b)
        if ch == "A":
            wins[(a, b)] += 1.0
        elif ch == "B":
            wins[(b, a)] += 1.0
        elif ch == "EQUAL":
            wins[(a, b)] += 0.5; wins[(b, a)] += 0.5
            n_equal += 1
        else:
            n_unparsed += 1
    print(f"[bt] {n_equal} EQUAL, {n_unparsed} unparsed, "
          f"{len(seen)} unique tracks in pairs")

    tracks = sorted(seen)
    scores = bt_mm(wins, tracks, args.max_iter, args.tol)

    # Convert to 0-10 priors via log-score quantile
    log_scores = {t: math.log(max(scores[t], 1e-12)) for t in tracks}
    lo = min(log_scores.values()); hi = max(log_scores.values())
    span = hi - lo if hi > lo else 1.0
    priors = {t: 10.0 * (log_scores[t] - lo) / span for t in tracks}

    # Pair counts and win rates
    pair_count: dict[int, int] = defaultdict(int)
    win_count: dict[int, float] = defaultdict(float)
    for (a, b), w in wins.items():
        pair_count[a] += int(w + (wins.get((b, a), 0) > 0))
        win_count[a] += w
    # cleaner: total games per track = sum of (w_ab + w_ba) across opponents
    games: dict[int, float] = defaultdict(float)
    won: dict[int, float] = defaultdict(float)
    for (a, b), w in wins.items():
        games[a] += w; games[b] += w
        won[a] += w
    win_rate = {t: (won[t] / games[t]) if games[t] > 0 else 0.0 for t in tracks}

    # Write outputs
    args.out_prefix.parent.mkdir(parents=True, exist_ok=True)
    txt_path = args.out_prefix.with_suffix(".txt")
    csv_path = args.out_prefix.with_suffix(".csv")

    sorted_tracks = sorted(tracks, key=lambda t: -priors[t])
    with txt_path.open("w") as f:
        f.write("# track_id|title|prior_0_10  (BT-MLE from pair judgments)\n")
        for t in sorted_tracks:
            title = meta.get(t, {}).get("title", f"track_{t}").replace("|", "/")
            f.write(f"{t}|{title}|{priors[t]:.3f}\n")

    with csv_path.open("w") as f:
        w = csv.writer(f)
        w.writerow(["track_id", "title", "artist", "bt_score", "prior_0_10",
                    "n_games", "win_rate"])
        for t in sorted_tracks:
            m = meta.get(t, {})
            w.writerow([t, m.get("title", ""), m.get("artist", ""),
                        f"{scores[t]:.6f}", f"{priors[t]:.3f}",
                        int(games[t]), f"{win_rate[t]:.3f}"])

    print(f"[bt] wrote {txt_path}")
    print(f"[bt] wrote {csv_path}")
    print(f"[bt] top 5: " + ", ".join(
        f"{meta.get(t, {}).get('title', t)[:20]}={priors[t]:.1f}"
        for t in sorted_tracks[:5]))
    print(f"[bt] bottom 5: " + ", ".join(
        f"{meta.get(t, {}).get('title', t)[:20]}={priors[t]:.1f}"
        for t in sorted_tracks[-5:]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
