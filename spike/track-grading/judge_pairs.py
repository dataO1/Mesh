"""LLM-as-judge pairwise intensity comparator using Audio Flamingo 3.

Sampling: anchored tournament. Pick K=3 anchor tracks evenly spaced in the
V11 axis ranking. Compare every other track against all 3 anchors. Each
track gets a (wins_vs_anchor_low, wins_vs_anchor_mid, wins_vs_anchor_high)
triple. Plus N random extra pairs for Bradley-Terry refinement.

For each pair we ask AF3:
  Listen to A, then B. Which is more intense (harsher / more aggressive)?
  Reply A | B | EQUAL.

Ranking derivation (two methods, both reported):
  1. Anchor-weighted: for each track, score = sum over anchors of
     (anchor_intensity_prior × win_indicator). Maps cleanly back to 0-10.
  2. Bradley-Terry MLE on all pair judgments. Produces relative strengths;
     z-normalize then map to 0-10.

Resumable via /tmp/track-grading/pairs/<a>_<b>.json cache.

Run:
  python judge_pairs.py [--limit-tracks N] [--extra-random-pairs M]
                        [--out-dir DIR]
"""
from __future__ import annotations

import argparse
import csv
import json
import re
import sys
import time
from pathlib import Path

import numpy as np
import torch
import soundfile as sf
import librosa


CLIP_SECS = 30
TARGET_SR = 16_000
MODEL_NAME = "nvidia/audio-flamingo-3-hf"


# Anchors picked from V11 ranking after seeing round-3 LLM single-call results.
# We deliberately pick tracks with strong, OBVIOUS intensity character so the
# LLM's pairwise judgment has clear targets:
#   LOW  : ZHU "Faded" — deep house/vocal-pop, model already scored it low (4-5)
#   MID  : Stephan Bodzin "Strand" — melodic techno, model scored 5
#   HIGH : Hyper "FCKD" — neuro DnB, model scored 7-8
#
# We could also cycle multiple anchor sets and average — but start simple.
ANCHOR_TRACK_TITLES = [
    ("low",  "Faded"),       # ZHU
    ("mid",  "Strand"),      # Bodzin
    ("high", "FCKD"),        # Hyper
]

ANCHOR_PRIORS = {"low": 3.0, "mid": 5.0, "high": 8.5}


# AF3 enforces strict 1:1 text-to-audio in its processor — no way around it
# with the chat-template API. So we concatenate clip A + 2s of silence + clip B
# into a single audio input and use ONE prompt that describes the layout.
SEPARATOR_SECS = 2.0  # silence between clips so the model perceives a boundary

PROMPT = """\
You will hear ONE audio recording that contains TWO 30-second clips back-to-back, separated by a brief silence.

Clip A: 0:00 to 0:30
(silence: 0:30 to 0:32)
Clip B: 0:32 to 1:02

Decide which clip is MORE INTENSE (harsher, more distorted, more aggressive, more energy-dense). Don't compare for tempo alone — focus on harshness, distortion, density, weight.

Reply with EXACTLY one token on a single line:
  A     — clip A is more intense
  B     — clip B is more intense
  EQUAL — about equally intense

Then on a second line, one short justification (≤20 words).
"""


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--collection", type=Path,
                   default=Path.home() / "Music" / "mesh-collection")
    p.add_argument("--out-dir", type=Path, default=Path("/tmp/track-grading"))
    p.add_argument("--limit-tracks", type=int, default=None,
                   help="Limit how many non-anchor tracks to compare")
    p.add_argument("--extra-random-pairs", type=int, default=0,
                   help="Add N random extra pairs for BT refinement")
    p.add_argument("--smoke-mode", action="store_true",
                   help="Compare 5 known-ordering pairs as a sanity check")
    return p.parse_args()


def load_track_meta(out_dir: Path) -> dict:
    """Return dict keyed by track_id with path/title/artist/drop_marker."""
    p = out_dir / "_track-list.csv"
    if not p.exists():
        sys.exit(f"missing {p} — run dump_track_list first")
    out = {}
    with p.open() as f:
        for r in csv.DictReader(f):
            tid = int(r["track_id"])
            out[tid] = {
                "path": r["path"],
                "title": r["title"],
                "artist": r["artist"],
                "drop_marker": int(r["drop_marker"]) if r["drop_marker"] else None,
            }
    return out


def find_anchor_ids(meta: dict) -> list[tuple[str, int]]:
    out = []
    for level, needle in ANCHOR_TRACK_TITLES:
        for tid, info in meta.items():
            if needle in info["title"]:
                out.append((level, tid))
                break
    return out


def load_audio_window(info: dict) -> np.ndarray | None:
    path = info["path"]
    try:
        sf_info = sf.info(path)
    except Exception as e:
        print(f"sf.info failed: {path}: {e}", file=sys.stderr)
        return None
    native_sr = sf_info.samplerate
    total_frames = sf_info.frames
    drop_marker = info.get("drop_marker")
    if drop_marker is not None and drop_marker > 0:
        start_native = max(0, drop_marker - native_sr * 5)
    else:
        start_native = total_frames // 3
    duration = native_sr * CLIP_SECS
    start_native = min(start_native, max(0, total_frames - duration))
    try:
        audio, sr = sf.read(path, start=start_native, frames=duration,
                            dtype="float32", always_2d=False)
    except Exception as e:
        print(f"sf.read failed: {path}: {e}", file=sys.stderr)
        return None
    if audio.ndim > 1:
        audio = audio.mean(axis=1)
    if sr != TARGET_SR:
        audio = librosa.resample(audio, orig_sr=sr, target_sr=TARGET_SR,
                                 res_type="soxr_hq")
    target_len = TARGET_SR * CLIP_SECS
    if len(audio) < target_len:
        audio = np.pad(audio, (0, target_len - len(audio)))
    elif len(audio) > target_len:
        audio = audio[:target_len]
    return audio.astype(np.float32)


def parse_choice(text: str) -> str | None:
    """Return 'A', 'B', or 'EQUAL' or None."""
    if not text:
        return None
    # First token of stripped first line, uppercased
    first_line = text.strip().split("\n")[0].strip()
    # Strip leading punctuation/whitespace
    m = re.match(r"^[\s\W]*([A-Z]+|equal|EQUAL)", first_line, re.IGNORECASE)
    if not m:
        return None
    tok = m.group(1).upper()
    if tok.startswith("A") and not tok.startswith("EQ"):
        return "A"
    if tok.startswith("B"):
        return "B"
    if tok.startswith("EQ"):
        return "EQUAL"
    return None


def pair_cache_path(out_dir: Path, a: int, b: int) -> Path:
    """One file per DIRECTED pair (a vs b ≠ b vs a) — we run both orders
    to cancel positional bias, see judge_one_pair_bilateral."""
    pdir = out_dir / "pairs"
    pdir.mkdir(parents=True, exist_ok=True)
    return pdir / f"{a}_vs_{b}.json"


def aggregated_cache_path(out_dir: Path, a: int, b: int) -> Path:
    """Bilateral aggregation cache (uses sorted ids — direction-independent)."""
    lo, hi = sorted((a, b))
    return out_dir / "pairs_agg" / f"{lo}_{hi}.json"


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    meta = load_track_meta(args.out_dir)
    print(f"[judge] loaded {len(meta)} tracks from track list")

    anchors = find_anchor_ids(meta)
    if len(anchors) != 3:
        print(f"[judge] anchor lookup failed: only matched {[a[0] for a in anchors]}", file=sys.stderr)
        sys.exit(1)
    anchor_ids = {level: tid for level, tid in anchors}
    print(f"[judge] anchors: " + ", ".join(
        f"{lev}={meta[tid]['title']} ({meta[tid]['artist']})" for lev, tid in anchors
    ))

    # Build pair list. Each (a,b) gets enqueued in BOTH orders to cancel
    # AF3's positional bias (smoke test showed it always picks "A"). We
    # aggregate the two judgments per pair downstream.
    pairs: list[tuple[int, int, str]] = []  # (a, b, reason)
    if args.smoke_mode:
        # 4 hand-picked unordered pairs (we'll judge each in both directions)
        SMOKE = [
            ("FCKD", "Faded",       "expect Hyper > ZHU"),
            ("How You Move", "Butternuts", "expect Charlotte > liquid"),
            ("Strand", "Faded",     "uncertain — both mid/low"),
            ("FCKD", "Strand",      "expect Hyper > Bodzin"),
            ("Omnivore", "Slinkystink",  "expect Noisia > Random Movement"),
        ]
        for needle_a, needle_b, reason in SMOKE:
            ta = next((tid for tid, i in meta.items() if needle_a in i["title"]), None)
            tb = next((tid for tid, i in meta.items() if needle_b in i["title"]), None)
            if ta and tb:
                pairs.append((ta, tb, reason))
                pairs.append((tb, ta, reason + " (rev)"))
        print(f"[judge] smoke mode: {len(pairs)} directed pairs queued (5 unordered × 2 orders)")
    else:
        candidate_ids = [tid for tid in meta if tid not in anchor_ids.values()]
        if args.limit_tracks:
            candidate_ids = candidate_ids[: args.limit_tracks]
        for tid in candidate_ids:
            for level, anchor_tid in anchor_ids.items():
                # Both orders to cancel positional bias
                pairs.append((tid, anchor_tid, f"vs_anchor_{level}"))
                pairs.append((anchor_tid, tid, f"vs_anchor_{level}_rev"))
        if args.extra_random_pairs:
            import random
            rng = random.Random(42)
            for _ in range(args.extra_random_pairs):
                a, b = rng.sample(candidate_ids, 2)
                pairs.append((a, b, "random_extra"))
                pairs.append((b, a, "random_extra_rev"))
        print(f"[judge] tournament mode: {len(pairs)} directed pairs queued "
              f"({len(candidate_ids)} candidates × {len(anchor_ids)} anchors × 2 orders "
              f"+ {args.extra_random_pairs * 2} random)")

    # Resume filter
    pending = []
    for a, b, reason in pairs:
        if pair_cache_path(args.out_dir, a, b).exists():
            continue
        pending.append((a, b, reason))
    print(f"[judge] {len(pending)} pairs pending after resume filter")

    if not pending:
        print("[judge] nothing to do.")
        return 0

    # Lazy model load
    print(f"[judge] loading {MODEL_NAME}...")
    t0 = time.time()
    from transformers import AutoProcessor
    try:
        from transformers import AudioFlamingo3ForConditionalGeneration as AF3Model
    except ImportError:
        from transformers import AutoModel as AF3Model
    processor = AutoProcessor.from_pretrained(MODEL_NAME, trust_remote_code=True)
    model = AF3Model.from_pretrained(
        MODEL_NAME, trust_remote_code=True,
        torch_dtype=torch.bfloat16, device_map="cuda",
    ).eval()
    print(f"[judge] model loaded in {time.time() - t0:.1f}s")

    n_done = 0
    n_failed = 0
    start = time.time()
    for a, b, reason in pending:
        cache = pair_cache_path(args.out_dir, a, b)
        track_t0 = time.time()
        audio_a = load_audio_window(meta[a])
        audio_b = load_audio_window(meta[b])
        if audio_a is None or audio_b is None:
            print(f"[judge] {a} vs {b}: audio decode failed", file=sys.stderr)
            n_failed += 1
            n_done += 1
            continue

        try:
            # Concatenate A + silence + B into a single audio (AF3's processor
            # rejects multi-audio in one prompt — see investigation).
            sep = np.zeros(int(TARGET_SR * SEPARATOR_SECS), dtype=np.float32)
            audio_concat = np.concatenate([audio_a, sep, audio_b])
            messages = [
                {"role": "user", "content": [
                    {"type": "text", "text": PROMPT},
                    {"type": "audio", "audio": audio_concat},
                ]},
            ]
            inputs = processor.apply_chat_template(
                messages, add_generation_prompt=True, tokenize=True,
                return_tensors="pt", return_dict=True,
            ).to("cuda")
            if "input_features" in inputs:
                inputs["input_features"] = inputs["input_features"].to(torch.bfloat16)
            with torch.no_grad():
                out_ids = model.generate(
                    **inputs, max_new_tokens=80, do_sample=True,
                    temperature=0.3, top_p=0.9,
                    pad_token_id=processor.tokenizer.eos_token_id,
                )
            generated = out_ids[0, inputs["input_ids"].shape[1]:]
            response_text = processor.tokenizer.decode(generated, skip_special_tokens=True)
        except Exception as e:
            print(f"[judge] {a} vs {b}: inference failed — {e}", file=sys.stderr)
            n_failed += 1
            n_done += 1
            continue

        choice = parse_choice(response_text)
        # Normalize to "winner_track_id" so cache file isn't direction-dependent
        lo, hi = sorted((a, b))
        if choice == "A":
            winner_id = a
        elif choice == "B":
            winner_id = b
        else:
            winner_id = None

        record = {
            "pair": [lo, hi],
            "presented_a": a,
            "presented_b": b,
            "reason": reason,
            "model": MODEL_NAME,
            "raw_response": response_text,
            "choice": choice,            # "A" | "B" | "EQUAL" | None
            "winner_id": winner_id,      # actual track ID that won, or None for equal/parse-fail
            "wall_time_s": round(time.time() - track_t0, 2),
            "ts": int(time.time()),
        }
        cache.write_text(json.dumps(record, indent=2))
        n_done += 1

        if n_done % 20 == 0 or n_done == len(pending):
            elapsed = time.time() - start
            rate = n_done / max(elapsed, 0.001)
            eta = (len(pending) - n_done) / max(rate, 0.001)
            print(f"[judge] {n_done}/{len(pending)} done "
                  f"({rate:.2f}/s, eta {eta/60:.1f}min, failed {n_failed}, "
                  f"last={a}vs{b} → {choice})")

    print(f"[judge] complete: {n_done - n_failed} succeeded, {n_failed} failed, "
          f"{(time.time() - start)/60:.1f}min total")
    return 0


if __name__ == "__main__":
    sys.exit(main())
