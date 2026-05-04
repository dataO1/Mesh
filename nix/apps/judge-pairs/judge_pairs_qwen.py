"""LLM-as-judge pairwise intensity comparator using Qwen2.5-Omni-7B.

Why this exists: AF3's processor enforces 1:1 text-to-audio, so we tried
concatenating two clips into one audio. Result was 100% positional bias
toward "A" — the model can't distinguish "first 30s" from "second 30s"
when they're glued together. See round-3 / round-4 notes for context.

Qwen2.5-Omni natively supports multiple audio elements per chat turn —
each gets its own audio token and the model can attend to them
independently. That's the structural fix the pairwise task needs.

Same anchored-tournament sampling as judge_pairs.py, same Bradley-Terry
post-processing, same per-pair JSON cache for resumability. The only
delta is the model + chat-template invocation.
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
# Qwen2.5-Omni-7B (un-quantized BF16). After the cpatonn Qwen3-Omni-30B
# AWQ build broke (packed MoE format), and the AWQ-7B variant required a
# pandas/datasets dep chain that wouldn't resolve in our Nix env, this is
# the safe path: ~16 GB BF16 weights fit 24 GB with ~7 GB headroom for
# 2× 30s audio + KV cache. Apache-2.0, native multi-audio in chat template.
MODEL_NAME = "Qwen/Qwen2.5-Omni-7B"


ANCHOR_TRACK_TITLES = [
    ("low",  "Faded"),
    ("mid",  "Strand"),
    ("high", "FCKD"),
]
ANCHOR_PRIORS = {"low": 3.0, "mid": 5.0, "high": 8.5}


PROMPT = """\
You hear two short clips from a DJ's track library: clip A first, then clip B.
Which clip is MORE INTENSE — harsher, more distorted, more aggressive, more energy-dense? Don't compare for tempo alone — focus on harshness, distortion, density, weight.

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
    p.add_argument("--limit-tracks", type=int, default=None)
    p.add_argument("--extra-random-pairs", type=int, default=0)
    p.add_argument("--smoke-mode", action="store_true")
    return p.parse_args()


def load_track_meta(out_dir: Path) -> dict:
    p = out_dir / "_track-list.csv"
    if not p.exists():
        sys.exit(f"missing {p}")
    out = {}
    with p.open() as f:
        for r in csv.DictReader(f):
            tid = int(r["track_id"])
            out[tid] = {
                "path": r["path"], "title": r["title"], "artist": r["artist"],
                "drop_marker": int(r["drop_marker"]) if r["drop_marker"] else None,
            }
    return out


def find_anchor_ids(meta: dict) -> list[tuple[str, int]]:
    out = []
    for level, needle in ANCHOR_TRACK_TITLES:
        for tid, info in meta.items():
            if needle in info["title"]:
                out.append((level, tid)); break
    return out


def load_audio_window(info: dict) -> np.ndarray | None:
    path = info["path"]
    try:
        sf_info = sf.info(path)
    except Exception as e:
        print(f"sf.info failed: {path}: {e}", file=sys.stderr); return None
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
        print(f"sf.read failed: {path}: {e}", file=sys.stderr); return None
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
    if not text: return None
    first_line = text.strip().split("\n")[0].strip()
    m = re.match(r"^[\s\W]*([A-Z]+|equal|EQUAL)", first_line, re.IGNORECASE)
    if not m: return None
    tok = m.group(1).upper()
    if tok.startswith("A") and not tok.startswith("EQ"): return "A"
    if tok.startswith("B"): return "B"
    if tok.startswith("EQ"): return "EQUAL"
    return None


def pair_cache_path(out_dir: Path, a: int, b: int) -> Path:
    pdir = out_dir / "pairs_qwen"
    pdir.mkdir(parents=True, exist_ok=True)
    return pdir / f"{a}_vs_{b}.json"


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    meta = load_track_meta(args.out_dir)
    anchors = find_anchor_ids(meta)
    if len(anchors) != 3:
        print(f"[judge-qwen] only matched: {[a[0] for a in anchors]}", file=sys.stderr)
        sys.exit(1)
    anchor_ids = {level: tid for level, tid in anchors}
    print(f"[judge-qwen] anchors: " + ", ".join(
        f"{lev}={meta[tid]['title']}" for lev, tid in anchors))

    pairs: list[tuple[int, int, str]] = []
    if args.smoke_mode:
        SMOKE = [
            ("FCKD", "Faded", "expect Hyper > ZHU"),
            ("How You Move", "Butternuts", "expect Charlotte > liquid"),
            ("Strand", "Faded", "uncertain — both mid/low"),
            ("FCKD", "Strand", "expect Hyper > Bodzin"),
            ("Omnivore", "Slinkystink", "expect Noisia > Random Movement"),
        ]
        for needle_a, needle_b, reason in SMOKE:
            ta = next((tid for tid, i in meta.items() if needle_a in i["title"]), None)
            tb = next((tid for tid, i in meta.items() if needle_b in i["title"]), None)
            if ta and tb:
                pairs.append((ta, tb, reason))
                pairs.append((tb, ta, reason + " (rev)"))
        print(f"[judge-qwen] smoke mode: {len(pairs)} directed pairs")
    else:
        candidate_ids = [tid for tid in meta if tid not in anchor_ids.values()]
        if args.limit_tracks:
            candidate_ids = candidate_ids[: args.limit_tracks]
        for tid in candidate_ids:
            for level, anchor_tid in anchor_ids.items():
                pairs.append((tid, anchor_tid, f"vs_anchor_{level}"))
                pairs.append((anchor_tid, tid, f"vs_anchor_{level}_rev"))
        if args.extra_random_pairs:
            import random
            rng = random.Random(42)
            for _ in range(args.extra_random_pairs):
                a, b = rng.sample(candidate_ids, 2)
                pairs.append((a, b, "random_extra"))
                pairs.append((b, a, "random_extra_rev"))
        print(f"[judge-qwen] tournament: {len(pairs)} directed pairs")

    pending = [(a, b, r) for a, b, r in pairs
               if not pair_cache_path(args.out_dir, a, b).exists()]
    print(f"[judge-qwen] {len(pending)} pending after resume filter")

    if not pending:
        print("[judge-qwen] nothing to do."); return 0

    print(f"[judge-qwen] loading {MODEL_NAME}...")
    t0 = time.time()
    from transformers import AutoProcessor
    try:
        from transformers import Qwen2_5OmniForConditionalGeneration as QwenOmni
    except ImportError:
        from transformers import AutoModelForCausalLM as QwenOmni
    processor = AutoProcessor.from_pretrained(MODEL_NAME, trust_remote_code=True)
    model = QwenOmni.from_pretrained(
        MODEL_NAME, trust_remote_code=True,
        torch_dtype=torch.bfloat16, device_map="cuda",
    ).eval()
    print(f"[judge-qwen] model loaded in {time.time() - t0:.1f}s")

    n_done = 0; n_failed = 0
    start = time.time()
    for a, b, reason in pending:
        cache = pair_cache_path(args.out_dir, a, b)
        track_t0 = time.time()
        audio_a = load_audio_window(meta[a])
        audio_b = load_audio_window(meta[b])
        if audio_a is None or audio_b is None:
            print(f"[judge-qwen] {a} vs {b}: audio decode failed", file=sys.stderr)
            n_failed += 1; n_done += 1; continue

        try:
            # Qwen3-Omni multi-audio chat format (each audio is a separate
            # content element; the processor attends to them independently —
            # this is the structural fix vs AF3's concat-audio failure mode).
            messages = [
                {"role": "system", "content": [
                    {"type": "text", "text": "You are an expert music analyst."},
                ]},
                {"role": "user", "content": [
                    {"type": "audio", "audio": audio_a},
                    {"type": "audio", "audio": audio_b},
                    {"type": "text", "text": PROMPT},
                ]},
            ]
            text_prompt = processor.apply_chat_template(
                messages, add_generation_prompt=True, tokenize=False,
            )
            inputs = processor(
                text=text_prompt,
                audio=[audio_a, audio_b],
                sampling_rate=TARGET_SR,
                return_tensors="pt", padding=True,
            ).to("cuda")
            with torch.no_grad():
                out_ids = model.generate(
                    **inputs, max_new_tokens=80,
                    do_sample=False, temperature=0.0,
                    pad_token_id=processor.tokenizer.eos_token_id,
                )
            generated = out_ids[0, inputs["input_ids"].shape[1]:]
            response_text = processor.tokenizer.decode(generated, skip_special_tokens=True)
        except Exception as e:
            print(f"[judge-qwen] {a} vs {b}: inference failed — {e}", file=sys.stderr)
            n_failed += 1; n_done += 1; continue

        choice = parse_choice(response_text)
        winner_id = a if choice == "A" else (b if choice == "B" else None)
        record = {
            "pair": [min(a,b), max(a,b)], "presented_a": a, "presented_b": b,
            "reason": reason, "model": MODEL_NAME,
            "raw_response": response_text, "choice": choice,
            "winner_id": winner_id,
            "wall_time_s": round(time.time() - track_t0, 2),
            "ts": int(time.time()),
        }
        cache.write_text(json.dumps(record, indent=2))
        n_done += 1
        if n_done % 20 == 0 or n_done == len(pending):
            elapsed = time.time() - start
            rate = n_done / max(elapsed, 0.001)
            eta = (len(pending) - n_done) / max(rate, 0.001)
            print(f"[judge-qwen] {n_done}/{len(pending)} ({rate:.2f}/s, "
                  f"eta {eta/60:.1f}min, failed {n_failed}, last={a}vs{b}→{choice})")

    print(f"[judge-qwen] complete: {n_done - n_failed} ok, {n_failed} fail, "
          f"{(time.time() - start)/60:.1f}min")
    return 0


if __name__ == "__main__":
    sys.exit(main())
