"""Pointwise (K=1) tournament runner for round-7.6 with Music Flamingo.

Music Flamingo's audio-token cap is 1 per prompt (architectural limit of
the AF3 backbone), so K=4 N-way ranking is impossible. Instead each
(track, axis) cell is rated 0-100 directly; the linear probe trains
against that (N_tracks, N_axes) matrix in place of BT priors.

Schema per cached call:
    /home/data01/Music/mesh-track-grading/round7_6_pointwise/<judge>/<axis>/<track_id>.json
    {
      "track_id": int,
      "axis": str,
      "judge": str,
      "model": str,
      "score": float,         # 0-100, NaN if parse_fail
      "raw_response": str,
      "wall_time_s": float,
      "ts": str
    }

Resume: any cell with a JSON on disk is skipped. Atomic write via rename.

Usage:
    bash spike/track-grading/run_r7_step.sh run_judge_pointwise.py \
         --judge music_flamingo --tracks-subset smoke-200 --workers 8

    bash spike/track-grading/run_r7_step.sh run_judge_pointwise.py \
         --judge music_flamingo --tracks-subset all --workers 8
"""
from __future__ import annotations

import argparse
import json
import math
import os
import random
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import soundfile as sf

sys.path.insert(0, str(Path(__file__).parent))
from judges import (  # noqa: E402
    MusicFlamingoJudge,
    ScoreResult,
    ParseError,
    InferenceError,
)


# ── audio loader (same convention as round 7.5) ──────────────────────
def load_audio(path: Path, target_sr: int) -> np.ndarray | None:
    if not path.exists():
        return None
    try:
        wav, sr = sf.read(str(path), dtype="float32", always_2d=False)
        if wav.ndim > 1:
            wav = wav.mean(axis=1)
        if sr != target_sr:
            from scipy.signal import resample_poly
            wav = resample_poly(wav, target_sr, sr).astype(np.float32)
        # Pad/truncate to 30 seconds (Whisper chunk length)
        target_len = target_sr * 30
        if wav.shape[0] < target_len:
            wav = np.pad(wav, (0, target_len - wav.shape[0]))
        else:
            wav = wav[:target_len]
        return wav
    except Exception:
        return None


# ── cell cache ───────────────────────────────────────────────────────
def cell_path(out_root: Path, judge_id: str, axis_id: str, track_id: int) -> Path:
    return out_root / judge_id / axis_id / f"{int(track_id)}.json"


def write_cell_atomic(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, ensure_ascii=False))
    os.replace(tmp, path)


# ── track enumeration ────────────────────────────────────────────────
def enumerate_tracks(audio_dir: Path) -> list[int]:
    """All Deezer track ids present as audio files."""
    return sorted(
        int(f.stem.split("_", 1)[1])
        for f in audio_dir.glob("dz_*.mp3")
    )


def select_tracks(all_tids: list[int], subset: str, seed: int) -> list[int]:
    """Deterministic track subsetting.

    'all'         → every audio file in the corpus
    'smoke-N'     → seeded random N-sample
    'first-N'     → first N (sorted by track id, useful for diff'd reruns)
    """
    if subset == "all":
        return list(all_tids)
    rng = random.Random(seed)
    if subset.startswith("smoke-"):
        n = int(subset.split("-", 1)[1])
        sample = list(all_tids)
        rng.shuffle(sample)
        return sorted(sample[:n])
    if subset.startswith("first-"):
        n = int(subset.split("-", 1)[1])
        return list(all_tids)[:n]
    raise ValueError(f"unknown subset spec: {subset}")


# ── audio cache ──────────────────────────────────────────────────────
class AudioCache:
    """Thread-safe LRU-ish audio cache.

    Single decode per track across all axes. With 16 axes per track in
    the working set, hit rate is effectively 15/16 = 94%. Capacity is
    set well above the working-window size to avoid thrash; with 4000
    tracks × ~2 MB each = ~8 GB RAM cap, well within our 93 GB budget.
    """
    def __init__(self, capacity: int = 4000):
        self.capacity = capacity
        self.lock = threading.Lock()
        self.cache: dict[int, np.ndarray] = {}
        self.order: list[int] = []  # FIFO eviction

    def get(self, tid: int, audio_dir: Path, sr: int) -> np.ndarray | None:
        with self.lock:
            wav = self.cache.get(tid)
            if wav is not None:
                return wav
        wav = load_audio(audio_dir / f"dz_{tid}.mp3", sr)
        if wav is None:
            return None
        with self.lock:
            if tid not in self.cache:
                self.cache[tid] = wav
                self.order.append(tid)
                if len(self.order) > self.capacity:
                    evict = self.order.pop(0)
                    self.cache.pop(evict, None)
        return wav


# ── core dispatch ────────────────────────────────────────────────────
def run(args) -> int:
    out_root = args.out_dir
    out_root.mkdir(parents=True, exist_ok=True)
    (out_root / "logs").mkdir(exist_ok=True)

    cfg = json.loads(args.prompts_file.read_text())
    if args.mode == "likert":
        template = cfg["_meta"]["likert_template"]
    elif args.mode == "raw-int":
        template = cfg["_meta"]["score_template"]
    else:
        raise ValueError(f"unknown mode: {args.mode}")
    print(f"[run] mode: {args.mode}")

    selected_axes = args.axes if args.axes else None
    axes = [a for a in cfg["axes"] if (selected_axes is None or a["id"] in selected_axes)]
    if not axes:
        print(f"no axes match {selected_axes}", file=sys.stderr)
        return 2
    print(f"[run] {len(axes)} axes selected: {[a['id'] for a in axes]}")

    all_tids = enumerate_tracks(args.audio_dir)
    print(f"[run] corpus has {len(all_tids)} audio files at {args.audio_dir}")
    tids = select_tracks(all_tids, args.tracks_subset, args.seed)
    print(f"[run] {len(tids)} tracks selected ({args.tracks_subset})")

    # Build cell list, skipping cells already on disk
    cells: list[tuple[dict, int]] = []  # (axis_dict, track_id)
    n_resumed = 0
    for axis in axes:
        for tid in tids:
            p = cell_path(out_root, args.judge, axis["id"], tid)
            if p.exists():
                n_resumed += 1
                continue
            cells.append((axis, tid))
    print(f"[run] {len(cells)} cells pending; {n_resumed} resumed from cache")
    if not cells:
        print("[run] nothing to do — every cell already cached")
        return 0

    if args.judge == "music_flamingo":
        judge = MusicFlamingoJudge()
    else:
        raise ValueError(f"unsupported judge for pointwise: {args.judge}")
    if not judge.is_alive():
        print(f"[run] judge {args.judge} not responding at {judge.url}",
              file=sys.stderr)
        return 1
    print(f"[run] judge alive: {judge.judge_id} ({judge.model_name})")

    audio_cache = AudioCache(capacity=args.audio_cache_size)
    state = {"ok": 0, "parse_fail": 0, "infer_fail": 0, "skip": 0}
    state_lock = threading.Lock()
    times: list[float] = []

    def process(axis: dict, tid: int):
        wav = audio_cache.get(tid, args.audio_dir, judge.sample_rate)
        if wav is None:
            with state_lock:
                state["skip"] += 1
            return
        prompt = template.format(
            axis_id=axis["id"],
            low_pole=axis["low_pole"],
            high_pole=axis["high_pole"],
        )
        ts = datetime.now(timezone.utc).isoformat(timespec="seconds")
        try:
            if args.mode == "likert":
                res = judge.score_likert(
                    track_id=tid, audio_array=wav, prompt_text=prompt,
                    axis_id=axis["id"], max_tokens=4, temperature=0.0,
                )
            else:
                res = judge.score(
                    track_id=tid, audio_array=wav, prompt_text=prompt,
                    axis_id=axis["id"], max_tokens=16, temperature=0.0,
                )
            payload = {
                "track_id": int(tid),
                "axis": axis["id"],
                "judge": judge.judge_id,
                "model": judge.model_name,
                "mode": args.mode,
                "score": float(res.score),
                "raw_response": res.raw_response,
                "wall_time_s": res.wall_time_s,
                "ts": ts,
            }
            if res.extras.get("bucket_probs"):
                payload["bucket_probs"] = [
                    round(p, 6) for p in res.extras["bucket_probs"]
                ]
            write_cell_atomic(cell_path(out_root, args.judge, axis["id"], tid), payload)
            with state_lock:
                state["ok"] += 1
                times.append(res.wall_time_s)
        except ParseError as e:
            payload = {
                "track_id": int(tid),
                "axis": axis["id"],
                "judge": judge.judge_id,
                "model": judge.model_name,
                "score": math.nan,
                "raw_response": str(e),
                "wall_time_s": 0.0,
                "ts": ts,
                "error": "parse_fail",
            }
            write_cell_atomic(cell_path(out_root, args.judge, axis["id"], tid), payload)
            with state_lock:
                state["parse_fail"] += 1
        except InferenceError as e:
            with state_lock:
                state["infer_fail"] += 1
            # Don't persist transient inference errors — they should be retried
            # on the next run rather than poisoning the cache.
            print(f"  infer_fail axis={axis['id']} tid={tid}: {e}",
                  file=sys.stderr)

    print(f"[run] dispatching {len(cells)} cells across {args.workers} workers ...")
    t0 = time.time()
    last_report = t0
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = [ex.submit(process, ax, tid) for ax, tid in cells]
        for done, fut in enumerate(as_completed(futs), 1):
            fut.result()  # surface exceptions from workers
            now = time.time()
            if now - last_report >= 30:
                with state_lock:
                    snap = dict(state)
                rate = snap["ok"] / max(now - t0, 1e-6)
                print(f"  [{done}/{len(cells)}] ok={snap['ok']} "
                      f"parse_fail={snap['parse_fail']} "
                      f"infer_fail={snap['infer_fail']} "
                      f"skip={snap['skip']}  tput={rate:.2f} c/s")
                last_report = now
    wall = time.time() - t0

    print()
    print("=== pointwise tournament done ===")
    print(f"cells:           {len(cells)}")
    print(f"ok:              {state['ok']}")
    print(f"parse_fail:      {state['parse_fail']}")
    print(f"infer_fail:      {state['infer_fail']}")
    print(f"skip (no audio): {state['skip']}")
    if times:
        ts = sorted(times)
        p50 = ts[len(ts) // 2]
        p95 = ts[int(0.95 * (len(ts) - 1))]
        print(f"per-call wall (s): p50={p50:.2f} p95={p95:.2f}")
    print(f"wall:            {wall:.1f}s ({state['ok']/max(wall,1e-6):.2f} c/s sustained)")
    return 0 if state["ok"] > 0 else 1


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--judge", default="music_flamingo",
                   choices=["music_flamingo"],
                   help="pointwise judges only — Qwen3-Omni uses K=4 path")
    p.add_argument("--mode", default="likert",
                   choices=["likert", "raw-int"],
                   help="likert: 5-bucket + logprob soft scoring (recommended). "
                        "raw-int: ask for 0-100 integer (collapses on subjective axes).")
    p.add_argument("--prompts-file", type=Path,
                   default=Path("spike/track-grading/round7_6_pointwise_prompts.json"))
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/audio"))
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_pointwise"))
    p.add_argument("--axes", nargs="*", default=None,
                   help="axes filter; default = all 16 in prompts file")
    p.add_argument("--tracks-subset", default="all",
                   help="all | smoke-N | first-N")
    p.add_argument("--workers", type=int, default=8)
    p.add_argument("--audio-cache-size", type=int, default=4000)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


if __name__ == "__main__":
    sys.exit(run(parse_args()))
