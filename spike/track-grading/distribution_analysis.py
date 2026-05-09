"""Compare two intensity-axis baselines on distribution shape, spread, and
resolution.

The question this script answers: when two model versions assign different
percentile ranks to the same library, is one of them genuinely *resolving*
finer intensity distinctions, or just shuffling tracks around within an
already-compressed score band?

Stats reported (for each baseline + side-by-side delta):
  - basic moments: mean, std, skew, excess-kurtosis
  - quartiles + p10/p90/p95/p99
  - Shannon entropy of a 50-bin score histogram + effective bucket count
    (perplexity = exp(H)). Higher = wider distribution of scores across bins.
  - density at the modal region: fraction of tracks within ±0.05 of median
  - rank-resolution: median |score_{rank N} - score_{rank N+1}| in the top
    quartile vs middle vs bottom. Small gap = tracks tightly packed, hard to
    rank-discriminate. Large gap = well-spread.
  - Gini-style concentration coefficient on the score distribution (after
    min-max normalization). Higher = more inequality (long-tailed).

Renders a markdown report and writes it to --out.

Usage:
  bash spike/track-grading/run_r7_step.sh distribution_analysis.py \\
       --baseline-a "..../V18.1 Library Baseline.md" \\
       --baseline-b "..../V18.X Library Baseline.md" \\
       --label-a V18.1 --label-b V18.X \\
       --out "..../V18.1 vs V18.X Library Distribution Analysis.md"
"""

from __future__ import annotations

import argparse
import math
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from diff_baselines import parse_baseline, parse_frontmatter  # noqa: E402


def percentile(sorted_asc: list[float], q: float) -> float:
    """Linear-interpolated percentile (q in 0..100), matches numpy default."""
    if not sorted_asc:
        return float("nan")
    if len(sorted_asc) == 1:
        return sorted_asc[0]
    rank = (q / 100.0) * (len(sorted_asc) - 1)
    lo = int(rank)
    hi = min(lo + 1, len(sorted_asc) - 1)
    frac = rank - lo
    return sorted_asc[lo] * (1 - frac) + sorted_asc[hi] * frac


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--baseline-a", type=Path, required=True)
    p.add_argument("--baseline-b", type=Path, required=True)
    p.add_argument("--label-a", default="A")
    p.add_argument("--label-b", default="B")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--bins", type=int, default=50,
                   help="histogram bins for entropy calc (default 50)")
    return p.parse_args()


def shannon_entropy_bits(scores: list[float], n_bins: int) -> tuple[float, int]:
    """Return (entropy_bits, effective_bucket_count = 2^entropy)."""
    if not scores:
        return 0.0, 0
    lo, hi = min(scores), max(scores)
    if lo == hi:
        return 0.0, 1
    width = (hi - lo) / n_bins
    counts = [0] * n_bins
    for s in scores:
        idx = int((s - lo) / width)
        if idx == n_bins:
            idx = n_bins - 1
        counts[idx] += 1
    n = len(scores)
    h = 0.0
    for c in counts:
        if c == 0:
            continue
        p = c / n
        h -= p * math.log2(p)
    return h, int(round(2.0 ** h))


def gini(scores: list[float]) -> float:
    """Gini coefficient on min-max normalized scores. 0 = uniform, 1 = max
    concentration. Used here as a shape descriptor, not an inequality claim."""
    if not scores:
        return 0.0
    lo = min(scores)
    shifted = [s - lo for s in scores]
    hi = max(shifted)
    if hi == 0:
        return 0.0
    norm = sorted(s / hi for s in shifted)
    n = len(norm)
    cum_total = sum(norm)
    if cum_total == 0:
        return 0.0
    cum_partial = 0.0
    weighted = 0.0
    for x in norm:
        cum_partial += x
        weighted += cum_partial
    return (n + 1 - 2 * weighted / cum_total) / n


def adjacent_rank_gap(sorted_desc: list[float], lo: int, hi: int) -> float:
    """Median |score[i] - score[i+1]| within rank slice [lo, hi)."""
    sl = sorted_desc[lo:hi]
    if len(sl) < 2:
        return float("nan")
    gaps = [abs(sl[i] - sl[i + 1]) for i in range(len(sl) - 1)]
    return statistics.median(gaps)


def density_at(scores: list[float], center: float, halfwidth: float) -> float:
    if not scores:
        return 0.0
    hits = sum(1 for s in scores if center - halfwidth <= s <= center + halfwidth)
    return hits / len(scores)


def central_moment(scores: list[float], k: int, mu: float, sd: float) -> float:
    if sd == 0 or not scores:
        return 0.0
    n = len(scores)
    return sum(((s - mu) / sd) ** k for s in scores) / n


def describe(scores: list[float], label: str, n_bins: int) -> dict:
    sorted_asc = sorted(scores)
    sorted_desc = list(reversed(sorted_asc))
    n = len(scores)
    mu = statistics.fmean(scores)
    sd = statistics.pstdev(scores)
    h_bits, eff_buckets = shannon_entropy_bits(scores, n_bins)
    median = percentile(sorted_asc, 50)
    p25 = percentile(sorted_asc, 25)
    p75 = percentile(sorted_asc, 75)
    p10 = percentile(sorted_asc, 10)
    p90 = percentile(sorted_asc, 90)
    return {
        "label": label,
        "n": n,
        "mean": mu,
        "std": sd,
        "skew": central_moment(scores, 3, mu, sd),
        "kurt": central_moment(scores, 4, mu, sd) - 3.0,
        "min": min(scores),
        "p10": p10,
        "p25": p25,
        "median": median,
        "p75": p75,
        "p90": p90,
        "p95": percentile(sorted_asc, 95),
        "p99": percentile(sorted_asc, 99),
        "max": max(scores),
        "iqr": p75 - p25,
        "p90_p10_spread": p90 - p10,
        "entropy_bits": h_bits,
        "effective_buckets": eff_buckets,
        "gini": gini(scores),
        "density_pm05_of_median": density_at(scores, median, 0.05),
        "density_pm10_of_median": density_at(scores, median, 0.10),
        "gap_top_q": adjacent_rank_gap(sorted_desc, 0, n // 4),
        "gap_mid_q": adjacent_rank_gap(sorted_desc, n // 4, 3 * n // 4),
        "gap_bot_q": adjacent_rank_gap(sorted_desc, 3 * n // 4, n),
    }


def fmt(v, decimals=4):
    if isinstance(v, float):
        if math.isnan(v):
            return "—"
        return f"{v:+.{decimals}f}" if abs(v) < 100 and v != int(v) else f"{v:.{decimals}f}"
    return str(v)


def main(args) -> int:
    fa = parse_frontmatter(args.baseline_a)
    fb = parse_frontmatter(args.baseline_b)
    rows_a = parse_baseline(args.baseline_a)
    rows_b = parse_baseline(args.baseline_b)
    scores_a = [r["score"] for r in rows_a.values()]
    scores_b = [r["score"] for r in rows_b.values()]

    print(f"[dist] {args.label_a}: {len(scores_a)} tracks "
          f"(axis={fa.get('axis_variant', '?')})")
    print(f"[dist] {args.label_b}: {len(scores_b)} tracks "
          f"(axis={fb.get('axis_variant', '?')})")

    a = describe(scores_a, args.label_a, args.bins)
    b = describe(scores_b, args.label_b, args.bins)

    body: list[str] = []
    body.append("---")
    body.append("tags: [knowledge-base, mesh, intensity-axis, distribution-analysis]")
    body.append(f"created: 2026-05-09")
    body.append(f"baseline_a: {args.baseline_a.name}")
    body.append(f"baseline_b: {args.baseline_b.name}")
    body.append(f"axis_a: {fa.get('axis_variant', '?')}")
    body.append(f"axis_b: {fb.get('axis_variant', '?')}")
    body.append(f"library: {fa.get('library', '?')}")
    body.append(f"n_a: {len(scores_a)}")
    body.append(f"n_b: {len(scores_b)}")
    body.append("status: distribution snapshot")
    body.append(f"related: [[Mesh — {args.label_a} Library Baseline]], "
                f"[[Mesh — {args.label_b} Library Baseline]], "
                f"[[Mesh — {args.label_a} vs {args.label_b} Library Diff]]")
    body.append("---\n")

    body.append(f"# Distribution analysis — {args.label_a} vs {args.label_b} on the deployed library\n")
    body.append(f"Each baseline's intensity scores are independent of the other (different "
                f"models, same library). This compares their **shape, spread, and resolution** "
                f"to verify which model resolves finer intensity distinctions vs which "
                f"compresses tracks into a narrow band.\n")

    body.append("## Moments + spread\n")
    body.append("| metric | what it measures | " + args.label_a + " | " + args.label_b + " | Δ (B − A) |")
    body.append("|---|---|---:|---:|---:|")
    rows = [
        ("mean",   "centre of mass",                                 a["mean"], b["mean"]),
        ("std",    "overall spread (raw σ)",                         a["std"], b["std"]),
        ("skew",   "asymmetry (>0 = long right tail)",              a["skew"], b["skew"]),
        ("kurt",   "tailedness (>0 = peaky/heavy-tailed)",          a["kurt"], b["kurt"]),
        ("IQR",    "central 50% width (p75 − p25)",                  a["iqr"], b["iqr"]),
        ("p90−p10","central 80% width",                              a["p90_p10_spread"], b["p90_p10_spread"]),
        ("min",    "least intense track",                            a["min"], b["min"]),
        ("max",    "most intense track",                             a["max"], b["max"]),
        ("p10",    "10th percentile score",                          a["p10"], b["p10"]),
        ("median", "50th percentile",                                a["median"], b["median"]),
        ("p90",    "90th percentile score",                          a["p90"], b["p90"]),
        ("p99",    "99th percentile (top 1 %)",                      a["p99"], b["p99"]),
    ]
    for name, what, va, vb in rows:
        body.append(f"| {name} | {what} | `{fmt(va)}` | `{fmt(vb)}` | `{fmt(vb-va)}` |")
    body.append("")

    body.append("## Resolution & concentration\n")
    body.append("These are the metrics that test the *\"V18.X spreads DnB apart, "
                "V18.1 lumps it\"* hypothesis directly.\n")
    body.append("| metric | interpretation | " + args.label_a + " | " + args.label_b + " | Δ |")
    body.append("|---|---|---:|---:|---:|")
    body.append(f"| Shannon entropy ({args.bins}-bin, bits) | wider score distribution → higher entropy | "
                f"`{fmt(a['entropy_bits'])}` | `{fmt(b['entropy_bits'])}` | "
                f"`{fmt(b['entropy_bits']-a['entropy_bits'])}` |")
    body.append(f"| Effective bucket count (2^H) | how many distinct intensity bins the model meaningfully uses | "
                f"`{a['effective_buckets']}` | `{b['effective_buckets']}` | "
                f"`{b['effective_buckets']-a['effective_buckets']:+d}` |")
    body.append(f"| Gini (on min-max scores) | concentration in narrow region (>0 = unequal/long-tailed) | "
                f"`{fmt(a['gini'])}` | `{fmt(b['gini'])}` | "
                f"`{fmt(b['gini']-a['gini'])}` |")
    body.append(f"| % of tracks within ±0.05 of median | density at the modal region (high = lump) | "
                f"`{a['density_pm05_of_median']*100:.1f} %` | `{b['density_pm05_of_median']*100:.1f} %` | "
                f"`{(b['density_pm05_of_median']-a['density_pm05_of_median'])*100:+.1f} pp` |")
    body.append(f"| % of tracks within ±0.10 of median | density at broader modal region | "
                f"`{a['density_pm10_of_median']*100:.1f} %` | `{b['density_pm10_of_median']*100:.1f} %` | "
                f"`{(b['density_pm10_of_median']-a['density_pm10_of_median'])*100:+.1f} pp` |")
    body.append("")

    body.append("## Adjacent-rank score gaps\n")
    body.append("Median absolute score difference between rank *N* and rank *N+1* within "
                "each library quartile. **Larger gap = better rank-resolution** (the model "
                "actually distinguishes consecutive tracks). Smaller gap = tracks are packed "
                "so densely that small score jitter reshuffles ranks.\n")
    body.append("| quartile | tracks | " + args.label_a + " gap | " + args.label_b + " gap | ratio (B/A) |")
    body.append("|---|---|---:|---:|---:|")
    for name, ka, kb in [
        ("Top 25 % (most intense)", "gap_top_q", "gap_top_q"),
        ("Middle 50 %",             "gap_mid_q", "gap_mid_q"),
        ("Bottom 25 %",             "gap_bot_q", "gap_bot_q"),
    ]:
        ga, gb = a[ka], b[kb]
        ratio = (gb / ga) if ga > 0 else float("nan")
        body.append(f"| {name} | ~{len(scores_a)//4} | `{fmt(ga)}` | `{fmt(gb)}` | `{fmt(ratio, 2)}` |")
    body.append("")

    body.append("## Verdict (mechanical)\n")
    verdict_lines = []
    if b["std"] > a["std"]:
        verdict_lines.append(f"- **{args.label_b} has wider overall spread** "
                             f"(σ {fmt(a['std'])} → {fmt(b['std'])}, +{(b['std']-a['std'])/a['std']*100:.1f} %).")
    else:
        verdict_lines.append(f"- **{args.label_a} has wider overall spread** "
                             f"(σ {fmt(a['std'])} → {fmt(b['std'])}).")
    if b["entropy_bits"] > a["entropy_bits"]:
        verdict_lines.append(f"- **{args.label_b} uses more distinct intensity bins** "
                             f"({a['effective_buckets']} → {b['effective_buckets']} effective buckets, "
                             f"+{b['entropy_bits']-a['entropy_bits']:.3f} bits).")
    else:
        verdict_lines.append(f"- **{args.label_a} uses more distinct intensity bins** "
                             f"({a['effective_buckets']} → {b['effective_buckets']} effective buckets).")
    d05_a = a["density_pm05_of_median"] * 100
    d05_b = b["density_pm05_of_median"] * 100
    if d05_b < d05_a:
        verdict_lines.append(f"- **{args.label_b} is less concentrated at the median** "
                             f"({d05_a:.1f} % → {d05_b:.1f} % within ±0.05 of median, "
                             f"{d05_a - d05_b:+.1f} pp less lumpy).")
    else:
        verdict_lines.append(f"- **{args.label_b} is more concentrated at the median** "
                             f"({d05_a:.1f} % → {d05_b:.1f} % within ±0.05).")
    top_ratio = b["gap_top_q"] / a["gap_top_q"] if a["gap_top_q"] > 0 else float("nan")
    if top_ratio > 1.05:
        verdict_lines.append(f"- **Top-quartile rank-resolution improved {top_ratio:.2f}×** in {args.label_b} "
                             f"— the model distinguishes adjacent high-intensity tracks better.")
    elif top_ratio < 0.95:
        verdict_lines.append(f"- **Top-quartile rank-resolution worsened {1/top_ratio:.2f}×** in {args.label_b} "
                             f"— the model packs adjacent high-intensity tracks more tightly.")
    else:
        verdict_lines.append(f"- Top-quartile rank-resolution ~unchanged ({top_ratio:.2f}×).")
    body.extend(verdict_lines)
    body.append("")
    body.append(f"_Read these together: if `{args.label_b}` shows wider σ + higher entropy + "
                f"lower density-at-median + larger top-quartile gap, the \"spreads DnB apart\" "
                f"claim is supported. If only some hold, the picture is mixed._\n")

    args.out.write_text("\n".join(body) + "\n")
    print(f"[dist] wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(parse_args()))
