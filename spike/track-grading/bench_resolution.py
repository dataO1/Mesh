"""Compare 5 / 10 / 20-bucket intensity rating schemes on the same captions.

For each scheme, runs the same prompt with appropriate output range and
logprob recovery, then reports:
  - score distribution (std, distinct values, histogram)
  - pairwise Spearman rho across schemes (do they agree on rank?)
  - intra-cluster spread (does higher resolution actually distinguish similar tracks?)

The 20-bucket scheme uses two-token recovery: model outputs "XX" where the
first token is the tens digit (0/1) and the second is the ones digit (0-9).
Score is the joint expectation under independence.
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
from pathlib import Path

import numpy as np
import requests


SYSTEM_PROMPT = (
    "You are a music analyst rating DJ-set intensity from text descriptions."
)

PROMPT_5 = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a 1–5 scale.\n"
    "1 = very low (ambient, contemplative, sparse).\n"
    "2 = low (gentle, slow, reflective).\n"
    "3 = medium (steady groove, balanced).\n"
    "4 = high (energetic, driving, club-ready).\n"
    "5 = very high (relentless, abrasive, peak-time).\n\n"
    "Reply with one digit (1, 2, 3, 4, or 5)."
)

PROMPT_10 = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a 0–9 scale.\n"
    "0 = silent / pure ambient soundscape\n"
    "1 = very low (drone, deep contemplative)\n"
    "2 = low (slow, sparse, reflective)\n"
    "3 = mid-low (gentle groove)\n"
    "4 = medium (steady balanced groove)\n"
    "5 = mid-high (driving, propulsive)\n"
    "6 = high (energetic, club-ready)\n"
    "7 = very high (peak-time, intense)\n"
    "8 = extreme (relentless, abrasive)\n"
    "9 = maximum (gabber, hardcore, ferocious)\n\n"
    "Reply with one digit (0-9)."
)

PROMPT_20 = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a fine 00–19 scale where "
    "00 = silent ambient and 19 = maximum gabber/hardcore intensity. Use "
    "the full range — different sub-genres should land at meaningfully "
    "different values (e.g., deep house ≈ 09, drum-and-bass ≈ 13, "
    "industrial techno ≈ 16, hardcore ≈ 18, ambient ≈ 02).\n\n"
    "Reply with EXACTLY two digits (e.g., 07, 13, 18). Nothing else."
)

PROMPT_50 = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a fine 00–49 scale where "
    "00 = silent ambient and 49 = maximum gabber/hardcore intensity. Use "
    "the full range — different sub-genres should land at meaningfully "
    "different values (e.g., ambient ≈ 05, deep house ≈ 22, drum-and-bass "
    "≈ 33, industrial techno ≈ 41, hardcore ≈ 47).\n\n"
    "Reply with EXACTLY two digits (e.g., 18, 33, 47). Nothing else."
)

PROMPT_100 = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a fine 00–99 scale where "
    "00 = silent ambient and 99 = maximum gabber/hardcore intensity. Use "
    "the full range — different sub-genres should land at meaningfully "
    "different values (e.g., ambient ≈ 10, deep house ≈ 45, drum-and-bass "
    "≈ 65, industrial techno ≈ 82, hardcore ≈ 95).\n\n"
    "Reply with EXACTLY two digits (e.g., 37, 65, 95). Nothing else."
)


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--captions-root", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo"))
    p.add_argument("--n", type=int, default=200)
    p.add_argument("--url", default=os.environ.get(
        "TEXT_LLM_URL", "http://localhost:8002/v1/chat/completions"))
    p.add_argument("--model", default=os.environ.get(
        "TEXT_LLM_MODEL", "jeffcookio/Mistral-Small-3.2-24B-Instruct-2506-awq-sym"))
    p.add_argument("--api-key", default=os.environ.get("TEXT_LLM_API_KEY", None))
    p.add_argument("--workers", type=int, default=16)
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading"))
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--no-think", action="store_true",
                   help="set chat_template_kwargs.enable_thinking=false (for Qwen3 family)")
    p.add_argument("--schemes", type=lambda s: [int(x) for x in s.split(",")],
                   default=[5, 10, 20],
                   help="comma-separated bucket counts to test (subset of {5,10,20,50,100})")
    return p.parse_args()


# ── Per-scheme: prompt, max_tokens, recovery function ─────────────────
def soft_score_5(content_list):
    if not content_list: return 0.5, [0.2]*5
    first = content_list[0]
    p = {str(i+1): 0.0 for i in range(5)}
    has_any = False
    for e in first.get("top_logprobs", []) or []:
        tok = (e.get("token") or "").strip()
        if tok in p:
            p[tok] = max(p[tok], math.exp(e["logprob"]))
            has_any = True
    if not has_any: return 0.5, [0.2]*5
    z = sum(p.values())
    if z == 0: return 0.5, [0.2]*5
    bp = [p[str(i+1)] / z for i in range(5)]
    return sum(b * i / 4 for i, b in enumerate(bp)), bp

def soft_score_10(content_list):
    if not content_list: return 0.5, [0.1]*10
    first = content_list[0]
    p = {str(i): 0.0 for i in range(10)}
    has_any = False
    for e in first.get("top_logprobs", []) or []:
        tok = (e.get("token") or "").strip()
        if tok in p:
            p[tok] = max(p[tok], math.exp(e["logprob"]))
            has_any = True
    if not has_any: return 0.5, [0.1]*10
    z = sum(p.values())
    if z == 0: return 0.5, [0.1]*10
    bp = [p[str(i)] / z for i in range(10)]
    return sum(b * i / 9 for i, b in enumerate(bp)), bp

def make_two_digit_scorer(n_buckets: int):
    """Two-token recovery for an N-bucket scheme (N ≤ 100).

    Tens-digit ∈ {0..⌈(N-1)/10⌉}, ones-digit ∈ {0..9}; valid pair if
    10*tens + ones < N. Score = Σ p_tens(a)·p_ones(b)·v / (N-1)
    over valid pairs, with mass renormalised to the valid set so missing
    or out-of-range tokens don't bias toward 0.5.
    """
    max_tens = (n_buckets - 1) // 10
    valid_tens = [str(i) for i in range(max_tens + 1)]

    def scorer(content_list):
        uniform = [1.0 / n_buckets] * n_buckets
        if not content_list or len(content_list) < 2:
            return 0.5, uniform
        p_a = {t: 0.0 for t in valid_tens}
        for e in content_list[0].get("top_logprobs", []) or []:
            tok = (e.get("token") or "").strip()
            if tok in p_a:
                p_a[tok] = max(p_a[tok], math.exp(e["logprob"]))
        p_b = {str(i): 0.0 for i in range(10)}
        for e in content_list[1].get("top_logprobs", []) or []:
            tok = (e.get("token") or "").strip()
            if tok in p_b:
                p_b[tok] = max(p_b[tok], math.exp(e["logprob"]))
        za = sum(p_a.values())
        zb = sum(p_b.values())
        if za == 0 or zb == 0:
            return 0.5, uniform
        p_a = {k: v / za for k, v in p_a.items()}
        p_b = {k: v / zb for k, v in p_b.items()}
        bp = [0.0] * n_buckets
        for a_str, pa in p_a.items():
            for b_str, pb in p_b.items():
                v = 10 * int(a_str) + int(b_str)
                if v < n_buckets:
                    bp[v] = pa * pb
        z_total = sum(bp)
        if z_total > 0:
            bp = [b / z_total for b in bp]
        score = sum(b * i / (n_buckets - 1) for i, b in enumerate(bp))
        return score, bp

    return scorer


soft_score_20 = make_two_digit_scorer(20)
soft_score_50 = make_two_digit_scorer(50)
soft_score_100 = make_two_digit_scorer(100)

# scheme → (name, prompt, max_tokens, recovery_fn, n_buckets)
SCHEMES = {
    5:   ("PROMPT_5",   PROMPT_5,   4, soft_score_5,   5),
    10:  ("PROMPT_10",  PROMPT_10,  4, soft_score_10,  10),
    20:  ("PROMPT_20",  PROMPT_20,  6, soft_score_20,  20),
    50:  ("PROMPT_50",  PROMPT_50,  6, soft_score_50,  50),
    100: ("PROMPT_100", PROMPT_100, 6, soft_score_100, 100),
}


def main(args) -> int:
    files = sorted(args.captions_root.glob("*.json"))
    if len(files) < args.n:
        sys.exit(f"only {len(files)} captions, need {args.n}")
    rng = random.Random(args.seed)
    rng.shuffle(files)
    files = files[: args.n]
    captions: list[tuple[int, str]] = []
    for f in files:
        rec = json.loads(f.read_text())
        cap = (rec.get("caption") or "").strip()
        if cap:
            captions.append((int(rec["track_id"]), cap))
    print(f"[bench] {len(captions)} captions, model={args.model}, workers={args.workers}")

    headers = {"Content-Type": "application/json"}
    if args.api_key:
        headers["Authorization"] = f"Bearer {args.api_key}"

    def call_one(scheme: int, cap: str) -> tuple[float, list[float], str, float]:
        _name, template, max_tok, recover, _n = SCHEMES[scheme]
        payload = {
            "model": args.model,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": template.format(caption=cap)},
            ],
            "max_tokens": max_tok,
            "temperature": 0.0,
            "logprobs": True,
            "top_logprobs": 12,
        }
        if args.no_think:
            payload["chat_template_kwargs"] = {"enable_thinking": False}
        t0 = time.perf_counter()
        try:
            r = requests.post(args.url, json=payload, headers=headers, timeout=60)
            r.raise_for_status()
            data = r.json()["choices"][0]
            text = (data["message"].get("content") or "").strip()
            content_list = (data.get("logprobs") or {}).get("content") or []
        except Exception as e:
            return float("nan"), [0]*SCHEMES[scheme][4], f"ERR:{str(e)[:40]}", time.perf_counter() - t0
        score, bp = recover(content_list)
        return score, bp, text, time.perf_counter() - t0

    results = {}  # scheme -> {tids: [], scores: [], buckets: [], samples: []}
    for scheme in args.schemes:
        if scheme not in SCHEMES:
            sys.exit(f"unknown scheme {scheme}; valid: {sorted(SCHEMES)}")
        print(f"\n--- scheme: {scheme} buckets ---")
        scores: list[float] = []
        buckets_all: list[list[float]] = []
        tids: list[int] = []
        samples: list[str] = []
        n_fail = 0
        t0 = time.perf_counter()
        with ThreadPoolExecutor(max_workers=args.workers) as ex:
            futs = {ex.submit(call_one, scheme, c): t for t, c in captions}
            for fut in as_completed(futs):
                tid = futs[fut]
                score, bp, text, _ = fut.result()
                if math.isnan(score):
                    n_fail += 1
                    continue
                scores.append(score)
                buckets_all.append(bp)
                tids.append(tid)
                if len(samples) < 6: samples.append(text)
        wall = time.perf_counter() - t0

        scores_arr = np.array(scores, dtype=np.float32)
        buckets_arr = np.array(buckets_all, dtype=np.float32)
        tids_arr = np.array(tids, dtype=np.int64)
        results[scheme] = {
            "scores": scores_arr, "buckets": buckets_arr, "tids": tids_arr,
            "n_fail": n_fail, "wall": wall, "samples": samples,
        }

        # Sort by tid so the cross-scheme comparison can align
        order = np.argsort(tids_arr)
        results[scheme]["tids"] = tids_arr[order]
        results[scheme]["scores"] = scores_arr[order]
        results[scheme]["buckets"] = buckets_arr[order]

        n_distinct = len(np.unique(np.round(scores_arr, 4)))
        # Histogram in 10 bins
        hist, edges = np.histogram(scores_arr, bins=10, range=(0, 1))
        print(f"  N={len(scores_arr)}, fail={n_fail}, wall={wall:.1f}s ({len(scores_arr)/wall:.1f} c/s)")
        print(f"  scores: mean={scores_arr.mean():.3f} std={scores_arr.std():.3f} "
              f"min={scores_arr.min():.3f} max={scores_arr.max():.3f}")
        print(f"  distinct values (rounded 4 decimals): {n_distinct}")
        print(f"  histogram (0→1, 10 bins):")
        for h, e0 in zip(hist, edges[:-1]):
            bar = "█" * int(40 * h / max(hist.max(), 1))
            print(f"    [{e0:.1f},{e0+0.1:.1f})  {h:>3d}  {bar}")
        print(f"  sample raw responses: {samples}")

        args.out_dir.mkdir(parents=True, exist_ok=True)
        out = args.out_dir / f"round7_6_caption_intensity_smoke_b{scheme}.npz"
        np.savez(out,
                 track_ids=tids_arr[order],
                 score=scores_arr[order],
                 bucket_probs=buckets_arr[order],
                 model_name=args.model)
        print(f"  saved {out}")

    # Cross-scheme rank agreement: pairwise ρ across every requested scheme.
    print(f"\n=== Cross-scheme Spearman ρ (do schemes agree on track ranking?) ===")
    schemes_run = list(results.keys())
    if len(schemes_run) >= 2:
        common = set.intersection(*[set(results[s]["tids"].tolist()) for s in schemes_run])
        if len(common) > 5:
            common_sorted = sorted(common)
            idx = {s: {int(t): i for i, t in enumerate(results[s]["tids"])} for s in schemes_run}
            vec = {s: np.array([results[s]["scores"][idx[s][t]] for t in common_sorted])
                   for s in schemes_run}

            def spearman(a, b):
                n = len(a); ra = np.argsort(np.argsort(a)); rb = np.argsort(np.argsort(b))
                return 1 - 6 * float(np.sum((ra - rb) ** 2)) / (n * (n * n - 1))

            for i, sa in enumerate(schemes_run):
                for sb in schemes_run[i + 1:]:
                    print(f"  ρ({sa:>3d}, {sb:>3d}) = {spearman(vec[sa], vec[sb]):+.4f}")

    print(f"\n=== Summary ===")
    print(f"  Higher distinct values + healthy std + high cross-scheme rank agreement = better resolution.")
    print(f"  If ρ between schemes is < 0.85, finer schemes are noise rather than signal.")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
