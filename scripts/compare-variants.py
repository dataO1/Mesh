#!/usr/bin/env python3
"""Score every intensity-axis variant against a manual prior file.

Inputs:
  - documents/axis-eval-results/V*.csv  (one ranked CSV per variant)
  - anchors file (path passed as $1): each line "track_id|name|prior"
    where prior ∈ [0.0, 10.0]

For each variant:
  - compute Spearman rank correlation between prior and variant rank
  - compute "band hits" — fraction of anchors landing in expected percentile band
  - print sorted leaderboard
  - flag the worst miss per variant

Usage: python compare-variants.py /tmp/anchors50.txt
"""
import csv
import sys
from pathlib import Path
from collections import defaultdict


def main():
    anchors_path = Path(sys.argv[1] if len(sys.argv) > 1 else '/tmp/anchors50.txt')
    eval_dir = Path('documents/axis-eval-results')
    if not anchors_path.exists():
        print(f"missing {anchors_path}", file=sys.stderr); sys.exit(1)

    # Parse anchors
    anchors = []
    with open(anchors_path) as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#'): continue
            tid, name, prior = line.split('|', 2)
            anchors.append((tid, name, float(prior)))

    # Expected rank from priors (descending)
    sorted_anchors = sorted(anchors, key=lambda x: -x[2])
    expected_rank = {tid: i+1 for i, (tid, _, _) in enumerate(sorted_anchors)}

    # Discover all variant CSVs
    variants = sorted(p.stem for p in eval_dir.glob('V*.csv'))
    if not variants:
        print(f"no variants in {eval_dir} — run scripts/eval-axis-variants.sh first", file=sys.stderr)
        sys.exit(1)

    # Need total count from first CSV
    with open(eval_dir / f'{variants[0]}.csv') as f:
        TOTAL = sum(1 for _ in f) - 1
    print(f"Loaded {len(anchors)} anchors, evaluating against {len(variants)} variants on {TOTAL}-track library\n")

    results = []
    for V in variants:
        var_rank = {}
        with open(eval_dir / f'{V}.csv') as f:
            for row in csv.DictReader(f):
                if row['track_id'] in expected_rank:
                    var_rank[row['track_id']] = int(row['rank'])
        if len(var_rank) < len(anchors):
            print(f"  warn: {V} missing {len(anchors)-len(var_rank)} anchor track(s)")

        # Spearman within the anchor sub-set
        sorted_by_var = sorted(var_rank.items(), key=lambda kv: kv[1])
        var_inner_rank = {tid: i+1 for i, (tid, _) in enumerate(sorted_by_var)}
        n = len(var_rank)
        sum_d2 = sum((expected_rank[tid] - var_inner_rank[tid])**2 for tid in var_rank)
        spearman = 1 - 6*sum_d2 / (n*(n*n - 1)) if n > 1 else 0.0

        # Band hits + worst miss
        band_hits = 0
        worst_miss = (0, '', 0)  # (abs_dev, name, rank)
        for tid, name, prior in anchors:
            if tid not in var_rank: continue
            actual_pct = var_rank[tid] / TOTAL
            if   prior >= 9:    expected_pct = 0.05
            elif prior >= 8:    expected_pct = 0.15
            elif prior >= 7:    expected_pct = 0.30
            elif prior >= 6:    expected_pct = 0.50
            elif prior >= 5:    expected_pct = 0.65
            elif prior >= 4:    expected_pct = 0.80
            elif prior >= 3:    expected_pct = 0.93
            else:               expected_pct = 0.97
            tolerance = 0.20
            if abs(actual_pct - expected_pct) <= tolerance: band_hits += 1
            dev = abs(actual_pct - expected_pct)
            if dev > worst_miss[0]:
                worst_miss = (dev, name, var_rank[tid])

        results.append((V, spearman, band_hits, len(anchors), worst_miss))

    # Sort by Spearman descending
    results.sort(key=lambda x: -x[1])

    print(f"{'rank':>4s}  {'variant':28s}  {'spearman':>9s}  {'band hits':>9s}  worst miss (rank)")
    print('-' * 110)
    for i, (V, s, h, n, miss) in enumerate(results):
        print(f"{i+1:>4d}  {V:28s}  {s:+.4f}    {h:>3d}/{n:<3d}    {miss[1][:55]} (#{miss[2]})")

    # Highlight V6/V7 lineage
    print()
    print("V6/V7 lineage comparison (the user's preferred starting point):")
    for V, s, h, n, _ in results:
        if V.startswith(('V6', 'V7', 'V8', 'V9', 'V10', 'V11', 'V12', 'V13')):
            print(f"  {V:28s}  ρ={s:+.4f}  bands={h}/{n}")

if __name__ == '__main__':
    main()
