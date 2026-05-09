"""Compare V18.1 vs V18.X intensity-axis baselines and render a diff table.

Reads two pre-existing baseline markdown files (e.g. the held-out tables or
the user-library tables), joins per `track_id`, computes per-track Δ score
and Δ percentile, and renders:

  - Frontmatter + summary stats (mean / p50 / p90 / max |Δ|, sign histogram).
  - Top-N most-shifted tracks with both old + new score and percentile.
  - Optional verdict: did intensity-rank ordering improve vs the consensus?
    (Only available when the held-out table includes a `consensus` column.)

Intent: drop into round-7.7 evaluation. Run once for the held-out test set
and once for the user library; both diffs land alongside the source
baselines in the Obsidian vault.

Usage:
    python diff_baselines.py \\
        --baseline-old "<path>/Mesh — V18.1 Held-Out Baseline.md" \\
        --baseline-new "<path>/Mesh — V18.X Held-Out Baseline.md" \\
        --out "<path>/Mesh — V18.1 vs V18.X Held-Out Diff.md" \\
        --top-n 50
"""
from __future__ import annotations

import argparse
import re
from datetime import datetime, timezone
from pathlib import Path


_TABLE_ROW = re.compile(r"^\|\s*(\d+)\s*\|")  # rank starts at 1, must be int


def parse_baseline(path: Path) -> dict:
    """Return {track_id: {score, percentile, rank, artist, title, consensus?, genre_seed?}}."""
    text = path.read_text()
    # Find the table — first markdown table with rank/percentile/score columns.
    lines = text.splitlines()
    rows: dict[int, dict] = {}
    in_table = False
    header: list[str] = []
    for line in lines:
        if line.strip().startswith("| rank") and "score" in line:
            header = [c.strip() for c in line.strip().strip("|").split("|")]
            in_table = True
            continue
        if in_table:
            if not line.strip().startswith("|"):
                in_table = False
                continue
            if line.strip().startswith("|---"):
                continue
            if not _TABLE_ROW.match(line):
                continue
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) < len(header):
                continue
            row = dict(zip(header, cells))
            try:
                tid = int(row["track_id"])
            except (KeyError, ValueError):
                continue
            try:
                score = float(row["score"])
                rank = int(row["rank"])
                pct_raw = row["percentile"].rstrip("%").strip()
                percentile = float(pct_raw)
            except (KeyError, ValueError):
                continue
            entry = {
                "score": score,
                "percentile": percentile,
                "rank": rank,
                "artist": row.get("artist", ""),
                "title": row.get("title", ""),
            }
            if "consensus" in row and row["consensus"]:
                try:
                    entry["consensus"] = float(row["consensus"])
                except ValueError:
                    pass
            if "genre_seed" in row:
                entry["genre_seed"] = row["genre_seed"]
            if "bpm" in row:
                entry["bpm"] = row["bpm"]
            if "key" in row:
                entry["key"] = row["key"]
            rows[tid] = entry
    return rows


def parse_frontmatter(path: Path) -> dict:
    text = path.read_text()
    if not text.startswith("---"):
        return {}
    end = text.find("\n---", 3)
    if end < 0:
        return {}
    fm: dict = {}
    for line in text[3:end].splitlines():
        if ":" in line:
            k, v = line.split(":", 1)
            fm[k.strip()] = v.strip()
    return fm


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--baseline-old", type=Path, required=True)
    p.add_argument("--baseline-new", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--top-n", type=int, default=50,
                   help="how many most-shifted tracks to render in the focused top-N table")
    p.add_argument("--threshold-pp", type=float, default=10.0,
                   help="render ALL tracks with |Δpercentile| ≥ this many pp in a "
                        "second (uncapped) table. Set to 0 to disable.")
    p.add_argument("--label-old", default="V18.1",
                   help="short label for the old baseline (display only)")
    p.add_argument("--label-new", default="V18.X",
                   help="short label for the new baseline (display only)")
    p.add_argument("--scope", default="held-out",
                   choices=["held-out", "library"],
                   help="display label for the diff scope")
    return p.parse_args()


def quartiles(values: list[float]) -> tuple[float, float, float, float, float]:
    """Return (min, p25, p50, p75, max). Empty input → all zeros."""
    if not values:
        return (0.0, 0.0, 0.0, 0.0, 0.0)
    sv = sorted(values)
    n = len(sv)
    def at(q: float) -> float:
        i = max(0, min(n - 1, int(round((n - 1) * q))))
        return sv[i]
    return (sv[0], at(0.25), at(0.50), at(0.75), sv[-1])


def pa_against_consensus(scored: list[tuple[float, float]]) -> float | None:
    """Pairwise agreement: fraction of (i, j) pairs where sign(score_i - score_j)
    matches sign(consensus_i - consensus_j). Strict-strict (ties excluded).
    Return None if fewer than 2 valid pairs."""
    n = len(scored)
    if n < 2:
        return None
    correct = 0
    valid = 0
    for i in range(n):
        s_i, y_i = scored[i]
        for j in range(i + 1, n):
            s_j, y_j = scored[j]
            ds = s_i - s_j
            dy = y_i - y_j
            if ds == 0 or dy == 0:
                continue
            valid += 1
            if (ds > 0) == (dy > 0):
                correct += 1
    return correct / valid if valid else None


def main(args) -> int:
    print(f"[diff] reading old: {args.baseline_old}")
    fm_old = parse_frontmatter(args.baseline_old)
    rows_old = parse_baseline(args.baseline_old)
    print(f"[diff]   parsed {len(rows_old)} rows")

    print(f"[diff] reading new: {args.baseline_new}")
    fm_new = parse_frontmatter(args.baseline_new)
    rows_new = parse_baseline(args.baseline_new)
    print(f"[diff]   parsed {len(rows_new)} rows")

    common_ids = set(rows_old) & set(rows_new)
    print(f"[diff] common track_ids: {len(common_ids)}")
    if not common_ids:
        print("[diff] FATAL: zero common track_ids — table parsing went sideways")
        return 1

    # ── Per-track deltas ──
    deltas = []
    for tid in common_ids:
        old = rows_old[tid]
        new = rows_new[tid]
        deltas.append({
            "track_id": tid,
            "artist": new.get("artist") or old.get("artist") or "",
            "title": new.get("title") or old.get("title") or "",
            "score_old": old["score"],
            "score_new": new["score"],
            "score_delta": new["score"] - old["score"],
            "pct_old": old["percentile"],
            "pct_new": new["percentile"],
            "pct_delta": new["percentile"] - old["percentile"],
            "rank_old": old["rank"],
            "rank_new": new["rank"],
            "rank_delta": new["rank"] - old["rank"],
            "consensus": new.get("consensus") or old.get("consensus"),
            "genre_seed": new.get("genre_seed") or old.get("genre_seed", ""),
        })

    # Sort by absolute percentile shift (most-impacted first; rank/percentile is
    # the human-readable signal, not the raw score).
    deltas.sort(key=lambda d: -abs(d["pct_delta"]))

    n = len(deltas)
    abs_pct = [abs(d["pct_delta"]) for d in deltas]
    abs_score = [abs(d["score_delta"]) for d in deltas]
    pct_min, pct_p25, pct_p50, pct_p75, pct_max = quartiles(abs_pct)
    score_min, score_p25, score_p50, score_p75, score_max = quartiles(abs_score)
    n_up = sum(1 for d in deltas if d["pct_delta"] > 0)
    n_down = sum(1 for d in deltas if d["pct_delta"] < 0)
    n_tie = sum(1 for d in deltas if d["pct_delta"] == 0)
    big_shifts = sum(1 for d in deltas if abs(d["pct_delta"]) >= 10.0)

    # PA against consensus where available (held-out tables have it; library tables don't)
    pa_old = pa_against_consensus(
        [(d["score_old"], d["consensus"]) for d in deltas if d["consensus"] is not None]
    )
    pa_new = pa_against_consensus(
        [(d["score_new"], d["consensus"]) for d in deltas if d["consensus"] is not None]
    )

    # ── Build the markdown ──
    args.out.parent.mkdir(parents=True, exist_ok=True)
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    body: list[str] = []
    body.append("---")
    body.append("tags: [knowledge-base, mesh, intensity-axis, baseline-diff, round-7.7]")
    body.append(f"created: {today}")
    body.append(f"scope: {args.scope}")
    body.append(f"label_old: {args.label_old}")
    body.append(f"label_new: {args.label_new}")
    body.append(f"baseline_old: {args.baseline_old.name}")
    body.append(f"baseline_new: {args.baseline_new.name}")
    body.append(f"axis_old: {fm_old.get('axis_variant', 'unknown')}")
    body.append(f"axis_new: {fm_new.get('axis_variant', 'unknown')}")
    body.append(f"axis_substrate_old: {fm_old.get('axis_substrate', fm_old.get('axis_kind', ''))}")
    body.append(f"axis_substrate_new: {fm_new.get('axis_substrate', fm_new.get('axis_kind', ''))}")
    body.append(f"n_compared: {n}")
    body.append(f"n_shifted_up: {n_up}")
    body.append(f"n_shifted_down: {n_down}")
    body.append(f"n_unchanged: {n_tie}")
    body.append(f"n_big_shifts_ge_10pp: {big_shifts}")
    if pa_old is not None and pa_new is not None:
        body.append(f"pa_old: {pa_old:.4f}")
        body.append(f"pa_new: {pa_new:.4f}")
        body.append(f"pa_delta: {pa_new - pa_old:+.4f}")
    body.append("related: [[Mesh — Round 7.7 Improvement Research]], [[Mesh — Round 7.7 Implementation Log]]")
    body.append("---\n")
    body.append(f"# {args.label_old} vs {args.label_new} — {args.scope} diff\n")
    body.append(f"Per-track intensity-rank shift between **{args.label_old}** "
                f"(`{fm_old.get('axis_substrate', fm_old.get('axis_kind', '512-d'))}`) and "
                f"**{args.label_new}** (`{fm_new.get('axis_substrate', fm_new.get('axis_kind', '1024-d'))}`) "
                f"on the same {n} {args.scope} tracks. Joined by `track_id`. "
                f"See [[Mesh — Round 7.7 Implementation Log]] §Step 6 for context.\n")

    body.append("## Summary\n")
    body.append("| metric | value |")
    body.append("|---|---:|")
    body.append(f"| Tracks compared | {n} |")
    body.append(f"| Shifted **up** in percentile | {n_up} ({100.0 * n_up / max(n, 1):.1f} %) |")
    body.append(f"| Shifted **down** | {n_down} ({100.0 * n_down / max(n, 1):.1f} %) |")
    body.append(f"| Unchanged | {n_tie} ({100.0 * n_tie / max(n, 1):.1f} %) |")
    body.append(f"| **Big shifts** (\\|Δpct\\| ≥ 10 pp) | **{big_shifts} ({100.0 * big_shifts / max(n, 1):.1f} %)** |")
    body.append(f"| \\|Δpercentile\\| distribution (min · p25 · p50 · p75 · max) | "
                f"`{pct_min:.1f}` · `{pct_p25:.1f}` · `{pct_p50:.1f}` · `{pct_p75:.1f}` · `{pct_max:.1f}` |")
    body.append(f"| \\|Δscore\\| distribution (min · p25 · p50 · p75 · max) | "
                f"`{score_min:+.4f}` · `{score_p25:+.4f}` · `{score_p50:+.4f}` · `{score_p75:+.4f}` · `{score_max:+.4f}` |")
    if pa_old is not None and pa_new is not None:
        verdict_word = ('improvement' if pa_new > pa_old + 0.001
                        else 'regression' if pa_new < pa_old - 0.001
                        else 'unchanged within noise')
        body.append(f"| **PA vs consensus, {args.label_old}** | `{pa_old:.4f}` |")
        body.append(f"| **PA vs consensus, {args.label_new}** | `{pa_new:.4f}` |")
        body.append(f"| **PA delta** | **{pa_new - pa_old:+.4f}** ({verdict_word}) |")
    body.append("")

    # Verdict heuristic — only meaningful when consensus is available.
    if pa_old is not None and pa_new is not None:
        if pa_new > pa_old + 0.001:
            verdict = "✅ **{} is an improvement**".format(args.label_new)
        elif pa_new < pa_old - 0.001:
            verdict = "❌ **{} is a regression**".format(args.label_new)
        else:
            verdict = "≈ **{} matches {} within noise**".format(args.label_new, args.label_old)
        body.append(f"### Verdict: {verdict}\n")

    def render_row(d: dict) -> str:
        artist = (d["artist"] or "").replace("|", "\\|")
        title = (d["title"] or "").replace("|", "\\|")
        genre = (d["genre_seed"] or "").replace("|", "\\|")
        rank_delta_str = f"{d['rank_delta']:+d}" if d["rank_delta"] != 0 else "0"
        pct_str = f"{d['pct_old']:.1f}% → {d['pct_new']:.1f}% ({d['pct_delta']:+.1f})"
        score_str = f"{d['score_old']:+.4f} → {d['score_new']:+.4f} ({d['score_delta']:+.4f})"
        return (f"| {rank_delta_str} | {pct_str} | {score_str} | "
                f"{d['track_id']} | {artist} | {title} | {genre} |")

    body.append(f"## Top {min(args.top_n, n)} most-shifted tracks (by |Δpercentile|)\n")
    body.append("| rank Δ | pct old → new (Δ) | score old → new (Δ) | track_id | artist | title | genre_seed |")
    body.append("|---:|:---|:---|---:|---|---|---|")
    for d in deltas[: args.top_n]:
        body.append(render_row(d))
    body.append("")

    # Optional all-above-threshold table — uncapped, sorted same way (descending |Δpct|).
    if args.threshold_pp > 0:
        big = [d for d in deltas if abs(d["pct_delta"]) >= args.threshold_pp]
        body.append(f"## All {len(big)} tracks with |Δpercentile| ≥ {args.threshold_pp:.0f} pp\n")
        body.append("Sorted by |Δpercentile| descending. Cross-reference with the top-N "
                    "table above for the heaviest hitters; everything else here is the "
                    "long tail of moderate shifts.\n")
        if big:
            body.append("| rank Δ | pct old → new (Δ) | score old → new (Δ) | track_id | artist | title | genre_seed |")
            body.append("|---:|:---|:---|---:|---|---|---|")
            for d in big:
                body.append(render_row(d))
        else:
            body.append("_(no tracks crossed the threshold — substrate change is uniformly small.)_")
        body.append("")

    args.out.write_text("\n".join(body) + "\n")
    print(f"[diff] wrote {args.out}")
    print(f"[diff] summary: {n} tracks, {n_up} up, {n_down} down, {n_tie} unchanged, "
          f"{big_shifts} big shifts (≥10 pp)")
    if pa_old is not None and pa_new is not None:
        print(f"[diff] PA: {args.label_old} {pa_old:.4f} → {args.label_new} {pa_new:.4f} "
              f"(Δ {pa_new - pa_old:+.4f})")
    return 0


if __name__ == "__main__":
    import sys
    sys.exit(main(parse_args()))
