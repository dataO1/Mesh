"""Caption every track in the corpus with Music Flamingo.

Plays to MF's strongest mode — long-form descriptive captioning. The
captions become the basis for a 768-d sentence-transformer feature
that augments MuQ-MuLan during probe training.

Schema per cached caption:
    /home/data01/Music/mesh-track-grading/round7_6_captions/<judge>/<track_id>.json
    {
      "track_id": int,
      "caption": str,
      "wall_time_s": float,
      "ts": str,
      "model": str,
      "max_tokens": int,
      "temperature": float,
      "top_p": float
    }

Resume: any track with a JSON on disk is skipped; atomic write via rename.

Usage:
    bash spike/track-grading/run_r7_step.sh run_judge_caption.py \
         --tracks-subset smoke-200 --workers 8

    bash spike/track-grading/run_r7_step.sh run_judge_caption.py \
         --tracks-subset all --workers 8
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))
from judges import MusicFlamingoJudge, InferenceError  # noqa: E402
from run_judge_pointwise import (  # noqa: E402  reuse helpers
    AudioCache,
    enumerate_tracks,
    select_tracks,
)


def caption_path(out_root: Path, judge_id: str, track_id: int) -> Path:
    return out_root / judge_id / f"{int(track_id)}.json"


def write_atomic(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, ensure_ascii=False))
    os.replace(tmp, path)


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--judge", default="music_flamingo",
                   choices=["music_flamingo"])
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/audio"))
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_captions"))
    p.add_argument("--tracks-subset", default="smoke-200",
                   help="all | smoke-N | first-N")
    p.add_argument("--max-tokens", type=int, default=256,
                   help="caption length budget (256 ≈ 190 words)")
    p.add_argument("--temperature", type=float, default=0.7,
                   help="NVIDIA-recommended decoding")
    p.add_argument("--top-p", type=float, default=0.9)
    p.add_argument("--prompt", default=None,
                   help="override default caption prompt")
    p.add_argument("--workers", type=int, default=8)
    p.add_argument("--audio-cache-size", type=int, default=4000)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def run(args) -> int:
    out_root = args.out_dir
    out_root.mkdir(parents=True, exist_ok=True)

    all_tids = enumerate_tracks(args.audio_dir)
    tids = select_tracks(all_tids, args.tracks_subset, args.seed)
    print(f"[caption] {len(tids)} tracks selected ({args.tracks_subset})")

    pending = []
    n_resumed = 0
    for tid in tids:
        if caption_path(out_root, args.judge, tid).exists():
            n_resumed += 1
            continue
        pending.append(tid)
    print(f"[caption] {len(pending)} tracks pending; {n_resumed} resumed")
    if not pending:
        print("[caption] nothing to do — all captions already cached")
        return 0

    if args.judge == "music_flamingo":
        judge = MusicFlamingoJudge()
    else:
        raise ValueError(f"unsupported judge: {args.judge}")
    if not judge.is_alive():
        print(f"[caption] judge {args.judge} not responding at {judge.url}",
              file=sys.stderr)
        return 1
    print(f"[caption] judge alive: {judge.judge_id} ({judge.model_name})")
    print(f"[caption] decoding: T={args.temperature} top_p={args.top_p} "
          f"max_tokens={args.max_tokens}")

    audio_cache = AudioCache(capacity=args.audio_cache_size)
    state = {"ok": 0, "skip": 0, "infer_fail": 0}
    state_lock = threading.Lock()
    times: list[float] = []
    word_counts: list[int] = []

    def process(tid: int):
        wav = audio_cache.get(tid, args.audio_dir, judge.sample_rate)
        if wav is None:
            with state_lock:
                state["skip"] += 1
            return
        ts = datetime.now(timezone.utc).isoformat(timespec="seconds")
        try:
            res = judge.caption(
                track_id=tid, audio_array=wav,
                prompt_text=args.prompt,
                max_tokens=args.max_tokens,
                temperature=args.temperature,
                top_p=args.top_p,
            )
        except InferenceError as e:
            with state_lock:
                state["infer_fail"] += 1
            print(f"  infer_fail tid={tid}: {e}", file=sys.stderr)
            return
        payload = {
            "track_id": int(tid),
            "caption": res["caption"],
            "wall_time_s": res["wall_time_s"],
            "ts": ts,
            "model": res["model"],
            "max_tokens": args.max_tokens,
            "temperature": args.temperature,
            "top_p": args.top_p,
            "completion_tokens": res.get("completion_tokens"),
        }
        write_atomic(caption_path(out_root, args.judge, tid), payload)
        with state_lock:
            state["ok"] += 1
            times.append(res["wall_time_s"])
            word_counts.append(len(res["caption"].split()))

    print(f"[caption] dispatching {len(pending)} tracks across {args.workers} workers ...")
    t0 = time.time()
    last_report = t0
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = [ex.submit(process, tid) for tid in pending]
        for done, fut in enumerate(as_completed(futs), 1):
            fut.result()
            now = time.time()
            if now - last_report >= 30:
                with state_lock:
                    snap = dict(state); ts_copy = list(times); wc_copy = list(word_counts)
                rate = snap["ok"] / max(now - t0, 1e-6)
                avg_wall = (sum(ts_copy) / len(ts_copy)) if ts_copy else 0
                avg_words = (sum(wc_copy) / len(wc_copy)) if wc_copy else 0
                print(f"  [{done}/{len(pending)}] ok={snap['ok']} "
                      f"infer_fail={snap['infer_fail']} skip={snap['skip']} "
                      f"tput={rate:.2f} c/s  avg_wall={avg_wall:.2f}s  "
                      f"avg_words={avg_words:.0f}")
                last_report = now
    wall = time.time() - t0

    print()
    print("=== caption sweep done ===")
    print(f"tracks attempted: {len(pending)}")
    print(f"ok:               {state['ok']}")
    print(f"infer_fail:       {state['infer_fail']}")
    print(f"skip (no audio):  {state['skip']}")
    if times:
        ts = sorted(times)
        print(f"per-call wall:    p50={ts[len(ts)//2]:.2f}s  "
              f"p95={ts[int(0.95*(len(ts)-1))]:.2f}s")
    if word_counts:
        wc = sorted(word_counts)
        print(f"caption length:   p10={wc[int(0.10*(len(wc)-1))]}w  "
              f"p50={wc[len(wc)//2]}w  p90={wc[int(0.90*(len(wc)-1))]}w")
    print(f"wall:             {wall:.1f}s ({state['ok']/max(wall,1e-6):.2f} c/s)")
    return 0 if state["ok"] > 0 else 1


if __name__ == "__main__":
    sys.exit(run(parse_args()))
