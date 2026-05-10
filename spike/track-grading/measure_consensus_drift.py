"""Stage S9 — Drift measurement: 3-juror vs 4-juror Dawid-Skene consensus.

Round-7.7 Phase-1b decision tool. Compares the existing 3-juror consensus
NPZ against a re-aggregated 4-juror consensus NPZ (3 originals + Gemini
Flash) to decide whether the new juror brings meaningful new information.

The decision rule (per `Mesh — Round 7.7 Improvement Research.md` §Phase 1b):
    mean(|Δ|) > 0.05  ⇒  consensus is fragile, branch to Phase 1c-i
    mean(|Δ|) ≤ 0.05  ⇒  consensus is robust,  branch to Phase 1c-ii

Drift is measured per-track on the consensus_intensity field, joined on
track_ids. Output is a Markdown report to the vault.

Usage:
    bash spike/track-grading/run_r7_step.sh measure_consensus_drift.py \\
        --baseline /home/data01/Music/mesh-track-grading/round7_6_consensus_3juror_baseline.npz \\
        --updated  /home/data01/Music/mesh-track-grading/round7_6_consensus_4juror.npz \\
        --new-juror caption_text_llm_gemini_flash \\
        --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \\
        --out "/home/data01/Notes/🗂️ Collection/Mesh — 3 vs 4 Juror Drift Report.md"
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np


N_BUCKETS = 20
DRIFT_THRESHOLD = 0.05  # mean per-track |Δ| above which consensus is fragile


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--baseline", type=Path, required=True,
                   help="3-juror consensus NPZ (the pre-Gemini snapshot)")
    p.add_argument("--updated", type=Path, required=True,
                   help="4-juror consensus NPZ (re-aggregated with Gemini)")
    p.add_argument("--new-juror", type=str,
                   default="caption_text_llm_gemini_flash",
                   help="source-name of the new juror (for σ²/reliability lookup)")
    p.add_argument("--captions-root", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/"
                                "round7_6_captions/music_flamingo"))
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--top-n", type=int, default=30)
    return p.parse_args()


def load_consensus(p: Path) -> dict:
    z = np.load(p, allow_pickle=True)
    out = {
        "track_ids": np.asarray(z["track_ids"], dtype=np.int64),
        "consensus": np.asarray(z["consensus_intensity"], dtype=np.float64),
        "source_names": [str(s) for s in z["source_names"]]
                        if "source_names" in z.files else [],
        "source_sigma2": (np.asarray(z["source_sigma2"], dtype=np.float64).tolist()
                          if "source_sigma2" in z.files else []),
        "source_reliabilities": (
            np.asarray(z["source_reliabilities"], dtype=np.float64).tolist()
            if "source_reliabilities" in z.files else []),
        "coverage": (np.asarray(z["coverage"], dtype=bool)
                     if "coverage" in z.files else None),
    }
    return out


def caption_excerpt(captions_root: Path, tid: int, max_chars: int = 180) -> str:
    p = captions_root / f"{tid}.json"
    if not p.exists():
        return ""
    try:
        rec = json.loads(p.read_text())
        cap = (rec.get("caption") or "").strip()
        cap = " ".join(cap.split())  # collapse whitespace
        if len(cap) > max_chars:
            cap = cap[: max_chars - 1].rstrip() + "…"
        return cap
    except Exception:
        return ""


def fmt_md_escape(s: str) -> str:
    return s.replace("|", r"\|").replace("\n", " ").replace("\r", " ")


def main() -> int:
    args = parse_args()
    base = load_consensus(args.baseline)
    upd = load_consensus(args.updated)

    # Inner-join by track_id
    tid_to_b = {int(t): float(c) for t, c in zip(base["track_ids"], base["consensus"])}
    tid_to_u = {int(t): float(c) for t, c in zip(upd["track_ids"], upd["consensus"])}
    common = sorted(set(tid_to_b) & set(tid_to_u))
    if not common:
        print("[drift] no common track_ids between baseline and updated", flush=True)
        return 1
    arr_b = np.array([tid_to_b[t] for t in common], dtype=np.float64)
    arr_u = np.array([tid_to_u[t] for t in common], dtype=np.float64)
    delta = arr_u - arr_b
    abs_d = np.abs(delta)

    # Aggregate stats
    mean_abs = float(abs_d.mean())
    median_abs = float(np.median(abs_d))
    p90 = float(np.percentile(abs_d, 90))
    p99 = float(np.percentile(abs_d, 99))
    mx = float(abs_d.max())
    fragile = mean_abs > DRIFT_THRESHOLD
    decision = ("FRAGILE — Phase 1c-i (jurors first, then LoRA)"
                if fragile else
                "ROBUST — Phase 1c-ii (LoRA first, jurors as parallel ablations)")

    # Bucket migration matrix
    b_bucket = np.clip((arr_b * N_BUCKETS).astype(int), 0, N_BUCKETS - 1)
    u_bucket = np.clip((arr_u * N_BUCKETS).astype(int), 0, N_BUCKETS - 1)
    mig = np.zeros((N_BUCKETS, N_BUCKETS), dtype=np.int64)
    for bi, ui in zip(b_bucket, u_bucket):
        mig[bi, ui] += 1
    diag_mass = int(mig.diagonal().sum())
    off_mass = int(mig.sum() - diag_mass)
    pct_stayed = 100.0 * diag_mass / max(int(mig.sum()), 1)

    # New-juror reliability lookup + covered-subset detection
    new_juror_sigma2 = None
    new_juror_reliab = None
    new_juror_idx = None
    for i, name in enumerate(upd["source_names"]):
        if name == args.new_juror:
            new_juror_idx = i
            if i < len(upd["source_sigma2"]):
                new_juror_sigma2 = upd["source_sigma2"][i]
            if i < len(upd["source_reliabilities"]):
                new_juror_reliab = upd["source_reliabilities"][i]
            break

    # Covered-subset stats (tracks where the new juror actually contributed).
    # When the juror has full coverage, this matches the all-corpus stats.
    # When coverage is partial (e.g. quota-capped run), the all-corpus mean
    # gets diluted by tracks where Δ=0 by construction. Separate stats let
    # us make the FRAGILE/ROBUST decision on the covered subset.
    covered_mask = None
    if upd["coverage"] is not None and new_juror_idx is not None:
        cov_col = upd["coverage"][:, new_juror_idx]
        upd_tid_to_covered = {int(t): bool(cov_col[i])
                              for i, t in enumerate(upd["track_ids"])}
        covered_mask = np.array([upd_tid_to_covered.get(t, False)
                                 for t in common], dtype=bool)
    n_covered = int(covered_mask.sum()) if covered_mask is not None else len(common)
    has_partial = covered_mask is not None and n_covered < len(common)

    if has_partial:
        cov_d = abs_d[covered_mask]
        mean_abs_cov = float(cov_d.mean())
        median_abs_cov = float(np.median(cov_d))
        p90_cov = float(np.percentile(cov_d, 90))
        p99_cov = float(np.percentile(cov_d, 99))
        mx_cov = float(cov_d.max())
        fragile_cov = mean_abs_cov > DRIFT_THRESHOLD
        decision_cov = ("FRAGILE — Phase 1c-i (jurors first, then LoRA)"
                        if fragile_cov else
                        "ROBUST — Phase 1c-ii (LoRA first, jurors as parallel ablations)")
    else:
        mean_abs_cov = mean_abs
        median_abs_cov = median_abs
        p90_cov = p90
        p99_cov = p99
        mx_cov = mx
        fragile_cov = fragile
        decision_cov = decision

    # Top-N most-shifted
    order = np.argsort(-abs_d)[: args.top_n]
    top_rows = []
    for k in order:
        tid = common[k]
        top_rows.append({
            "track_id": tid,
            "old": float(arr_b[k]),
            "new": float(arr_u[k]),
            "delta": float(delta[k]),
            "caption": caption_excerpt(args.captions_root, tid),
        })

    # Per-direction breakdown
    n_up = int((delta > 1e-6).sum())
    n_down = int((delta < -1e-6).sum())
    n_flat = int((np.abs(delta) <= 1e-6).sum())

    # Render markdown
    out_lines: list[str] = []
    out_lines.append("---")
    out_lines.append("tags: [knowledge-base, mesh, intensity-axis, round-7.7, "
                     "drift-test, phase-1b]")
    out_lines.append(f"baseline: {args.baseline.name}")
    out_lines.append(f"updated: {args.updated.name}")
    out_lines.append(f"new_juror: {args.new_juror}")
    out_lines.append(f"n_common: {len(common)}")
    out_lines.append(f"mean_abs_drift: {mean_abs:.4f}")
    out_lines.append(f"decision_threshold: {DRIFT_THRESHOLD}")
    out_lines.append(f"verdict: {'FRAGILE' if fragile_cov else 'ROBUST'}")
    out_lines.append(f"verdict_basis: {'covered-subset' if has_partial else 'all-corpus'}")
    out_lines.append(f"new_juror_coverage: {n_covered}/{len(common)}")
    out_lines.append("status: drift snapshot")
    out_lines.append("---")
    out_lines.append("")
    out_lines.append("# 3-juror vs 4-juror consensus drift report")
    out_lines.append("")
    if has_partial:
        out_lines.append(f"**⚠️ Partial new-juror coverage**: the new juror "
                         f"`{args.new_juror}` covered **{n_covered} of "
                         f"{len(common)} ({100.0*n_covered/len(common):.1f} %)** "
                         f"tracks. The remaining {len(common)-n_covered} tracks "
                         f"have Δ=0 by construction (only 3 jurors contribute, "
                         f"identically to the baseline). The decision below is "
                         f"based on the **covered subset only** — the all-corpus "
                         f"stats are reported for completeness but mechanically "
                         f"diluted by ~{100.0*(len(common)-n_covered)/len(common):.0f} %.")
        out_lines.append("")
    out_lines.append(f"**Decision: {decision_cov}**")
    out_lines.append(f"  *(based on {'covered-subset' if has_partial else 'all-corpus'} "
                     f"mean \\|Δ\\| = {mean_abs_cov:.4f})*")
    out_lines.append("")
    out_lines.append(f"Per-track absolute drift on `consensus_intensity`, inner-joined "
                     f"on `track_ids` between the 3-juror baseline and the 4-juror "
                     f"re-aggregated consensus.")
    out_lines.append("")
    if has_partial:
        out_lines.append("## Drift on covered subset (load-bearing)")
        out_lines.append("")
        out_lines.append(f"Tracks where `{args.new_juror}` actually contributed "
                         f"to the 4-juror EM (n = **{n_covered}**).")
        out_lines.append("")
        out_lines.append("| metric | value | interpretation |")
        out_lines.append("|---|---:|---|")
        out_lines.append(f"| mean \\|Δ\\| | **{mean_abs_cov:.4f}** | "
                         f"{'> ' if fragile_cov else '≤ '}{DRIFT_THRESHOLD} threshold "
                         f"⇒ {'FRAGILE' if fragile_cov else 'ROBUST'} |")
        out_lines.append(f"| median \\|Δ\\| | {median_abs_cov:.4f} | typical track shift |")
        out_lines.append(f"| p90 \\|Δ\\| | {p90_cov:.4f} | tail shift |")
        out_lines.append(f"| p99 \\|Δ\\| | {p99_cov:.4f} | extreme tail |")
        out_lines.append(f"| max \\|Δ\\| | {mx_cov:.4f} | worst single shift |")
        out_lines.append("")
    out_lines.append("## All-corpus drift (diluted if coverage is partial)")
    out_lines.append("")
    out_lines.append("| metric | value | interpretation |")
    out_lines.append("|---|---:|---|")
    out_lines.append(f"| n_common (joined tracks) | {len(common)} | size of the comparison |")
    out_lines.append(f"| mean \\|Δ\\| | **{mean_abs:.4f}** | "
                     f"{'> ' if fragile else '≤ '}{DRIFT_THRESHOLD} threshold "
                     f"⇒ {'FRAGILE' if fragile else 'ROBUST'} (all-corpus basis) |")
    out_lines.append(f"| median \\|Δ\\| | {median_abs:.4f} | typical track shift |")
    out_lines.append(f"| p90 \\|Δ\\| | {p90:.4f} | tail shift |")
    out_lines.append(f"| p99 \\|Δ\\| | {p99:.4f} | extreme tail |")
    out_lines.append(f"| max \\|Δ\\| | {mx:.4f} | worst single shift |")
    out_lines.append(f"| moved up (Δ > 0) | {n_up} ({100.0*n_up/len(common):.1f} %) | "
                     "Gemini pulled consensus up |")
    out_lines.append(f"| moved down (Δ < 0) | {n_down} ({100.0*n_down/len(common):.1f} %) | "
                     "Gemini pulled consensus down |")
    out_lines.append(f"| unchanged | {n_flat} | identical to within 1e-6 |")
    out_lines.append("")
    out_lines.append("## New juror reliability (from 4-juror EM)")
    out_lines.append("")
    if new_juror_idx is None:
        out_lines.append(f"⚠️ Source name `{args.new_juror}` not found in updated NPZ. "
                         f"Available sources: {upd['source_names']}")
    else:
        rels = upd["source_reliabilities"]
        sig2 = upd["source_sigma2"]
        out_lines.append("| source | σ² | reliability (1/σ², normalised) |")
        out_lines.append("|---|---:|---:|")
        for i, name in enumerate(upd["source_names"]):
            tag = " ← **new juror**" if i == new_juror_idx else ""
            s2 = f"{sig2[i]:.5f}" if i < len(sig2) else "—"
            r = f"{rels[i]:.4f}" if i < len(rels) else "—"
            out_lines.append(f"| {name}{tag} | {s2} | {r} |")
        out_lines.append("")
        if new_juror_sigma2 is not None and new_juror_sigma2 <= 0.011:
            out_lines.append(f"⚠️ New juror σ² = **{new_juror_sigma2:.5f}** — at or "
                             f"near the σ²-floor (0.01). EM treats it as equal-weight "
                             f"with the others. Consider this when interpreting drift "
                             f"magnitude — mechanical attenuation is `|Gemini − old| / 4` "
                             f"in the floor regime.")
            out_lines.append("")
    out_lines.append("## Bucket migration (20-bucket, percent of total)")
    out_lines.append("")
    out_lines.append(f"Diagonal (no bucket change): **{diag_mass}** ({pct_stayed:.1f} %). "
                     f"Off-diagonal: {off_mass} ({100.0 - pct_stayed:.1f} %).")
    out_lines.append("")
    out_lines.append("Migration matrix rows = old bucket, columns = new bucket. Showing "
                     "non-zero entries only:")
    out_lines.append("")
    out_lines.append("| old\\new | " + " | ".join(f"{c:02d}" for c in range(N_BUCKETS)) + " |")
    out_lines.append("|---:|" + "---:|" * N_BUCKETS)
    for r in range(N_BUCKETS):
        if mig[r].sum() == 0:
            continue
        cells = [(f"**{mig[r, c]}**" if r == c and mig[r, c] > 0
                  else (str(mig[r, c]) if mig[r, c] > 0 else ""))
                 for c in range(N_BUCKETS)]
        out_lines.append(f"| {r:02d} | " + " | ".join(cells) + " |")
    out_lines.append("")
    out_lines.append(f"## Top {args.top_n} most-shifted tracks")
    out_lines.append("")
    out_lines.append("Sorted by descending |Δ|. Caption excerpts (≤180 chars) help "
                     "judge whether the new juror is correcting a mis-rating or "
                     "introducing one.")
    out_lines.append("")
    out_lines.append("| # | track_id | old → new | Δ | caption |")
    out_lines.append("|---:|---:|---|---:|---|")
    for i, row in enumerate(top_rows, 1):
        out_lines.append(
            f"| {i} | {row['track_id']} | {row['old']:.4f} → {row['new']:.4f} | "
            f"{row['delta']:+.4f} | {fmt_md_escape(row['caption'])} |"
        )
    out_lines.append("")
    out_lines.append("## How to read this report")
    out_lines.append("")
    out_lines.append(f"- **Mean |Δ| > {DRIFT_THRESHOLD}** ⇒ Gemini Flash brings new "
                     f"information not redundant with the existing 3-juror panel. "
                     f"Branch to **Phase 1c-i**: land all remaining new-information "
                     f"jurors (B4 specialists + DeepSeek + Snorkel) BEFORE training E4 "
                     f"LoRA so the LoRA pairs come from a stable consensus.")
    out_lines.append(f"- **Mean |Δ| ≤ {DRIFT_THRESHOLD}** ⇒ Gemini Flash agrees with "
                     f"the existing consensus to within ~1 bucket on average. Branch to "
                     f"**Phase 1c-ii**: train E4 LoRA against the current 4-juror "
                     f"consensus immediately; remaining jurors are parallel ablations.")
    out_lines.append("- **σ²-floor caveat**: if the new juror's σ² lands at 0.01 (the "
                     "EM floor), drift is mechanically attenuated by 1/N. A small drift "
                     "in the floor regime does NOT prove juror redundancy — it could "
                     "also mean the EM under-weights a useful new signal. Cross-check "
                     "the reliability table.")
    out_lines.append("- **Migration matrix**: clusters near the diagonal mean tracks "
                     "stayed in their bucket; off-diagonal mass means meaningful "
                     "rebucketing. Symmetric migration (i→j and j→i) suggests noise; "
                     "asymmetric migration (e.g., consistent up-shift) suggests "
                     "systematic rating change.")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text("\n".join(out_lines) + "\n")
    print()
    print("=== drift report ===")
    print(f"n_common:       {len(common)}")
    if has_partial:
        print(f"new-juror coverage: {n_covered}/{len(common)} "
              f"({100.0*n_covered/len(common):.1f} %)")
        print(f"covered-subset mean |Δ|: {mean_abs_cov:.4f}  "
              f"({'FRAGILE' if fragile_cov else 'ROBUST'})")
        print(f"covered-subset p90/p99/max: {p90_cov:.4f} / {p99_cov:.4f} / {mx_cov:.4f}")
        print(f"all-corpus mean |Δ|:    {mean_abs:.4f}  (diluted)")
    else:
        print(f"mean |Δ|:       {mean_abs:.4f}  ({'FRAGILE' if fragile else 'ROBUST'})")
        print(f"median |Δ|:     {median_abs:.4f}")
        print(f"p90 / p99 / max: {p90:.4f} / {p99:.4f} / {mx:.4f}")
    if new_juror_sigma2 is not None:
        print(f"new juror σ²:   {new_juror_sigma2:.5f}  "
              f"reliability: {new_juror_reliab:.4f}")
    print(f"verdict: {decision_cov}")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
