"""Embed all round-7 corpus MP3s into MuQ-MuLan vectors (multi-output, round-7.7 Phase 1a).

Reads:  /home/data01/Music/mesh-track-grading/audio/dz_<deezer_track_id>.mp3
Writes: /home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz
        with arrays:
          - track_ids       : int64   [N]
          - embeddings      : float32 [N, 512]   (L2-normalised joint-space — for similarity, unchanged)
          - embeddings_1024 : float32 [N, 1024]  (mean-pooled Conformer hidden states — for intensity probe, NEW)
          - artists         : object  [N]
          - titles          : object  [N]
          - genre_seed      : object  [N]

Uses the PyTorch reference path (`MuQMuLan.from_pretrained(...)`). Per-track:
the 30-second waveform is internally split into 3 × 10-second clips; both
heads (1024-d hidden + 512-d latent) are computed per clip and then averaged
across clips to a single (1024,) and (512,) per track — matching the
training distribution that V18.1 was trained on.

The 1024-d head is the new intensity-probe substrate per the round-7.7
research finding (the MuQ paper itself uses 1024-d hidden states for probe
tasks; the 512-d projection is meant for zero-shot text-audio retrieval).
The 512-d head is preserved unchanged for backwards-compatibility with
mesh-cue similarity / clustering / suggestion-graph and any future
text-tower work.

Resume safety: if the prior NPZ lacks the `embeddings_1024` field (i.e.
it was written by the v18.1-era single-output script), the resume is
declined and the corpus is re-encoded from scratch — we need both fields
populated for every track.

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
HIDDEN_DIM = 1024  # MuQ Conformer hidden size (pre-projection)
LATENT_DIM = 512   # MuQ-MuLan joint-space latent (post audio_to_latents)


def encode_dual(model, wavs_batch):
    """Returns (audio_1024, audio_512) for a batch of 30-second waveforms.

    audio_512: averaged 512-d L2-normalized joint-space embedding from the
               model's standard audio path, AVERAGED over the 3 internal
               10-second clips (matches `model(wavs=...)` semantics).
    audio_1024: averaged 1024-d Conformer hidden state, mean-pooled over
               time per clip, then averaged across clips. We do the
               clip-split + per-clip encoder call manually because the
               muq library doesn't expose a get_audio_features-style entry
               point for the pre-projection hidden states.

    Both outputs use the SAME 3-clip averaging scheme as `model(wavs=...)`,
    so the 512-d output here is bit-equivalent to the prior single-output
    `model(wavs=x)` call.
    """
    muq_model = model.mulan.audio.model.model
    encoder = muq_model.encoder
    preproc = muq_model.preprocessor_melspec_2048
    stat = muq_model.stat
    mean_t = stat["melspec_2048_mean"]
    std_t = stat["melspec_2048_std"]

    # Model dtype: matches whatever the caller half'd / kept the model at.
    # The preproc returns fp32 (calls `x.float()` internally per muq lib);
    # we cast back to the model's dtype before feeding the conv stack.
    # `encoder` is a bound method, not a Module — read dtype off the model
    # parameters directly.
    encoder_dtype = next(model.parameters()).dtype

    n_samples_per_clip = SAMPLE_RATE * CLIP_SECS  # 240_000 samples
    total_samples = wavs_batch.shape[1]
    n_clips = max(1, total_samples // n_samples_per_clip)  # typically 3 for a 30 s preview

    with torch.no_grad():
        # 512-d via standard model call (does its own clip split + average internally)
        audio_512 = model(wavs=wavs_batch)

        # 1024-d via manual clip split → encoder hidden → mean-pool → average.
        # Pass clip waveforms through with their input dtype intact (fp16 if
        # the caller half'd the model; fp32 otherwise) — the preprocessor +
        # encoder match the model's dtype, casting here breaks the conv weights.
        per_clip_1024 = []
        for clip_idx in range(n_clips):
            clip_start = clip_idx * n_samples_per_clip
            clip_end = clip_start + n_samples_per_clip
            clip_wavs = wavs_batch[:, clip_start:clip_end]

            mel = preproc(clip_wavs)[..., :-1]
            mel = (mel - mean_t) / std_t
            mel = mel.to(dtype=encoder_dtype)  # match encoder weights (fp16 or fp32)

            _logits, hidden, _new_mask = encoder(mel, is_features_only=True)
            # MuQ returns per-layer hidden states as a tuple; index last layer
            # (matches model config use_layer_idx=-1).
            if isinstance(hidden, (tuple, list)):
                hidden = hidden[-1]
            clip_1024 = hidden.mean(dim=1)  # (B, 1024) — mean-pool over time
            per_clip_1024.append(clip_1024)

        audio_1024 = torch.stack(per_clip_1024, dim=0).mean(dim=0)  # (B, 1024)

    return audio_1024, audio_512


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
    p.add_argument("--lora", type=Path, default=None,
                   help="Path to LoRA adapter dir (e.g. round7_7_lora/epoch_002_lora/) "
                        "to merge before encoding")
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
    #
    # round-7.7 Phase 1a: resume requires BOTH `embeddings` (512-d) AND
    # `embeddings_1024` (1024-d) to be present. NPZs from the v18.1-era
    # single-output script lack the 1024-d field; we decline the resume
    # and re-encode from scratch in that case (we need 1024-d for every
    # track to retrain V18 on the new substrate).
    prior_ids_512: dict[int, np.ndarray] = {}
    prior_ids_1024: dict[int, np.ndarray] = {}
    prior_artists: dict[int, str] = {}
    prior_titles: dict[int, str] = {}
    prior_seeds: dict[int, str] = {}
    if args.out.exists() and not args.no_resume:
        try:
            z = np.load(args.out, allow_pickle=True)
            if "embeddings_1024" not in z.files:
                print(f"[embed] prior NPZ at {args.out} lacks 'embeddings_1024' field "
                      f"(pre-round-7.7 schema). Re-encoding from scratch to populate "
                      f"both heads.", file=sys.stderr)
            else:
                p_tids = z["track_ids"].astype(np.int64)
                p_emb_512 = z["embeddings"].astype(np.float32)
                p_emb_1024 = z["embeddings_1024"].astype(np.float32)
                p_art = z["artists"].astype(object) if "artists" in z.files else \
                        np.empty(len(p_tids), dtype=object)
                p_tit = z["titles"].astype(object) if "titles" in z.files else \
                        np.empty(len(p_tids), dtype=object)
                p_sd  = z["genre_seed"].astype(object) if "genre_seed" in z.files else \
                        np.empty(len(p_tids), dtype=object)
                for i, t in enumerate(p_tids):
                    tid = int(t)
                    prior_ids_512[tid] = p_emb_512[i]
                    prior_ids_1024[tid] = p_emb_1024[i]
                    prior_artists[tid] = str(p_art[i]) if i < len(p_art) else ""
                    prior_titles[tid] = str(p_tit[i]) if i < len(p_tit) else ""
                    prior_seeds[tid] = str(p_sd[i]) if i < len(p_sd) else ""
                print(f"[embed] resume: {len(prior_ids_512)} dual-head embeddings already in {args.out}")
                work = [(tid, f) for tid, f in work if tid not in prior_ids_512]
                print(f"[embed] {len(work)} tracks pending after resume")
        except Exception as e:
            print(f"[embed] resume read failed ({e}); re-encoding all tracks",
                  file=sys.stderr)
            prior_ids_512.clear(); prior_ids_1024.clear()
            prior_artists.clear(); prior_titles.clear(); prior_seeds.clear()

    if not work:
        print("[embed] nothing to do — every manifest track already encoded")
        return 0

    print(f"[embed] loading MuQ-MuLan on {args.device} ...")
    model = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    model = model.to(args.device).eval()

    # Apply LoRA adapters if requested (merge into base weights for inference)
    if args.lora is not None:
        from peft import PeftModel
        print(f"[embed] Loading LoRA adapters from {args.lora} ...", flush=True)
        model = PeftModel.from_pretrained(model, args.lora)
        model = model.to(args.device)
        print("[embed] Merging LoRA into base weights ...", flush=True)
        model = model.merge_and_unload()
        model = model.to(args.device).eval()
        print("[embed] LoRA merged.", flush=True)

    if args.device == "cuda" and args.dtype == "float16":
        model = model.half()
    print(f"[embed] model ready ({sum(p.numel() for p in model.parameters())/1e6:.0f}M params)")

    args.out.parent.mkdir(parents=True, exist_ok=True)

    track_ids: list[int] = []
    embeddings_512: list[np.ndarray] = []
    embeddings_1024: list[np.ndarray] = []
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

        emb_1024_t, emb_512_t = encode_dual(model, x)
        emb_1024 = emb_1024_t.float().cpu().numpy()
        emb_512 = emb_512_t.float().cpu().numpy()

        for tid, e_1024, e_512 in zip(b_ids, emb_1024, emb_512):
            track_ids.append(tid)
            embeddings_1024.append(e_1024)
            embeddings_512.append(e_512)
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
    merged_ids: list[int] = list(prior_ids_512.keys()) + track_ids
    merged_emb_512: list[np.ndarray] = list(prior_ids_512.values()) + embeddings_512
    merged_emb_1024: list[np.ndarray] = list(prior_ids_1024.values()) + embeddings_1024
    merged_artists = [prior_artists[t] for t in prior_ids_512.keys()] + artists
    merged_titles = [prior_titles[t] for t in prior_ids_512.keys()] + titles
    merged_seeds = [prior_seeds[t] for t in prior_ids_512.keys()] + seeds

    if not merged_ids:
        print("[embed] no embeddings produced — refusing to write empty NPZ")
        return 1

    order = np.argsort(np.array(merged_ids, dtype=np.int64))
    arr_ids = np.array(merged_ids, dtype=np.int64)[order]
    arr_emb_512 = (np.stack(merged_emb_512).astype(np.float32) if merged_emb_512
                   else np.zeros((0, LATENT_DIM), dtype=np.float32))[order]
    arr_emb_1024 = (np.stack(merged_emb_1024).astype(np.float32) if merged_emb_1024
                    else np.zeros((0, HIDDEN_DIM), dtype=np.float32))[order]
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
    print(f"[embed] saving {n_total} dual-head embeddings → {args.out}")
    with open(tmp_path, "wb") as fh:
        np.savez(fh,
                 track_ids=arr_ids,
                 embeddings=arr_emb_512,
                 embeddings_1024=arr_emb_1024,
                 artists=arr_artists,
                 titles=arr_titles,
                 genre_seed=arr_seeds)
    os.replace(tmp_path, args.out)

    norms_512 = np.linalg.norm(arr_emb_512, axis=1)
    norms_1024 = np.linalg.norm(arr_emb_1024, axis=1)
    print(f"[embed] L2 norms — 512-d: min={norms_512.min():.3f} max={norms_512.max():.3f} "
          f"mean={norms_512.mean():.3f} (expected ≈ 1.0)")
    print(f"[embed] L2 norms — 1024-d: min={norms_1024.min():.3f} max={norms_1024.max():.3f} "
          f"mean={norms_1024.mean():.3f} (no L2 norm — Conformer hidden raw magnitudes)")
    print(f"[embed] this run: {len(track_ids)}/{len(work)} succeeded "
          f"({n_fail} decode failures); "
          f"NPZ now holds {len(arr_ids)} total embeddings")
    print(f"[embed] elapsed: {(time.time()-t_start)/60:.1f}min")
    return 0


if __name__ == "__main__":
    sys.exit(main())
