"""Validate BT-derived priors against hand-anchors.

Computes Spearman ρ between BT-derived priors (from build_bt_priors.py)
and the round-2 hand-anchor file. If the LLM pairwise judgments are real
signal, Spearman should be >> 0 (round-3 LLM-priors hit ~+0.20-0.27
against hand-anchors as a baseline; pair-derived should beat that).

Also prints per-anchor diffs to flag systematic disagreements.

Usage:
  python validate_bt_priors.py
  python validate_bt_priors.py --bt documents/axis-eval-results/llm-pair-priors.txt \
                                --hand /tmp/anchors50.txt
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--bt", type=Path,
                   default=Path("documents/axis-eval-results/llm-pair-priors.txt"))
    p.add_argument("--hand", type=Path, default=Path("/tmp/anchors50.txt"))
    return p.parse_args()


def load_priors(p: Path) -> dict[int, tuple[str, float]]:
    out = {}
    with p.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"): continue
            parts = line.split("|", 2)
            if len(parts) != 3: continue
            tid = int(parts[0])
            out[tid] = (parts[1], float(parts[2]))
    return out


def spearman(xs: list[float], ys: list[float]) -> float:
    n = len(xs)
    if n < 2: return 0.0
    rx = sorted(range(n), key=lambda i: xs[i])
    ry = sorted(range(n), key=lambda i: ys[i])
    rank_x = [0] * n; rank_y = [0] * n
    for r, i in enumerate(rx): rank_x[i] = r + 1
    for r, i in enumerate(ry): rank_y[i] = r + 1
    sum_d2 = sum((rank_x[i] - rank_y[i]) ** 2 for i in range(n))
    return 1 - 6 * sum_d2 / (n * (n * n - 1))


def main() -> int:
    args = parse_args()
    if not args.bt.exists():
        sys.exit(f"missing {args.bt} — run build_bt_priors.py first")
    if not args.hand.exists():
        sys.exit(f"missing {args.hand}")

    bt = load_priors(args.bt)
    hand = load_priors(args.hand)
    overlap = sorted(set(bt) & set(hand))
    print(f"[validate] bt={len(bt)} hand={len(hand)} overlap={len(overlap)}")
    if len(overlap) < 5:
        sys.exit("not enough overlap to compute meaningful correlation")

    bt_v = [bt[t][1] for t in overlap]
    hand_v = [hand[t][1] for t in overlap]
    rho = spearman(bt_v, hand_v)
    print(f"[validate] Spearman ρ (BT vs hand) = {rho:+.4f}")
    print(f"[validate]   round-2 hand-vs-V11   = +0.358 (best variant baseline)")
    print(f"[validate]   round-3 LLM-vs-V11    = +0.190 (LLM-as-absolute baseline)")

    # Per-anchor disagreement table (top 10 worst)
    diffs = sorted(((abs(bt[t][1] - hand[t][1]), t,
                     bt[t][0], bt[t][1], hand[t][1]) for t in overlap),
                   reverse=True)
    print(f"\n[validate] top-10 disagreements (|bt - hand|):")
    print(f"  {'tid':>20s}  {'title':40s}  {'bt':>6s}  {'hand':>6s}")
    for d, t, name, bv, hv in diffs[:10]:
        print(f"  {t:>20d}  {name[:40]:40s}  {bv:>6.2f}  {hv:>6.2f}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
