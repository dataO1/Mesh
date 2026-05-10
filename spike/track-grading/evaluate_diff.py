"""For tracks that shifted significantly between V18.1 and V18.X, decide
per-track which model's ranking is closer to the consensus ground-truth.

Reads the same two baseline tables as `diff_baselines.py`, plus uses the
`consensus` column (held-out tables only) as ground truth. For each
track:
  - consensus_pct = percentile-rank of consensus value in the held-out set
  - error_old    = |V18.1 percentile − consensus_pct|
  - error_new    = |V18.X percentile − consensus_pct|
  - winner       = "V18.X" if error_new < error_old, else "V18.1", else "tie"

For the subset of tracks with |Δpercentile| ≥ threshold (the "big shifts"
the user is interested in), reports:
  1. Aggregate winner count: which model wins more often on the shifted tracks?
  2. Mean absolute error for both, before vs after.
  3. Per-direction breakdown — was the "shifted up" cohort moved toward or
     away from consensus? Same for "shifted down".
  4. Top-N tracks where V18.X most strongly beat V18.1 (the "fixed" tracks).
  5. Top-N tracks where V18.X most strongly lost to V18.1 (the "broken" tracks).

Usage:
    python evaluate_diff.py \\
      --baseline-old "<path>/Mesh — V18.1 Held-Out Baseline.md" \\
      --baseline-new "<path>/Mesh — V18.X Held-Out Baseline.md" \\
      --out "<path>/Mesh — V18.1 vs V18.X Big-Shift Verdict.md" \\
      --threshold-pp 10
"""
from __future__ import annotations

import argparse
import sys
from datetime import datetime, timezone
from pathlib import Path

# Reuse the parser + frontmatter helpers from diff_baselines.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from diff_baselines import parse_baseline, parse_frontmatter  # noqa: E402


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--baseline-old", type=Path, required=True)
    p.add_argument("--baseline-new", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--threshold-pp", type=float, default=10.0,
                   help="evaluate tracks whose |Δpercentile| crosses this threshold")
    p.add_argument("--top-n-show", type=int, default=30,
                   help="how many fixed/broken tracks to render in each table")
    p.add_argument("--label-old", default="V18.1")
    p.add_argument("--label-new", default="V18.X")
    return p.parse_args()


def consensus_percentile_table(rows: dict) -> dict[int, float]:
    """For tracks that have a consensus value, return {tid: percentile_rank}.
    Percentile is over the held-out set's consensus distribution: highest
    consensus = 100 %, lowest = 0 %.
    """
    cons_pairs = [(tid, r["consensus"]) for tid, r in rows.items()
                  if r.get("consensus") is not None]
    if not cons_pairs:
        return {}
    cons_pairs.sort(key=lambda p: p[1])  # ascending
    n = len(cons_pairs)
    return {
        tid: 100.0 * (i / (n - 1)) if n > 1 else 50.0
        for i, (tid, _) in enumerate(cons_pairs)
    }


def main(args) -> int:
    print(f"[eval] reading {args.baseline_old.name} + {args.baseline_new.name}")
    rows_old = parse_baseline(args.baseline_old)
    rows_new = parse_baseline(args.baseline_new)
    common = set(rows_old) & set(rows_new)
    if not common:
        sys.exit("zero common track_ids")

    # Consensus percentile table — built from whichever baseline has it
    # (held-out tables include a consensus column; library tables don't).
    cons_pct_new = consensus_percentile_table(rows_new)
    cons_pct_old = consensus_percentile_table(rows_old)
    if not cons_pct_new and not cons_pct_old:
        sys.exit("Neither baseline has consensus values — this script only "
                 "works on the held-out diff (where the consensus column is "
                 "present). For the library diff, the user library has no "
                 "ground-truth labels — there's no truth to evaluate against.")
    cons_pct = cons_pct_new if cons_pct_new else cons_pct_old

    # Build per-track verdicts for the big-shift cohort
    big = []
    for tid in common:
        if tid not in cons_pct:
            continue
        old, new = rows_old[tid], rows_new[tid]
        d_pct = new["percentile"] - old["percentile"]
        if abs(d_pct) < args.threshold_pp:
            continue
        consensus_p = cons_pct[tid]
        err_old = abs(old["percentile"] - consensus_p)
        err_new = abs(new["percentile"] - consensus_p)
        improvement = err_old - err_new  # positive = V18.X is closer to consensus
        big.append({
            "track_id": tid,
            "artist": new.get("artist") or old.get("artist") or "",
            "title": new.get("title") or old.get("title") or "",
            "genre_seed": new.get("genre_seed") or old.get("genre_seed", ""),
            "consensus": old.get("consensus") or new.get("consensus"),
            "consensus_pct": consensus_p,
            "pct_old": old["percentile"],
            "pct_new": new["percentile"],
            "pct_delta": d_pct,
            "err_old": err_old,
            "err_new": err_new,
            "improvement_pp": improvement,
        })
    big.sort(key=lambda d: -d["improvement_pp"])  # most-improved first

    n_big = len(big)
    if n_big == 0:
        sys.exit(f"No tracks crossed the |Δpct| ≥ {args.threshold_pp} threshold.")

    # Aggregate verdicts
    n_v18x_wins = sum(1 for d in big if d["improvement_pp"] > 0.5)  # V18.X meaningfully closer
    n_v181_wins = sum(1 for d in big if d["improvement_pp"] < -0.5)
    n_tie = n_big - n_v18x_wins - n_v181_wins
    mean_err_old = sum(d["err_old"] for d in big) / n_big
    mean_err_new = sum(d["err_new"] for d in big) / n_big

    # Per-direction (shifted up vs down)
    up_set = [d for d in big if d["pct_delta"] > 0]
    down_set = [d for d in big if d["pct_delta"] < 0]
    up_correct = sum(1 for d in up_set if d["consensus_pct"] > d["pct_old"])
    down_correct = sum(1 for d in down_set if d["consensus_pct"] < d["pct_old"])
    up_n = len(up_set)
    down_n = len(down_set)

    # Render the verdict markdown
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    fm_old = parse_frontmatter(args.baseline_old)
    fm_new = parse_frontmatter(args.baseline_new)

    body: list[str] = []
    body.append("---")
    body.append("tags: [knowledge-base, mesh, intensity-axis, baseline-diff, round-7.7, evaluation]")
    body.append(f"created: {today}")
    body.append(f"label_old: {args.label_old}")
    body.append(f"label_new: {args.label_new}")
    body.append(f"axis_old: {fm_old.get('axis_variant', 'unknown')}")
    body.append(f"axis_new: {fm_new.get('axis_variant', 'unknown')}")
    body.append(f"threshold_pp: {args.threshold_pp}")
    body.append(f"n_big_shifts: {n_big}")
    body.append(f"n_v18x_wins: {n_v18x_wins}")
    body.append(f"n_v181_wins: {n_v181_wins}")
    body.append(f"n_tie: {n_tie}")
    body.append(f"mean_err_old: {mean_err_old:.2f}")
    body.append(f"mean_err_new: {mean_err_new:.2f}")
    body.append("related: [[Mesh — Round 7.7 Improvement Research]], [[Mesh — Round 7.7 Implementation Log]], [[Mesh — V18.1 vs V18.X Held-Out Diff]]")
    body.append("---\n")
    body.append(f"# {args.label_old} vs {args.label_new} — big-shift verdict\n")
    body.append(f"For the **{n_big} held-out tracks** whose intensity percentile "
                f"shifted by ≥ {args.threshold_pp:.0f} pp between {args.label_old} and "
                f"{args.label_new}, this doc decides per-track which model agrees "
                f"more with the consensus ground-truth (the same 3-juror Dawid-Skene "
                f"label used at training time, percentile-ranked over the held-out set).\n")
    body.append("Companion to [[Mesh — V18.1 vs V18.X Held-Out Diff]] — that doc "
                "shows WHAT shifted, this doc shows WHICH SHIFTS WERE RIGHT.\n")

    # Big aggregate verdict
    if n_v18x_wins > n_v181_wins * 1.2:
        verdict_line = f"## ✅ Verdict: {args.label_new} wins on the big shifts"
        narrative = (f"Of the {n_big} tracks that shifted ≥ {args.threshold_pp:.0f} pp, "
                     f"**{args.label_new} agrees more with consensus on {n_v18x_wins} tracks ({100.0 * n_v18x_wins / n_big:.1f} %)**, "
                     f"vs {n_v181_wins} ({100.0 * n_v181_wins / n_big:.1f} %) for {args.label_old} "
                     f"and {n_tie} ties (within ±0.5 pp). Mean absolute error to consensus drops "
                     f"from `{mean_err_old:.1f}` ({args.label_old}) to `{mean_err_new:.1f}` "
                     f"({args.label_new}) — a **{mean_err_old - mean_err_new:+.1f} pp improvement** on the shifted cohort.")
    elif n_v181_wins > n_v18x_wins * 1.2:
        verdict_line = f"## ❌ Verdict: {args.label_new} regresses on the big shifts"
        narrative = (f"Of the {n_big} tracks that shifted ≥ {args.threshold_pp:.0f} pp, "
                     f"**{args.label_old} agrees more with consensus on {n_v181_wins} tracks ({100.0 * n_v181_wins / n_big:.1f} %)**, "
                     f"vs {n_v18x_wins} ({100.0 * n_v18x_wins / n_big:.1f} %) for {args.label_new}. "
                     f"Mean absolute error rises from `{mean_err_old:.1f}` to `{mean_err_new:.1f}`. "
                     f"This is a regression on the shifted-cohort even if global PA went up — the "
                     f"model is making more confident wrong calls on the borderline tracks.")
    else:
        verdict_line = f"## ≈ Verdict: split — {args.label_new} and {args.label_old} are roughly tied on the big shifts"
        narrative = (f"Of the {n_big} tracks that shifted ≥ {args.threshold_pp:.0f} pp, "
                     f"{n_v18x_wins} go to {args.label_new}, {n_v181_wins} to {args.label_old}, "
                     f"{n_tie} ties. Mean absolute error: `{mean_err_old:.1f}` → `{mean_err_new:.1f}`. "
                     f"The substrate change reorganized the borderline cohort but didn't materially "
                     f"shift it toward consensus — the +0.18 pp aggregate PA gain came from elsewhere.")
    body.append(verdict_line + "\n")
    body.append(narrative + "\n")

    body.append("## Aggregate metrics on the big-shift cohort\n")
    body.append("| metric | value |")
    body.append("|---|---:|")
    body.append(f"| Big-shift tracks (\\|Δpct\\| ≥ {args.threshold_pp:.0f} pp) | {n_big} |")
    body.append(f"| **{args.label_new} closer to consensus** | **{n_v18x_wins} ({100.0 * n_v18x_wins / n_big:.1f} %)** |")
    body.append(f"| **{args.label_old} closer to consensus** | **{n_v181_wins} ({100.0 * n_v181_wins / n_big:.1f} %)** |")
    body.append(f"| Tied (within 0.5 pp) | {n_tie} ({100.0 * n_tie / n_big:.1f} %) |")
    body.append(f"| Mean \\|error\\| to consensus, {args.label_old} | `{mean_err_old:.2f}` pp |")
    body.append(f"| Mean \\|error\\| to consensus, {args.label_new} | `{mean_err_new:.2f}` pp |")
    body.append(f"| **Mean error delta** | **{mean_err_new - mean_err_old:+.2f} pp** "
                f"({'lower error = improvement' if mean_err_new < mean_err_old else 'higher error = regression' if mean_err_new > mean_err_old else 'unchanged'}) |")
    body.append("")

    body.append("## Per-direction breakdown\n")
    body.append("Was the shift in the *correct* direction relative to where consensus would put each track?\n")
    body.append("| cohort | n | shifts in correct direction | %  |")
    body.append("|---|---:|---:|---:|")
    body.append(f"| Shifted **up** in {args.label_new} | {up_n} | {up_correct} | "
                f"{100.0 * up_correct / max(up_n, 1):.1f} % |")
    body.append(f"| Shifted **down** in {args.label_new} | {down_n} | {down_correct} | "
                f"{100.0 * down_correct / max(down_n, 1):.1f} % |")
    body.append("")
    body.append("(Random baseline = 50 %. >50 % means shifts are systematically toward consensus; <50 % "
                "means shifts are systematically away from consensus.)\n")

    # Top fixed (V18.X most-strongly beat V18.1)
    top_fixed = big[: args.top_n_show]
    body.append(f"## Top {len(top_fixed)} tracks where {args.label_new} most-strongly beat {args.label_old}\n")
    body.append("(Sorted by `improvement` = `error_old − error_new`. High = V18.X moved closer to consensus by a lot.)\n")
    body.append("| improvement | consensus pct | pct old → new | error old → new | track_id | artist | title |")
    body.append("|---:|---:|:---|:---|---:|---|---|")
    for d in top_fixed:
        artist = (d["artist"] or "").replace("|", "\\|")
        title = (d["title"] or "").replace("|", "\\|")
        body.append(f"| {d['improvement_pp']:+.1f} | {d['consensus_pct']:.1f}% | "
                    f"{d['pct_old']:.1f}% → {d['pct_new']:.1f}% | "
                    f"{d['err_old']:.1f} → {d['err_new']:.1f} | "
                    f"{d['track_id']} | {artist} | {title} |")
    body.append("")

    # Top broken (V18.X most-strongly lost to V18.1)
    top_broken = list(reversed(big))[: args.top_n_show]
    body.append(f"## Top {len(top_broken)} tracks where {args.label_new} most-strongly lost to {args.label_old}\n")
    body.append("(Sorted by `regression` = `error_new − error_old`. High = V18.X moved further from "
                "consensus than V18.1 was.)\n")
    body.append("| regression | consensus pct | pct old → new | error old → new | track_id | artist | title |")
    body.append("|---:|---:|:---|:---|---:|---|---|")
    for d in top_broken:
        artist = (d["artist"] or "").replace("|", "\\|")
        title = (d["title"] or "").replace("|", "\\|")
        body.append(f"| {-d['improvement_pp']:+.1f} | {d['consensus_pct']:.1f}% | "
                    f"{d['pct_old']:.1f}% → {d['pct_new']:.1f}% | "
                    f"{d['err_old']:.1f} → {d['err_new']:.1f} | "
                    f"{d['track_id']} | {artist} | {title} |")
    body.append("")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text("\n".join(body) + "\n")
    print(f"[eval] wrote {args.out}")
    print(f"[eval] verdict: {args.label_new} wins {n_v18x_wins}, "
          f"{args.label_old} wins {n_v181_wins}, ties {n_tie} "
          f"(mean error: {mean_err_old:.2f} → {mean_err_new:.2f})")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
