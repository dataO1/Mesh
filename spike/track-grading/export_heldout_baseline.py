"""Export V18.1 intensity scores on the held-out 3985-track test set.

Round-7.7 baseline pinning. Generates an Obsidian-friendly markdown table
with V18.1 score + consensus score + per-track metadata for every track
in the round-7.6 held-out test split. Counterpart to the user-library
baseline at "Mesh — V18.1 Library Baseline.md", but for cross-genre
data (Deezer corpus across ~2 100 genre seeds, not DnB-only).

Pinned BEFORE replacing the V18.1 deployed axis. After round-7.7 V18.X
ships, regenerate the same table on V18.X and join the two by track_id
to compute per-track score deltas + held-out PA delta.

Usage:
  bash spike/track-grading/run_r7_step.sh export_heldout_baseline.py \\
       --out "/path/to/Mesh — V18.1 Held-Out Baseline.md"
"""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from math import erf, sqrt
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--axis", type=Path,
                   default=Path("/home/data01/Projects/Mesh/models/muq-mulan-aggression-axis.json"))
    p.add_argument("--audio-emb", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz"))
    p.add_argument("--audio-emb-key", default="embeddings",
                   help="which audio head to use. For the V18.1 baseline this MUST be 'embeddings' "
                        "(the 512-d joint-space, the substrate V18.1 was trained on).")
    p.add_argument("--split", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_split.npz"))
    p.add_argument("--consensus", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_consensus.npz"))
    p.add_argument("--manifest", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json"))
    p.add_argument("--out", type=Path, required=True,
                   help="Markdown output path (Obsidian vault).")
    p.add_argument("--limit", type=int, default=None, help="(debug) only first N held-out tracks")
    return p.parse_args()


def gelu(x: np.ndarray) -> np.ndarray:
    """Match torch.nn.GELU default ('none'), used by V18.1 MLP."""
    return 0.5 * x * (1.0 + np.vectorize(erf)(x / sqrt(2)))


def project_v18(axis_json: dict, audio_arr: np.ndarray) -> np.ndarray:
    """Project (N, in_dim) audio embeddings through the loaded V18 MLP/linear axis."""
    if "mlp" in axis_json and isinstance(axis_json["mlp"], dict):
        mlp = axis_json["mlp"]
        W1 = np.asarray(mlp["W1"], dtype=np.float32)  # (hidden, in_dim)
        b1 = np.asarray(mlp["b1"], dtype=np.float32)
        W2 = np.asarray(mlp["W2"], dtype=np.float32)  # (1, hidden)
        b2 = float(mlp["b2"])
        if audio_arr.shape[1] != W1.shape[1]:
            sys.exit(f"[heldout] audio dim {audio_arr.shape[1]} ≠ axis W1 in_dim {W1.shape[1]}")
        h = audio_arr @ W1.T + b1
        h = gelu(h)
        return (h @ W2.T + b2).squeeze(-1)
    if "intensity_axis_vec" in axis_json:
        W = np.asarray(axis_json["intensity_axis_vec"], dtype=np.float32)
        b = float(axis_json.get("bias", 0.0))
        if audio_arr.shape[1] != W.shape[0]:
            sys.exit(f"[heldout] audio dim {audio_arr.shape[1]} ≠ axis vec dim {W.shape[0]}")
        return audio_arr @ W + b
    sys.exit(f"[heldout] unrecognized axis JSON schema: keys={list(axis_json.keys())[:8]}")


def main(args) -> int:
    print(f"[heldout] loading axis: {args.axis}")
    axis_json = json.loads(args.axis.read_text())
    variant = axis_json.get("version", axis_json.get("variant_id", "unknown"))
    print(f"[heldout] axis variant: {variant}")

    print(f"[heldout] loading audio embeddings: {args.audio_emb}")
    e = np.load(args.audio_emb, allow_pickle=True)
    if args.audio_emb_key not in e.files:
        sys.exit(f"[heldout] NPZ has no '{args.audio_emb_key}' field; fields={list(e.files)}")
    audio_arr = e[args.audio_emb_key].astype(np.float32)
    audio_tids = e["track_ids"].astype(np.int64)
    audio_artists = e["artists"].astype(object) if "artists" in e.files else np.empty(len(audio_tids), dtype=object)
    audio_titles  = e["titles"].astype(object)  if "titles"  in e.files else np.empty(len(audio_tids), dtype=object)
    audio_seeds   = e["genre_seed"].astype(object) if "genre_seed" in e.files else np.empty(len(audio_tids), dtype=object)
    tid_to_i = {int(t): i for i, t in enumerate(audio_tids)}
    print(f"[heldout]   {len(audio_tids)} tracks in audio cache, dim={audio_arr.shape[1]}")

    print(f"[heldout] loading split: {args.split}")
    s = np.load(args.split, allow_pickle=True)
    sp_tids = s["track_ids"].astype(np.int64)
    sp_split = s["split"].astype(object)
    test_tids = [int(t) for t, sp in zip(sp_tids, sp_split) if str(sp) == "test"]
    print(f"[heldout]   {len(test_tids)} held-out test tracks")

    print(f"[heldout] loading consensus: {args.consensus}")
    cs = np.load(args.consensus, allow_pickle=True)
    cs_tids = cs["track_ids"].astype(np.int64)
    cs_arr = cs["consensus_intensity"].astype(np.float32)
    cs_lookup = {int(t): float(c) for t, c in zip(cs_tids, cs_arr)}

    # Manifest provides genre_seed where audio NPZ may be missing it.
    manifest = json.loads(args.manifest.read_text())
    seed_lookup = {int(r["deezer_track_id"]): str(r.get("genre_seed", r.get("category", "")))
                   for r in manifest if r.get("deezer_track_id") is not None}

    # ── Project + collect ──
    test_tids_in_audio = [t for t in test_tids if t in tid_to_i]
    if args.limit is not None:
        test_tids_in_audio = test_tids_in_audio[: args.limit]
    print(f"[heldout]   {len(test_tids_in_audio)} test tracks have audio embeddings (others dropped)")

    test_indices = np.array([tid_to_i[t] for t in test_tids_in_audio], dtype=np.int64)
    test_audio = audio_arr[test_indices]
    print(f"[heldout] projecting {len(test_indices)} tracks through {variant}...")
    scores = project_v18(axis_json, test_audio)

    # ── Build the table ──
    rows = []
    for tid, score in zip(test_tids_in_audio, scores):
        i = tid_to_i[tid]
        rows.append({
            "track_id": tid,
            "score": float(score),
            "consensus": cs_lookup.get(tid, float("nan")),
            "artist": str(audio_artists[i]) if i < len(audio_artists) else "",
            "title": str(audio_titles[i]) if i < len(audio_titles) else "",
            "genre_seed": seed_lookup.get(tid, str(audio_seeds[i]) if i < len(audio_seeds) else ""),
        })
    rows.sort(key=lambda r: -r["score"])  # descending
    n = len(rows)

    # Distribution stats
    arr = np.array([r["score"] for r in rows])
    cons_arr = np.array([r["consensus"] for r in rows if not np.isnan(r["consensus"])])
    pct = lambda q: float(np.quantile(arr, q))

    # ── Pairwise agreement vs consensus (the gold metric) ──
    valid = np.array([not np.isnan(r["consensus"]) for r in rows])
    s_v = np.array([r["score"] for r in rows])[valid]
    y_v = np.array([r["consensus"] for r in rows])[valid]
    nv = len(s_v)
    if nv > 1:
        ds = s_v[:, None] - s_v[None, :]
        dy = y_v[:, None] - y_v[None, :]
        tri = np.triu(np.ones((nv, nv), dtype=bool), k=1)
        valid_pairs = tri & (ds != 0) & (dy != 0)
        pa = float((valid_pairs & ((ds > 0) == (dy > 0))).sum() / max(valid_pairs.sum(), 1))
        # Spearman ρ
        ra = np.argsort(np.argsort(s_v))
        rb = np.argsort(np.argsort(y_v))
        rho = float(1 - 6 * np.sum((ra - rb) ** 2) / (nv * (nv * nv - 1)))
    else:
        pa = float("nan")
        rho = float("nan")

    # ── Write Obsidian markdown ──
    args.out.parent.mkdir(parents=True, exist_ok=True)
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    body = []
    body.append("---")
    body.append("tags: [knowledge-base, mesh, intensity-axis, baseline-export, held-out]")
    body.append(f"created: {today}")
    body.append("status: archival baseline")
    body.append(f"axis_variant: {variant}")
    body.append(f"audio_emb_key: {args.audio_emb_key}")
    body.append(f"audio_emb_dim: {audio_arr.shape[1]}")
    body.append(f"n_tracks: {n}")
    body.append(f"n_with_consensus: {int(valid.sum())}")
    body.append(f"test_pa_vs_consensus: {pa:.4f}")
    body.append(f"test_spearman_vs_consensus: {rho:.4f}")
    body.append("---\n")
    body.append(f"# Held-out cross-genre intensity ranking — {variant}\n")
    body.append("Per-track V18.x intensity projection on the round-7.6 held-out test "
                "set (artist-stratified ~10 % of the 39 913 Deezer corpus, spanning ~2 100 "
                "genre seeds). Counterpart to the DnB-library baseline at "
                "[[Mesh — V18.1 Library Baseline]]. Pinned for round-7.7 before/after "
                "comparison: regenerate after V18.X ships and join by `track_id`.\n")
    body.append(f"**Axis:** `{variant}` ({args.audio_emb_key}, dim={audio_arr.shape[1]})  ")
    body.append(f"**N:** {n} test tracks ({int(valid.sum())} have a consensus label)  ")
    body.append(f"**Distribution:** min `{arr.min():+.4f}` · p25 `{pct(0.25):+.4f}` · "
                f"median `{pct(0.5):+.4f}` · p75 `{pct(0.75):+.4f}` · max `{arr.max():+.4f}` · "
                f"mean ± σ `{arr.mean():+.4f} ± {arr.std():.4f}`  ")
    body.append(f"**Test PA vs consensus:** **{pa:.4f}** · Spearman ρ `{rho:.4f}`\n")
    body.append("(PA is the round-7.6 spec's primary acceptance metric — V18.1's reported "
                "0.8174 should reproduce here within ~1e-4 if the axis JSON + held-out IDs "
                "are intact.)\n")
    body.append("## Full per-track ranking (sorted by V18.x score, descending)\n")
    body.append("| rank | percentile | score | consensus | track_id | artist | title | genre_seed |")
    body.append("|---:|---:|---:|---:|---:|---|---|---|")
    for rank_idx, r in enumerate(rows):
        rank = rank_idx + 1
        pct_rank = 100.0 * (n - rank) / max(n - 1, 1)
        artist = r["artist"].replace("|", "\\|")
        title = r["title"].replace("|", "\\|")
        genre = r["genre_seed"].replace("|", "\\|") if r["genre_seed"] else ""
        cons_str = f"{r['consensus']:.4f}" if not np.isnan(r["consensus"]) else ""
        body.append(f"| {rank} | {pct_rank:.1f}% | {r['score']:+.4f} | {cons_str} | "
                    f"{r['track_id']} | {artist} | {title} | {genre} |")

    args.out.write_text("\n".join(body) + "\n")
    print(f"[heldout] wrote {n} tracks to {args.out}")
    print(f"[heldout] PA = {pa:.4f}, Spearman ρ = {rho:.4f}")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
