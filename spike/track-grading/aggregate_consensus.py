"""Stages S6+S7+S8 — Build the 4-source consensus intensity label.

Loads:
  S6a  r7.5 BT-prior intensity (computed inline from round7_5_priors.npz
       via a learned blend over 16 axes, simplified: ListMLE→max-correlation
       with `aggressive_overall`-projected target, falling back to a
       weighted z-mean over a curated pro-high axis subset)
  S6b  r7.5 mined-tag aggressive_overall evidence (already a per-track scalar)
  S4   caption→text-LLM intensity (NPZ from caption_intensity_rating.py)
  S6c  MF Likert intensity (optional; partial coverage; read from
       round7_6_likert/music_flamingo/<axis>/<tid>.json if present)

Then:
  S7  rank-normalize each source per track (empirical CDF) → [0, 1]
  S8  Dawid-Skene EM over 4 sources with continuous-Gaussian likelihood
      → consensus_intensity ∈ [0, 1] + per-source σ²

Per spec § 13–14. Per G7: source_category / genre prior is NOT a label
source.

Usage:
    bash spike/track-grading/run_r7_step.sh aggregate_consensus.py \\
         --bt-priors /home/data01/Music/mesh-track-grading/round7_5_priors.npz \\
         --bt-tags  /home/data01/Music/mesh-track-grading/round7_5_tags.npz \\
         --cap-intensity /home/data01/Music/mesh-track-grading/round7_6_caption_intensity.npz \\
         --likert-root  /home/data01/Music/mesh-track-grading/round7_6_likert/music_flamingo \\
         --out /home/data01/Music/mesh-track-grading/round7_6_consensus.npz
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np


# Pro-high axes used for the BT-prior intensity blend (§ S6a). These are
# the axes whose HIGH pole is meaningfully correlated with "intensity for
# DJ purposes". Sign per polar prompt convention: HIGH = more intense.
PRO_HIGH_AXES = [
    "timbre_roughness",
    "vocal_aggression",
    "bass_presence",
    "noise_layer",
    "drop_architecture",
    "textural_density",
    "onset_density",
    "tempo_perception",
]

# Likert axes used as the MF-Likert intensity proxy (S6c). Same convention.
LIKERT_PRO_HIGH = PRO_HIGH_AXES


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--bt-priors", type=Path, required=True)
    p.add_argument("--bt-tags", type=Path, required=True)
    p.add_argument("--cap-intensity", type=Path, action="append", required=True,
                   help="path to a caption→text-LLM intensity NPZ; may be "
                        "specified multiple times (one per text-LLM source). "
                        "Each gets its own jury slot named "
                        "'caption_text_llm_<filename_stem>' (e.g. _smoke, "
                        "_nemotron, _local_3b).")
    p.add_argument("--likert-root", type=Path, default=None,
                   help="optional MF Likert directory; if missing or empty, S6c is dropped")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--em-tol", type=float, default=1e-5)
    p.add_argument("--em-max-iter", type=int, default=200)
    return p.parse_args()


def rank_normalize(x: np.ndarray) -> np.ndarray:
    """Empirical CDF: maps x to (rank+0.5)/N ∈ (0, 1). NaNs preserved."""
    out = np.full_like(x, np.nan, dtype=np.float64)
    mask = ~np.isnan(x)
    if mask.sum() == 0:
        return out
    vals = x[mask]
    ranks = np.argsort(np.argsort(vals)).astype(np.float64)
    out[mask] = (ranks + 0.5) / mask.sum()
    return out


def load_btprior_intensity(priors_path: Path, tags_path: Path) -> tuple[np.ndarray, np.ndarray]:
    """S6a — Blend r7.5 BT priors across pro-high axes into a 1d intensity.
    Returns (track_ids, score)."""
    z = np.load(priors_path, allow_pickle=True)
    tids = z["track_ids"].astype(np.int64)
    axes = list(z["axes"])
    scores = z["scores"]                      # [16, N]

    # Z-score each axis row
    rows = []
    for aid in PRO_HIGH_AXES:
        if aid not in axes:
            print(f"[btprior] WARNING: axis '{aid}' not in r7.5 priors; skipping",
                  file=sys.stderr)
            continue
        j = axes.index(aid)
        col = scores[j].astype(np.float64)
        col = (col - np.nanmean(col)) / (np.nanstd(col) + 1e-9)
        rows.append(col)
    if not rows:
        sys.exit("no pro-high axes found in r7.5 priors — cannot compute S6a")
    intensity = np.nanmean(np.stack(rows, axis=0), axis=0)
    print(f"[btprior] S6a — z-mean over {len(rows)} pro-high axes: "
          f"std={np.nanstd(intensity):.3f}")
    return tids, intensity.astype(np.float32)


def load_aggressive_tag(tags_path: Path) -> tuple[np.ndarray, np.ndarray]:
    """S6b — Just the aggressive_overall mined-evidence column."""
    z = np.load(tags_path, allow_pickle=True)
    tids = z["track_ids"].astype(np.int64)
    tag_names = list(z["tag_names"])
    if "aggressive_overall" not in tag_names:
        sys.exit("'aggressive_overall' not in r7.5 tags")
    j = tag_names.index("aggressive_overall")
    col = z["tag_evidence"][:, j].astype(np.float32)
    print(f"[aggtag]  S6b — aggressive_overall evidence: "
          f"mean={col.mean():.2f} std={col.std():.2f} nonzero={(col != 0).sum()}")
    return tids, col


def load_caption_intensity(path: Path) -> tuple[np.ndarray, np.ndarray]:
    """S4 — caption→text-LLM rating, already a per-track scalar."""
    z = np.load(path, allow_pickle=True)
    tids = z["track_ids"].astype(np.int64)
    score = z["score"].astype(np.float32)
    print(f"[capint]  S4 — caption-text-LLM intensity: "
          f"mean={score.mean():.3f} std={score.std():.3f}")
    return tids, score


def load_mf_likert_intensity(root: Path) -> tuple[np.ndarray, np.ndarray] | None:
    """S6c (optional) — MF Likert intensity = z-mean over pro-high Likert axes."""
    if root is None or not root.exists():
        print("[likert]  S6c — no Likert dir; skipping")
        return None
    by_tid: dict[int, dict[str, float]] = {}
    n_files = 0
    for axis_dir in sorted(root.iterdir()):
        if not axis_dir.is_dir() or axis_dir.name not in LIKERT_PRO_HIGH:
            continue
        for f in axis_dir.glob("*.json"):
            n_files += 1
            try:
                rec = json.loads(f.read_text())
            except Exception:
                continue
            tid = int(rec["track_id"])
            sc = rec.get("score")
            if sc is None or (isinstance(sc, float) and math.isnan(sc)):
                continue
            by_tid.setdefault(tid, {})[axis_dir.name] = float(sc)
    if not by_tid:
        print(f"[likert]  S6c — Likert dir present but no usable cells; skipping")
        return None
    # Need same axes per track for fair z-mean
    axis_to_col: dict[str, list[tuple[int, float]]] = {a: [] for a in LIKERT_PRO_HIGH}
    for tid, axis_map in by_tid.items():
        for a, v in axis_map.items():
            axis_to_col[a].append((tid, v))
    # z-score per axis
    z_per_axis: dict[str, dict[int, float]] = {}
    for a, pairs in axis_to_col.items():
        if not pairs: continue
        arr = np.array([v for _, v in pairs], dtype=np.float64)
        m = arr.mean(); s = arr.std() + 1e-9
        z_per_axis[a] = {tid: float((v - m) / s) for tid, v in pairs}
    # mean across axes per track
    tids: list[int] = []
    vals: list[float] = []
    for tid in by_tid:
        zs = [z_per_axis[a][tid] for a in z_per_axis if tid in z_per_axis[a]]
        if not zs: continue
        tids.append(tid)
        vals.append(float(np.mean(zs)))
    print(f"[likert]  S6c — MF Likert intensity: covered {len(tids)} tracks, "
          f"std={np.std(vals):.3f}")
    return np.array(tids, dtype=np.int64), np.array(vals, dtype=np.float32)


def dawid_skene_em(
    sources: dict[str, np.ndarray],   # per-source [N] arrays in [0, 1], NaN where missing
    tol: float = 1e-5,
    max_iter: int = 200,
) -> tuple[np.ndarray, dict[str, float]]:
    """Continuous-target Dawid-Skene-style EM.

    Latent z_i ∈ [0,1]. Each source s contributes:
        x_norm_s_i ~ Normal(z_i, σ_s²)  (truncation to [0,1] approximated)
    E-step: precision-weighted mean over covered sources.
    M-step: σ_s² = mean_i {covered} (x_norm_s_i - z_i)².
    Returns (z, sigma2_per_source).
    """
    src_names = list(sources.keys())
    S = len(src_names)
    N = next(len(v) for v in sources.values())
    X = np.stack([sources[s] for s in src_names], axis=0)   # [S, N]
    cov = ~np.isnan(X)                                       # [S, N]

    # Init z = nanmean across sources; σ² = 1
    sigma2 = np.ones(S, dtype=np.float64)
    z = np.nanmean(X, axis=0)
    z[np.isnan(z)] = 0.5

    for it in range(max_iter):
        # E: z_i = Σ_s [covered]·X / σ²_s  /  Σ_s [covered] / σ²_s
        prec = np.where(cov, 1.0 / sigma2[:, None], 0.0)     # [S, N]
        num = np.nansum(np.where(cov, X * prec, 0.0), axis=0)
        den = prec.sum(axis=0)
        z_new = np.where(den > 0, num / np.maximum(den, 1e-12), z)

        # M: σ² = mean (x - z)² over covered tracks
        sigma2_new = sigma2.copy()
        for s in range(S):
            mask = cov[s]
            if mask.sum() == 0:
                sigma2_new[s] = 1.0
                continue
            sigma2_new[s] = float(np.mean((X[s, mask] - z_new[mask]) ** 2)) + 1e-9

        delta = float(np.max(np.abs(sigma2_new - sigma2)))
        sigma2 = sigma2_new
        z = z_new
        if delta < tol:
            print(f"[ds] converged at iter {it+1}  Δσ²={delta:.2e}")
            break
    else:
        print(f"[ds] hit max_iter={max_iter}  Δσ²={delta:.2e}")

    return z.astype(np.float32), {n: float(s) for n, s in zip(src_names, sigma2)}


def main(args) -> int:
    # ── Load all sources ────────────────────────────────────────────────
    bt_tids, bt_score = load_btprior_intensity(args.bt_priors, args.bt_tags)
    agg_tids, agg_score = load_aggressive_tag(args.bt_tags)
    # One or more caption-intensity NPZs (S4). Each becomes its own jury slot.
    cap_sources: dict[str, tuple[np.ndarray, np.ndarray]] = {}
    for p in args.cap_intensity:
        # Source name = "caption_text_llm" if single source; otherwise
        # disambiguate via the file stem (e.g., _nemotron, _local_3b).
        if len(args.cap_intensity) == 1:
            name = "caption_text_llm"
        else:
            stem = p.stem
            stem = stem.replace("round7_6_caption_intensity", "")
            stem = stem.lstrip("_") or "default"
            name = f"caption_text_llm_{stem}"
        # Stem-collision guard: if two paths produce the same name (e.g.,
        # same filename in different dirs), we'd silently lose one source's
        # signal. Disambiguate by appending the parent dir name.
        if name in cap_sources:
            disambig = f"{name}__{p.parent.name}"
            print(f"[capint]  WARNING: source name '{name}' already registered "
                  f"from a previous --cap-intensity; renaming to '{disambig}' "
                  f"to avoid collision",
                  file=sys.stderr)
            name = disambig
            if name in cap_sources:
                sys.exit(f"[capint] FATAL: cannot disambiguate '{name}' "
                         f"— pass distinct file paths")
        cap_sources[name] = load_caption_intensity(p)
        print(f"[capint]  registered as jury source: {name}  ({p})")
    lik = load_mf_likert_intensity(args.likert_root) if args.likert_root else None

    # ── Build the union track list ──────────────────────────────────────
    all_tids = sorted(set(bt_tids.tolist()) | set(agg_tids.tolist())
                      | {t for cs in cap_sources.values() for t in cs[0].tolist()}
                      | (set(lik[0].tolist()) if lik else set()))
    N = len(all_tids)
    tid_to_i = {t: i for i, t in enumerate(all_tids)}
    print(f"[agg] N={N} unique track IDs across sources")

    def expand(tids: np.ndarray, vals: np.ndarray) -> np.ndarray:
        out = np.full(N, np.nan, dtype=np.float64)
        for t, v in zip(tids.tolist(), vals.tolist()):
            i = tid_to_i.get(int(t))
            if i is not None:
                out[i] = v
        return out

    raw = {
        "r7.5_bt_blend":           expand(bt_tids, bt_score),
        "aggressive_overall_tag":  expand(agg_tids, agg_score),
    }
    for name, (tids, scores) in cap_sources.items():
        raw[name] = expand(tids, scores)
    if lik is not None:
        raw["MF_Likert"] = expand(lik[0], lik[1])

    # ── S7 rank-normalize each source ──────────────────────────────────
    rn = {s: rank_normalize(v) for s, v in raw.items()}
    print(f"[s7] rank-normalized {len(rn)} sources")
    for s, v in rn.items():
        cov = (~np.isnan(v)).sum()
        print(f"     {s:25s}  coverage={cov}/{N} ({100*cov/N:.1f}%)")

    # ── S8 Dawid-Skene EM ──────────────────────────────────────────────
    z, sigma2 = dawid_skene_em(rn, tol=args.em_tol, max_iter=args.em_max_iter)

    print(f"\n[s8] per-source noise σ²:")
    inv_var = {s: 1.0 / s2 for s, s2 in sigma2.items()}
    total = sum(inv_var.values())
    print(f"     {'source':25s}  {'σ²':>8s}  {'reliability (1/σ²)':>20s}  {'normalized':>12s}")
    for s in sigma2:
        print(f"     {s:25s}  {sigma2[s]:>8.4f}  {inv_var[s]:>20.4f}  "
              f"{inv_var[s]/total:>12.3f}")

    print(f"\n[consensus] z stats: mean={z.mean():.3f}  std={z.std():.3f}  "
          f"min={z.min():.3f}  max={z.max():.3f}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    # Build a coverage mask so consumers don't have to recompute it from
    # the raw rank-norm matrix (spec § 14 mentions reliabilities + sources;
    # we add coverage as a convenience).
    coverage = np.stack([(~np.isnan(v)).astype(bool) for v in rn.values()], axis=1)

    np.savez(
        args.out,
        track_ids=np.array(all_tids, dtype=np.int64),
        consensus_intensity=z,
        source_names=np.array(list(rn.keys()), dtype=object),
        # σ² per source (raw EM output; useful for debug)
        source_sigma2=np.array([sigma2[s] for s in rn], dtype=np.float32),
        # Per spec § 14: reliabilities = 1/σ² normalized to sum to 1.
        # Field name `source_reliabilities` matches the spec verbatim.
        source_reliabilities=np.array(
            [inv_var[s] / total for s in rn], dtype=np.float32),
        coverage=coverage,
    )
    print(f"[agg] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
