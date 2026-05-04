"""LLM-as-judge pairwise intensity comparator via vLLM OpenAI-compat API.

Why this exists: the in-process transformers path for Qwen3-Omni-AWQ broke
on cpatonn's packed-MoE format (see round-3 notes). vLLM has its own
AWQ-marlin loader that handles the packed layout natively, but only when
served via vLLM's own engine. So we serve the model out-of-process via
`spike/track-grading/serve_qwen3_omni.sh` and POST chat completions over
HTTP. Same anchored-tournament sampling, same Bradley-Terry post-processing,
same per-pair JSON cache for resumability — only the inference call changes.

Audio is sent as base64-encoded WAV in `audio_url` content blocks (vLLM's
multimodal chat template understands this for Qwen3-Omni). Two audio blocks
per turn = the structural fix vs AF3's concat-audio failure mode.
"""
from __future__ import annotations

import argparse
import base64
import csv
import io
import json
import os
import re
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import numpy as np
import requests
import soundfile as sf
import librosa


CLIP_SECS = 30
TARGET_SR = 16_000
MODEL_NAME = "cpatonn/Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit"
VLLM_URL = os.environ.get("VLLM_URL", "http://localhost:8000/v1/chat/completions")
REQUEST_TIMEOUT = 180  # seconds — first request triggers CUDA-graph compile


ANCHOR_TRACK_TITLES = [
    ("low",  "Faded"),
    ("mid",  "Strand"),
    ("high", "FCKD"),
]


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
    p.add_argument("--workers", type=int, default=8,
                   help="concurrent inflight requests (vLLM batches them)")
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


def audio_to_b64_wav(audio: np.ndarray) -> str:
    buf = io.BytesIO()
    sf.write(buf, audio, TARGET_SR, format="WAV", subtype="PCM_16")
    return base64.b64encode(buf.getvalue()).decode("ascii")


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
    pdir = out_dir / "pairs_vllm"
    pdir.mkdir(parents=True, exist_ok=True)
    return pdir / f"{a}_vs_{b}.json"


def vllm_judge(audio_a: np.ndarray, audio_b: np.ndarray) -> str:
    """POST to vLLM. Returns generated text (caller parses)."""
    b64_a = audio_to_b64_wav(audio_a)
    b64_b = audio_to_b64_wav(audio_b)
    # vLLM's documented format: input_audio with base64 + format. NOTE: the
    # `uuid` field documented for multi-audio dedup actually breaks two-audio
    # requests in vllm 0.20 (Internal Server Error: assert_never on parsed
    # audio). Order in the content array is what disambiguates clip A vs B —
    # PROMPT references "clip A first, then clip B" so audio_a goes first.
    payload = {
        "model": MODEL_NAME,
        "messages": [
            {"role": "system", "content": "You are an expert music analyst."},
            {"role": "user", "content": [
                {"type": "input_audio",
                 "input_audio": {"data": b64_a, "format": "wav"}},
                {"type": "input_audio",
                 "input_audio": {"data": b64_b, "format": "wav"}},
                {"type": "text", "text": PROMPT},
            ]},
        ],
        "max_tokens": 80,
        "temperature": 0.0,
    }
    r = requests.post(VLLM_URL, json=payload, timeout=REQUEST_TIMEOUT)
    r.raise_for_status()
    return r.json()["choices"][0]["message"]["content"]


def wait_for_endpoint(timeout_s: float = 600) -> bool:
    health = VLLM_URL.rsplit("/v1/", 1)[0] + "/health"
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            r = requests.get(health, timeout=5)
            if r.status_code == 200:
                return True
        except Exception:
            pass
        time.sleep(2)
    return False


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    print(f"[judge-vllm] waiting for {VLLM_URL} ...")
    if not wait_for_endpoint():
        print("[judge-vllm] endpoint never came up; is serve_qwen3_omni.sh running?",
              file=sys.stderr)
        return 1
    print(f"[judge-vllm] endpoint ready")

    meta = load_track_meta(args.out_dir)
    anchors = find_anchor_ids(meta)
    if len(anchors) != 3:
        print(f"[judge-vllm] only matched: {[a[0] for a in anchors]}", file=sys.stderr)
        sys.exit(1)
    anchor_ids = {level: tid for level, tid in anchors}
    print(f"[judge-vllm] anchors: " + ", ".join(
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
        print(f"[judge-vllm] smoke mode: {len(pairs)} directed pairs")
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
        print(f"[judge-vllm] tournament: {len(pairs)} directed pairs")

    pending = [(a, b, r) for a, b, r in pairs
               if not pair_cache_path(args.out_dir, a, b).exists()]
    print(f"[judge-vllm] {len(pending)} pending after resume filter")
    if not pending:
        print("[judge-vllm] nothing to do."); return 0

    # Concurrent inflight: vLLM batches requests in its async engine, so
    # posting N=8 in parallel keeps the GPU busy through the audio-decode
    # gaps. Per-pair JSON cache + atomic write-via-tmpfile means any death
    # mid-run resumes cleanly. Audio decoding runs in worker threads so the
    # GIL doesn't serialise the librosa resamples.
    n_done_lock = threading.Lock()
    state = {"done": 0, "failed": 0, "last_choice": None, "last_a": None, "last_b": None}
    start = time.time()

    def process_one(a: int, b: int, reason: str) -> None:
        cache = pair_cache_path(args.out_dir, a, b)
        track_t0 = time.time()
        audio_a = load_audio_window(meta[a])
        audio_b = load_audio_window(meta[b])
        if audio_a is None or audio_b is None:
            with n_done_lock:
                state["failed"] += 1; state["done"] += 1
                state["last_a"], state["last_b"], state["last_choice"] = a, b, "DECODE_FAIL"
            print(f"[judge-vllm] {a} vs {b}: audio decode failed", file=sys.stderr)
            return
        try:
            response_text = vllm_judge(audio_a, audio_b)
        except Exception as e:
            with n_done_lock:
                state["failed"] += 1; state["done"] += 1
                state["last_a"], state["last_b"], state["last_choice"] = a, b, "INFER_FAIL"
            print(f"[judge-vllm] {a} vs {b}: inference failed — {e}", file=sys.stderr)
            return

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
        # Atomic write via tmp+rename so partial JSON never lands in cache.
        tmp = cache.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(record, indent=2))
        tmp.rename(cache)
        with n_done_lock:
            state["done"] += 1
            state["last_a"], state["last_b"], state["last_choice"] = a, b, choice
            d = state["done"]
            if d % 25 == 0 or d == len(pending):
                elapsed = time.time() - start
                rate = d / max(elapsed, 0.001)
                eta = (len(pending) - d) / max(rate, 0.001)
                print(f"[judge-vllm] {d}/{len(pending)} ({rate:.2f}/s, "
                      f"eta {eta/60:.1f}min, failed {state['failed']}, "
                      f"last={a}vs{b}→{choice})")

    print(f"[judge-vllm] running with {args.workers} concurrent workers")
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = [ex.submit(process_one, a, b, r) for a, b, r in pending]
        for _ in as_completed(futures):
            pass
    n_done = state["done"]; n_failed = state["failed"]

    print(f"[judge-vllm] complete: {n_done - n_failed} ok, {n_failed} fail, "
          f"{(time.time() - start)/60:.1f}min")
    return 0


if __name__ == "__main__":
    sys.exit(main())
