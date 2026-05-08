"""Sweep worker counts to find the optimal text-LLM throughput.

Picks N random captions from the smoke set and rates them at varying
worker concurrency, reporting throughput, p50/p95 latency, error rate.

Usage:
    bash spike/track-grading/run_r7_step.sh bench_text_llm_throughput.py \\
        --url http://172.16.51.90:8000/v1/chat/completions \\
        --model sakamakismile/Qwen3.6-27B-Text-NVFP4-MTP \\
        --n 100 --workers-list 8 16 32 48 64 96 128 \\
        --no-think
"""
from __future__ import annotations

import argparse
import json
import math
import os
import random
import statistics
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import requests


SYSTEM_PROMPT = (
    "You are a music analyst rating DJ-set intensity from text descriptions."
)
USER_TEMPLATE = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a 1–5 scale. "
    "Reply with one digit (1, 2, 3, 4, or 5)."
)


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--captions-root", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo"))
    p.add_argument("--n", type=int, default=100,
                   help="captions per worker-count trial")
    p.add_argument("--url", default=os.environ.get(
        "TEXT_LLM_URL", "http://localhost:8002/v1/chat/completions"))
    p.add_argument("--model", default=os.environ.get(
        "TEXT_LLM_MODEL", "Qwen/Qwen2.5-3B-Instruct"))
    p.add_argument("--api-key", default=os.environ.get("TEXT_LLM_API_KEY", None))
    p.add_argument("--workers-list", nargs="+", type=int,
                   default=[8, 16, 32, 48, 64, 96, 128])
    p.add_argument("--max-tokens", type=int, default=4)
    p.add_argument("--temperature", type=float, default=0.0)
    p.add_argument("--no-think", action="store_true")
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--warmup", type=int, default=4,
                   help="warmup calls before timing each trial")
    return p.parse_args()


def main(args) -> int:
    files = sorted(args.captions_root.glob("*.json"))
    if not files:
        sys.exit(f"no captions at {args.captions_root}")
    rng = random.Random(args.seed)
    rng.shuffle(files)
    # Cycle if we need more requests than available captions
    raw_caps: list[str] = []
    for f in files:
        rec = json.loads(f.read_text())
        cap = (rec.get("caption") or "").strip()
        if cap:
            raw_caps.append(cap)
    if not raw_caps:
        sys.exit("no captions found")
    captions = [raw_caps[i % len(raw_caps)] for i in range(args.n)]
    print(f"[bench] pool of {len(raw_caps)} unique captions, "
          f"cycled to N={args.n} requests")
    print(f"[bench] {len(captions)} captions, model={args.model}")
    print(f"[bench] url={args.url}")
    print(f"[bench] worker counts: {args.workers_list}")

    headers = {"Content-Type": "application/json"}
    if args.api_key:
        headers["Authorization"] = f"Bearer {args.api_key}"

    def call(cap: str) -> tuple[bool, float, str]:
        payload = {
            "model": args.model,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": USER_TEMPLATE.format(caption=cap)},
            ],
            "max_tokens": args.max_tokens,
            "temperature": args.temperature,
        }
        if args.no_think:
            payload["chat_template_kwargs"] = {"enable_thinking": False}
        t0 = time.perf_counter()
        try:
            r = requests.post(args.url, json=payload, headers=headers, timeout=120)
            r.raise_for_status()
            content = (r.json()["choices"][0]["message"].get("content") or "").strip()
        except Exception as e:
            return False, time.perf_counter() - t0, str(e)[:80]
        return True, time.perf_counter() - t0, content[:8]

    # Warmup
    print(f"\n[bench] warmup {args.warmup} calls (sequential) ...")
    for i in range(min(args.warmup, len(captions))):
        ok, dt, _ = call(captions[i])
        if not ok:
            print(f"  warmup {i}: FAIL after {dt:.2f}s — endpoint not ready?")
        else:
            print(f"  warmup {i}: ok in {dt:.2f}s")

    # Sweep
    print(f"\n{'workers':>8s}  {'wall_s':>8s}  {'c/s':>8s}  {'p50_ms':>8s}  "
          f"{'p95_ms':>8s}  {'fail':>5s}  {'sample_response':<20s}")
    print(f"{'-'*8}  {'-'*8}  {'-'*8}  {'-'*8}  {'-'*8}  {'-'*5}  {'-'*20}")
    results = []
    for W in args.workers_list:
        latencies = []
        contents = []
        n_fail = 0
        t0 = time.perf_counter()
        with ThreadPoolExecutor(max_workers=W) as ex:
            futs = [ex.submit(call, cap) for cap in captions]
            for f in as_completed(futs):
                ok, dt, body = f.result()
                if ok:
                    latencies.append(dt)
                    if len(contents) < 5: contents.append(body)
                else:
                    n_fail += 1
        wall = time.perf_counter() - t0
        n_ok = len(latencies)
        if n_ok == 0:
            print(f"{W:>8d}  ALL FAILED")
            results.append({"workers": W, "wall": wall, "tput": 0, "fail": n_fail})
            continue
        latencies.sort()
        p50 = latencies[len(latencies) // 2] * 1000
        p95 = latencies[int(0.95 * (len(latencies) - 1))] * 1000
        tput = n_ok / wall
        print(f"{W:>8d}  {wall:>8.2f}  {tput:>8.2f}  {p50:>8.1f}  {p95:>8.1f}  "
              f"{n_fail:>5d}  {contents[0]!r}")
        results.append({"workers": W, "wall": wall, "tput": tput,
                        "p50_ms": p50, "p95_ms": p95, "fail": n_fail,
                        "sample": contents[0] if contents else ""})

    # Find the knee
    if results:
        best = max(results, key=lambda r: r["tput"])
        print(f"\n[bench] BEST throughput: {best['tput']:.2f} c/s @ {best['workers']} workers")
        # Estimate full-run time at best throughput
        for n_corpus in (200, 5000, 15000, 30000, 40000):
            print(f"  est. full-corpus {n_corpus:>5d} → {n_corpus/best['tput']/60:.1f} min "
                  f"({n_corpus/best['tput']/3600:.2f} hr)")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
