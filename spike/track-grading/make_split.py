"""Stage S9 — Artist-stratified train/val/test split.

Per spec § 15: split by artist only (G8). Genre stratification is
intentionally NOT enforced — relying on the noisy `source_category` would
re-introduce the trust we removed in G7. With ~40 k tracks and the law of
large numbers, genre balance handles itself; per-cluster eval (S12)
diagnoses any genre-biased gaps via caption-emb K-means.

Asserts: 0 artists shared between train/test, 0 between val/test.

Usage:
    bash spike/track-grading/run_r7_step.sh make_split.py \\
         --corpus /home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json \\
         --consensus /home/data01/Music/mesh-track-grading/round7_6_consensus.npz \\
         --out /home/data01/Music/mesh-track-grading/round7_6_split.npz
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--corpus", type=Path, required=True)
    p.add_argument("--consensus", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--train-frac", type=float, default=0.80)
    p.add_argument("--val-frac",   type=float, default=0.10)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def main(args) -> int:
    # ── Load corpus + consensus to know which tracks have labels ──────
    z = np.load(args.consensus, allow_pickle=True)
    labelled_tids = set(int(t) for t in z["track_ids"])

    tracks = json.loads(args.corpus.read_text())
    tid_to_artist: dict[int, str] = {}
    for t in tracks:
        tid = int(t.get("deezer_track_id"))
        if tid in labelled_tids:
            tid_to_artist[tid] = (t.get("artist") or "Unknown").strip()
    print(f"[split] {len(tid_to_artist)} labelled tracks, "
          f"{len(set(tid_to_artist.values()))} unique artists")

    # ── Group tracks by artist, then bucket artists by track count ────
    artist_to_tids: dict[str, list[int]] = defaultdict(list)
    for tid, artist in tid_to_artist.items():
        artist_to_tids[artist].append(tid)

    # Shuffle artists deterministically
    rng = np.random.default_rng(args.seed)
    artists = sorted(artist_to_tids.keys())
    rng.shuffle(artists)

    # ── Allocate artists → splits to hit track-count fractions ────────
    target_train = args.train_frac * len(tid_to_artist)
    target_val   = args.val_frac   * len(tid_to_artist)
    n_train = n_val = 0
    split_of_artist: dict[str, str] = {}
    for a in artists:
        n = len(artist_to_tids[a])
        if n_train < target_train:
            split_of_artist[a] = "train"; n_train += n
        elif n_val < target_val:
            split_of_artist[a] = "val"; n_val += n
        else:
            split_of_artist[a] = "test"

    # ── Materialize per-track split ───────────────────────────────────
    track_ids = sorted(tid_to_artist.keys())
    splits = np.array([split_of_artist[tid_to_artist[tid]] for tid in track_ids],
                      dtype=object)
    artists_arr = np.array([tid_to_artist[tid] for tid in track_ids], dtype=object)

    # ── Sanity assertions ─────────────────────────────────────────────
    train_artists = set(a for a, s in split_of_artist.items() if s == "train")
    val_artists   = set(a for a, s in split_of_artist.items() if s == "val")
    test_artists  = set(a for a, s in split_of_artist.items() if s == "test")
    assert not (train_artists & test_artists), "train ↔ test artist leak!"
    assert not (val_artists & test_artists),   "val ↔ test artist leak!"
    assert not (train_artists & val_artists),  "train ↔ val artist leak!"

    n_total = len(track_ids)
    print(f"[split] track-level distribution:")
    for s in ("train", "val", "test"):
        n_s = (splits == s).sum()
        n_a = sum(1 for v in split_of_artist.values() if v == s)
        print(f"   {s:5s}: {n_s:>6d} tracks ({100*n_s/n_total:5.1f}%)  "
              f"{n_a:>5d} artists")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(
        args.out,
        track_ids=np.array(track_ids, dtype=np.int64),
        split=splits,
        artist=artists_arr,
    )
    print(f"[split] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
