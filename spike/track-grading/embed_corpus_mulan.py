"""Embed all round-7 corpus MP3s into 512-d MuQ-MuLan vectors.

Reads:  /home/data01/Music/mesh-track-grading/audio/dz_<deezer_track_id>.mp3
Writes: /home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz
        with arrays:
          - track_ids : int64   [N]
          - embeddings: float32 [N, 512]   (L2-normalised by the model)
          - artists   : object  [N]   (from corpus_tracks.json)
          - titles    : object  [N]
          - genre_seed: object  [N]   (which everynoise genre seeded the track)

Uses the PyTorch reference path (`MuQMuLan.from_pretrained(...)`), which
automatically splits clips longer than `clip_secs=10` and averages — perfect
for the 30 s Deezer previews. Runs on GPU if available, batches by audio
length.

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/embed_corpus_mulan.py
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/embed_corpus_mulan.py --limit 100
"""
from __future__ import annotations

import argparse
import contextlib
import json
import os
import sys
import time
from pathlib import Path


@contextlib.contextmanager
def _silence_c_stderr():
    """Redirect process-level stderr fd to /dev/null around mp3 decode.

    libsndfile/libmpg123 emits ID3v2 warnings to stderr from C, bypassing
    Python's `warnings`/`logging` stacks. Deezer previews trip this on
    nearly every file. Silencer is the same shape as
    spike/track-grading/run_judge_pointwise.py.
    """
    saved = os.dup(2)
    devnull_fd = os.open(os.devnull, os.O_WRONLY)
    try:
        os.dup2(devnull_fd, 2)
        yield
    finally:
        os.dup2(saved, 2)
        os.close(devnull_fd)
        os.close(saved)

# LD_LIBRARY_PATH must be set BEFORE python starts (the dynamic loader
# caches it at process startup, so setting it here is too late). The
# wrapper script `run_embed.sh` does the env prep — invoke this via that.
# Direct `python embed_corpus_mulan.py` invocation will fall back to CPU
# unless the caller has already exported the right LD_LIBRARY_PATH.

import numpy as np
import torch
import librosa
from muq import MuQMuLan


SAMPLE_RATE = 24_000
PREVIEW_SECS = 30  # Deezer preview length
CLIP_SECS = 10     # MuQ-MuLan internal clip length


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/audio"))
    p.add_argument("--manifest", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json"))
    p.add_argument("--out", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz"))
    p.add_argument("--batch-size", type=int, default=32,
                   help="audio waveforms per forward pass")
    p.add_argument("--limit", type=int, default=None,
                   help="only process first N tracks (for smoke tests)")
    p.add_argument("--device", default="cuda" if torch.cuda.is_available() else "cpu")
    p.add_argument("--dtype", default="float16",
                   choices=["float16", "float32"],
                   help="MuQ inference dtype on GPU")
    p.add_argument("--no-resume", action="store_true",
                   help="re-encode every track even if already present in --out")
    return p.parse_args()


def load_manifest_index(manifest_path: Path) -> dict[int, dict]:
    """track_id → {artist, title, genre_seed}."""
    rows = json.loads(manifest_path.read_text())
    out = {}
    for r in rows:
        tid = r.get("deezer_track_id")
        if tid is None:
            continue
        out[int(tid)] = {
            "artist": r.get("artist", ""),
            "title": r.get("title", ""),
            "genre_seed": r.get("genre_seed", r.get("category", "")),
        }
    return out


def load_audio(path: Path) -> np.ndarray | None:
    """Load MP3 as 30 s × 24 kHz mono float32. None on failure."""
    try:
        with _silence_c_stderr():
            wav, _ = librosa.load(str(path), sr=SAMPLE_RATE, mono=True,
                                  duration=PREVIEW_SECS)
    except Exception as e:
        print(f"[load] {path.name}: {e}", file=sys.stderr)
        return None
    target = SAMPLE_RATE * PREVIEW_SECS
    if len(wav) < target:
        wav = np.pad(wav, (0, target - len(wav)))
    elif len(wav) > target:
        wav = wav[:target]
    return wav.astype(np.float32)


def main() -> int:
    args = parse_args()
    if not args.manifest.exists():
        sys.exit(f"missing {args.manifest}")
    if not args.audio_dir.exists():
        sys.exit(f"missing {args.audio_dir}")

    meta = load_manifest_index(args.manifest)
    print(f"[embed] manifest: {len(meta)} tracks")

    files = sorted(args.audio_dir.glob("dz_*.mp3"))
    if args.limit:
        files = files[: args.limit]
    print(f"[embed] audio files: {len(files)}")

    # Filter to files with manifest entries.
    work = []
    for f in files:
        try:
            tid = int(f.stem.removeprefix("dz_"))
        except ValueError:
            continue
        if tid in meta:
            work.append((tid, f))
    print(f"[embed] aligned with manifest: {len(work)}")

    # Resume from existing NPZ: skip track_ids already encoded. Atomic
    # append-and-merge below preserves the prior rows, so the existing
    # NPZ stays intact even on partial completion.
    prior_ids: dict[int, np.ndarray] = {}
    prior_artists: dict[int, str] = {}
    prior_titles: dict[int, str] = {}
    prior_seeds: dict[int, str] = {}
    if args.out.exists() and not args.no_resume:
        try:
            z = np.load(args.out, allow_pickle=True)
            p_tids = z["track_ids"].astype(np.int64)
            p_emb = z["embeddings"].astype(np.float32)
            p_art = z["artists"].astype(object) if "artists" in z.files else \
                    np.empty(len(p_tids), dtype=object)
            p_tit = z["titles"].astype(object) if "titles" in z.files else \
                    np.empty(len(p_tids), dtype=object)
            p_sd  = z["genre_seed"].astype(object) if "genre_seed" in z.files else \
                    np.empty(len(p_tids), dtype=object)
            for i, t in enumerate(p_tids):
                tid = int(t)
                prior_ids[tid] = p_emb[i]
                prior_artists[tid] = str(p_art[i]) if i < len(p_art) else ""
                prior_titles[tid] = str(p_tit[i]) if i < len(p_tit) else ""
                prior_seeds[tid] = str(p_sd[i]) if i < len(p_sd) else ""
            print(f"[embed] resume: {len(prior_ids)} embeddings already in {args.out}")
            work = [(tid, f) for tid, f in work if tid not in prior_ids]
            print(f"[embed] {len(work)} tracks pending after resume")
        except Exception as e:
            print(f"[embed] resume read failed ({e}); re-encoding all tracks",
                  file=sys.stderr)
            prior_ids.clear()
            prior_artists.clear(); prior_titles.clear(); prior_seeds.clear()

    if not work:
        print("[embed] nothing to do — every manifest track already encoded")
        return 0

    print(f"[embed] loading MuQ-MuLan on {args.device} ...")
    model = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    model = model.to(args.device).eval()
    if args.device == "cuda" and args.dtype == "float16":
        model = model.half()
    print(f"[embed] model ready ({sum(p.numel() for p in model.parameters())/1e6:.0f}M params)")

    args.out.parent.mkdir(parents=True, exist_ok=True)

    track_ids: list[int] = []
    embeddings: list[np.ndarray] = []
    artists: list[str] = []
    titles: list[str] = []
    seeds: list[str] = []
    n_fail = 0
    t_start = time.time()

    # Process in batches. Each waveform is the same fixed length so we can
    # stack them into a single tensor.
    BATCH = args.batch_size
    for batch_start in range(0, len(work), BATCH):
        batch = work[batch_start: batch_start + BATCH]
        wavs: list[np.ndarray] = []
        b_ids: list[int] = []
        for tid, f in batch:
            w = load_audio(f)
            if w is None:
                n_fail += 1
                continue
            wavs.append(w)
            b_ids.append(tid)
        if not wavs:
            continue
        x = torch.from_numpy(np.stack(wavs)).to(args.device)
        if args.device == "cuda" and args.dtype == "float16":
            x = x.half()
        with torch.no_grad():
            emb = model(wavs=x)
        emb = emb.float().cpu().numpy()
        for tid, e in zip(b_ids, emb):
            track_ids.append(tid)
            embeddings.append(e)
            m = meta[tid]
            artists.append(m["artist"])
            titles.append(m["title"])
            seeds.append(m["genre_seed"])

        done = batch_start + len(batch)
        elapsed = time.time() - t_start
        rate = done / max(elapsed, 0.01)
        eta = (len(work) - done) / max(rate, 0.01)
        if (batch_start // BATCH) % 5 == 0 or done >= len(work):
            print(f"  [{done}/{len(work)}] ok={len(track_ids)} fail={n_fail} "
                  f"({rate:.1f}/s, eta {eta/60:.1f}min)")

    # Merge prior (resumed) rows with newly-encoded rows. Sort by track_id
    # for determinism so v18-train's track-id intersection has a stable
    # order across re-runs.
    merged_ids: list[int] = list(prior_ids.keys()) + track_ids
    merged_emb: list[np.ndarray] = list(prior_ids.values()) + embeddings
    merged_artists = [prior_artists[t] for t in prior_ids.keys()] + artists
    merged_titles = [prior_titles[t] for t in prior_ids.keys()] + titles
    merged_seeds = [prior_seeds[t] for t in prior_ids.keys()] + seeds

    if not merged_ids:
        print("[embed] no embeddings produced — refusing to write empty NPZ")
        return 1

    order = np.argsort(np.array(merged_ids, dtype=np.int64))
    arr_ids = np.array(merged_ids, dtype=np.int64)[order]
    arr_emb = (np.stack(merged_emb).astype(np.float32) if merged_emb
               else np.zeros((0, 512), dtype=np.float32))[order]
    arr_artists = np.array(merged_artists, dtype=object)[order]
    arr_titles = np.array(merged_titles, dtype=object)[order]
    arr_seeds = np.array(merged_seeds, dtype=object)[order]

    n_total = len(arr_ids)
    n_new = len(track_ids)
    n_resumed = n_total - n_new
    print(f"[embed] merging: {n_resumed} resumed + {n_new} new = {n_total} total")

    # Atomic write: stage to <out>.tmp then os.replace. Prevents the
    # "kill mid-save corrupts the NPZ" failure mode that would force a
    # full re-encode. Pass a file handle (not the path) to np.savez so
    # it doesn't auto-append `.npz` and break os.replace.
    tmp_path = args.out.with_suffix(args.out.suffix + ".tmp")
    print(f"[embed] saving {n_total} embeddings → {args.out}")
    with open(tmp_path, "wb") as fh:
        np.savez(fh,
                 track_ids=arr_ids,
                 embeddings=arr_emb,
                 artists=arr_artists,
                 titles=arr_titles,
                 genre_seed=arr_seeds)
    os.replace(tmp_path, args.out)

    norms = np.linalg.norm(arr_emb, axis=1)
    print(f"[embed] L2 norms: min={norms.min():.3f} max={norms.max():.3f} "
          f"mean={norms.mean():.3f}")
    print(f"[embed] this run: {len(track_ids)}/{len(work)} succeeded "
          f"({n_fail} decode failures); "
          f"NPZ now holds {len(arr_ids)} total embeddings")
    print(f"[embed] elapsed: {(time.time()-t_start)/60:.1f}min")
    return 0


if __name__ == "__main__":
    sys.exit(main())
