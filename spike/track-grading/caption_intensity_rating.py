"""Stage S4 — Caption → text-LLM intensity rating (20-bucket).

Feeds each MF caption (text only) into a text LLM with a 00–19 DJ-set-
intensity rubric. Reads two-token logprobs (tens digit ∈ {0,1}, ones
digit ∈ {0..9}) and returns the soft expected value over the 20 valid
joint pairs, normalised to [0, 1].

Resolution chosen 2026-05-07 after a held-out CV bench across {5, 10, 20,
50, 100} buckets: b20 saturates audio-grounded predictability (CV R² ≈ 0.39,
statistically tied with b50; b5/b10 lose ~0.08 R²; b100 anchors and drops).
See documents/round-7-6-pipeline-spec.md and bench_resolution.py.

This is the LLM-juror source in the multi-source jury. Heterogeneous from
the audio-judged sources (r7.5 BT, MF Likert, mined-tag aggressive) because
the text LLM never sees audio — it scores caption text only.

Usage:
    bash spike/track-grading/run_r7_step.sh caption_intensity_rating.py \\
         --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \\
         --out /home/data01/Music/mesh-track-grading/round7_6_caption_intensity.npz \\
         --workers 8

Pre-requisite: text-LLM serve at http://localhost:8002 (start via
`bash spike/track-grading/serve_text_llm.sh` with TEXT_LLM_MODEL set).
"""
from __future__ import annotations

import argparse
import json
import math
import os
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import requests
from requests.adapters import HTTPAdapter
from urllib3.util.retry import Retry


SYSTEM_PROMPT = (
    "You are a music analyst rating DJ-set intensity from text descriptions."
)

# 20-bucket two-digit prompt; same wording as bench_resolution.PROMPT_20.
USER_TEMPLATE = (
    "Description of a music clip:\n\n"
    '"""\n{caption}\n"""\n\n'
    "Rate the clip's overall DJ-set intensity on a fine 00–19 scale where "
    "00 = silent ambient and 19 = maximum gabber/hardcore intensity. Use "
    "the full range — different sub-genres should land at meaningfully "
    "different values (e.g., deep house ≈ 09, drum-and-bass ≈ 13, "
    "industrial techno ≈ 16, hardcore ≈ 18, ambient ≈ 02).\n\n"
    "Reply with EXACTLY two digits (e.g., 07, 13, 18). Nothing else."
)

N_BUCKETS = 20
TENS_TOKENS = ("0", "1")                                   # tens digit ∈ {0,1}
ONES_TOKENS = tuple(str(i) for i in range(10))             # ones digit ∈ {0..9}


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--captions-root", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--url", default=os.environ.get(
        "TEXT_LLM_URL", "http://localhost:8002/v1/chat/completions"))
    p.add_argument("--health-url", default=os.environ.get(
        "TEXT_LLM_HEALTH_URL", "http://localhost:8002/health"))
    p.add_argument("--model", default=os.environ.get(
        "TEXT_LLM_MODEL", "Qwen/Qwen2.5-3B-Instruct"))
    p.add_argument("--api-key", default=os.environ.get("TEXT_LLM_API_KEY", None),
                   help="Bearer token for remote OpenAI-compatible endpoints "
                        "(e.g., a Qwen on a Spark/NIM). None for local serves.")
    p.add_argument("--no-health-check", action="store_true",
                   help="skip the /health probe (some remote endpoints don't expose one)")
    p.add_argument("--no-think", action="store_true",
                   default=os.environ.get("TEXT_LLM_NO_THINK", "0") == "1",
                   help="disable Qwen3-style reasoning/thinking via "
                        "chat_template_kwargs={enable_thinking: false}; needed "
                        "for Qwen3 reasoning models so the answer comes "
                        "immediately instead of after a long <think> block")
    p.add_argument("--workers", type=int, default=int(
        os.environ.get("TEXT_LLM_WORKERS", "8")))
    p.add_argument("--top-logprobs", type=int, default=12)
    p.add_argument("--max-tokens", type=int, default=6)
    p.add_argument("--temperature", type=float, default=0.0)
    return p.parse_args()


def soft_score_20_from_logprobs(logprobs: dict) -> tuple[float, list[float]]:
    """Two-token recovery on a 20-bucket 0..19 scale.

    Reads token-position 0 (tens digit ∈ {0,1}) and token-position 1
    (ones digit ∈ {0..9}); models the joint as the product of the two
    marginals over valid digits; renormalises mass over the 20 valid
    pairs (10·a + b ≤ 19); returns (score in [0,1], length-20 bucket
    distribution). On any missing/empty signal, falls back to uniform
    (caller should use hard_score_from_text instead).
    """
    uniform = [1.0 / N_BUCKETS] * N_BUCKETS
    if not logprobs:
        return 0.5, uniform
    content = logprobs.get("content") or []
    if len(content) < 2:
        return 0.5, uniform

    def marginal(slot: dict, valid: tuple[str, ...]) -> dict[str, float]:
        m: dict[str, float] = {t: -math.inf for t in valid}
        for entry in slot.get("top_logprobs") or []:
            tok = (entry.get("token") or "").strip()
            if tok in m:
                m[tok] = max(m[tok], entry["logprob"])
                continue
            ds = "".join(c for c in tok if c.isdigit())
            if ds and ds[0] in m:
                m[ds[0]] = max(m[ds[0]], entry["logprob"])
        if all(math.isinf(v) for v in m.values()):
            return {t: 1.0 / len(valid) for t in valid}  # uniform fallback
        peak = max(v for v in m.values() if not math.isinf(v))
        exps = {t: math.exp(v - peak) if not math.isinf(v) else 0.0
                for t, v in m.items()}
        z = sum(exps.values())
        return {t: (e / z if z > 0 else 1.0 / len(valid)) for t, e in exps.items()}

    p_tens = marginal(content[0], TENS_TOKENS)
    p_ones = marginal(content[1], ONES_TOKENS)
    bp = [0.0] * N_BUCKETS
    for a_str, pa in p_tens.items():
        for b_str, pb in p_ones.items():
            v = 10 * int(a_str) + int(b_str)
            if v < N_BUCKETS:
                bp[v] = pa * pb
    z_total = sum(bp)
    if z_total <= 0:
        return 0.5, uniform
    bp = [b / z_total for b in bp]
    score = sum(b * i / (N_BUCKETS - 1) for i, b in enumerate(bp))
    return score, bp


def make_resilient_session(retries: int = 6, backoff: float = 1.5) -> requests.Session:
    """HTTP session with exponential-backoff retry on transient errors.

    Retries on:
      - Connection errors (TCP reset, refused, DNS hiccup)
      - Read timeouts
      - 429 Too Many Requests (with Retry-After)
      - 5xx server errors (502, 503, 504)
    Does NOT retry on 4xx (those are permanent — wrong payload, wrong model id, etc.)

    Total retry budget at retries=6, backoff=1.5: roughly 1.5^0..5 = ~30s + the
    request times themselves. Handles 30-60s network blips cleanly without
    losing work, while a longer outage falls through to the streaming wrapper.
    """
    s = requests.Session()
    retry_cfg = Retry(
        total=retries,
        connect=retries,
        read=retries,
        status=retries,
        backoff_factor=backoff,
        status_forcelist=[429, 500, 502, 503, 504],
        allowed_methods=frozenset(["POST", "GET"]),
        respect_retry_after_header=True,
        raise_on_status=False,
    )
    adapter = HTTPAdapter(max_retries=retry_cfg, pool_connections=64, pool_maxsize=128)
    s.mount("http://", adapter)
    s.mount("https://", adapter)
    return s


def hard_score_from_text(text: str) -> tuple[float, list[float]]:
    """Fallback when the remote endpoint doesn't return logprobs.

    Parses the leading digit pair "00".."19" from the response. Returns
    (score_in_0_1, one-hot bucket_probs of length 20). If only one digit
    is present, treats it as the ones digit (tens=0). Defaults to bucket
    10 / score=0.5 if no usable digit is found.
    """
    bp = [0.0] * N_BUCKETS
    digits = [c for c in text.strip() if c.isdigit()]
    if not digits:
        bp[N_BUCKETS // 2] = 1.0
        return 0.5, bp
    if len(digits) >= 2:
        v = 10 * int(digits[0]) + int(digits[1])
    else:
        v = int(digits[0])
    if not (0 <= v < N_BUCKETS):
        bp[N_BUCKETS // 2] = 1.0
        return 0.5, bp
    bp[v] = 1.0
    return v / (N_BUCKETS - 1), bp


def main(args) -> int:
    # Resume-safety: if --out already exists, load it and skip those track_ids.
    # Lets us re-run the rater periodically during a caption sweep so it
    # streams captions as they become available without redoing prior work.
    prior_track_ids: set[int] = set()
    prior_scores: list[float] = []
    prior_buckets: list[list[float]] = []
    prior_raws: list[str] = []
    prior_tids_list: list[int] = []
    if args.out.exists():
        try:
            z = np.load(args.out, allow_pickle=True)
            prior_buckets_arr = z["bucket_probs"]
            if prior_buckets_arr.ndim != 2 or prior_buckets_arr.shape[1] != N_BUCKETS:
                # Old NPZ from a different bucket count (e.g. legacy 5-bucket).
                # Don't merge — start fresh so all rows have consistent shape.
                print(f"[rate] existing NPZ has {prior_buckets_arr.shape[1]}-bucket "
                      f"probs but rater is now {N_BUCKETS}-bucket; ignoring prior "
                      f"and re-rating all captions", file=sys.stderr)
            else:
                prior_tids_list = [int(t) for t in z["track_ids"]]
                prior_track_ids = set(prior_tids_list)
                prior_scores = [float(s) for s in z["score"]]
                prior_buckets = [list(b) for b in prior_buckets_arr]
                if "raw_first_token" in z.files:
                    prior_raws = [str(r) for r in z["raw_first_token"]]
                else:
                    prior_raws = [""] * len(prior_tids_list)
                print(f"[rate] resume: {len(prior_track_ids)} track ratings already "
                      f"on disk; will skip those")
        except Exception as e:
            print(f"[rate] resume read failed ({e}); starting fresh", file=sys.stderr)
            prior_track_ids = set()

    # Optional health check
    if not args.no_health_check:
        try:
            r = requests.get(args.health_url, timeout=10)
            if r.status_code != 200:
                print(f"[rate] text-LLM serve not healthy at {args.health_url}",
                      file=sys.stderr)
                return 1
        except Exception as e:
            print(f"[rate] text-LLM serve unreachable: {e}", file=sys.stderr)
            return 1
    headers = {"Content-Type": "application/json"}
    if args.api_key:
        headers["Authorization"] = f"Bearer {args.api_key}"
    # Single shared session with retry adapter — pooled connections + automatic
    # exponential-backoff retry on transient network/server errors.
    sess = make_resilient_session()

    all_files = sorted(args.captions_root.glob("*.json"))
    if not all_files:
        print(f"no captions at {args.captions_root}", file=sys.stderr)
        return 1
    # Filter out already-rated tracks
    files: list[Path] = []
    for f in all_files:
        try:
            tid = int(f.stem)
        except ValueError:
            continue
        if tid not in prior_track_ids:
            files.append(f)
    print(f"[rate] {len(all_files)} captions on disk, "
          f"{len(prior_track_ids)} already rated, "
          f"{len(files)} pending")
    if not files:
        print("[rate] nothing to do — all captions already rated")
        return 0
    print(f"[rate] model={args.model}, workers={args.workers}")

    state_lock = threading.Lock()
    track_ids: list[int] = []
    scores: list[float] = []
    bucket_probs_all: list[list[float]] = []
    raws: list[str] = []
    n_fail = 0

    def call(rec_path: Path) -> None:
        nonlocal n_fail
        try:
            rec = json.loads(rec_path.read_text())
        except Exception:
            with state_lock:
                n_fail += 1
            return
        tid = int(rec["track_id"])
        cap = (rec.get("caption") or "").strip()
        if not cap:
            with state_lock:
                n_fail += 1
            return
        payload = {
            "model": args.model,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": USER_TEMPLATE.format(caption=cap)},
            ],
            "max_tokens": args.max_tokens,
            "temperature": args.temperature,
            "logprobs": True,
            "top_logprobs": args.top_logprobs,
        }
        if args.no_think:
            # vLLM Qwen3 chat template honours this to suppress the
            # reasoning/<think> prefix and emit the answer directly.
            payload["chat_template_kwargs"] = {"enable_thinking": False}
        try:
            r = sess.post(args.url, json=payload, headers=headers, timeout=120)
            r.raise_for_status()
            data = r.json()["choices"][0]
            text = data["message"]["content"] or ""
            logprobs = data.get("logprobs") or {}
        except Exception as e:
            with state_lock:
                n_fail += 1
            # Track-id stays NOT in the output NPZ → next streaming-wrapper
            # poll will pick it up automatically (resume-safety only marks
            # successful ratings as "done").
            print(f"  [rate] tid={tid} POST failed (after retries): {e}",
                  file=sys.stderr)
            return
        # Prefer logprob-recovered soft score; if the remote endpoint doesn't
        # return logprobs (some proxied OpenAI-compatible servers strip them)
        # or returns < 2 token positions, fall back to hard digit parsing.
        score_0_1, bp = soft_score_20_from_logprobs(logprobs)
        uniform_p = 1.0 / N_BUCKETS
        if all(abs(p - uniform_p) < 1e-6 for p in bp):
            # Uniform fallback — no usable logprobs, use the hard digits.
            score_0_1, bp = hard_score_from_text(text)
        with state_lock:
            track_ids.append(tid)
            scores.append(score_0_1)
            bucket_probs_all.append(bp)
            raws.append(text.strip()[:8])

    print(f"[rate] dispatching across {args.workers} workers ...")
    t0 = time.time()
    last_report = t0
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = [ex.submit(call, f) for f in files]
        for done, fut in enumerate(as_completed(futs), 1):
            fut.result()
            now = time.time()
            if now - last_report >= 30:
                with state_lock:
                    n = len(scores)
                rate = n / max(now - t0, 1e-6)
                print(f"  [{done}/{len(files)}] ok={n} fail={n_fail} "
                      f"tput={rate:.2f} c/s")
                last_report = now
    wall = time.time() - t0

    # Merge with prior NPZ (resume-safety) and sort by track_id deterministically
    track_ids = list(prior_tids_list) + list(track_ids)
    scores = list(prior_scores) + list(scores)
    bucket_probs_all = list(prior_buckets) + list(bucket_probs_all)
    raws = list(prior_raws) + list(raws)
    order = np.argsort(track_ids)
    track_ids_arr = np.array(track_ids, dtype=np.int64)[order]
    scores_arr = np.array(scores, dtype=np.float32)[order]
    bucket_arr = np.array(bucket_probs_all, dtype=np.float32)[order]
    raws_arr = np.array(raws, dtype=object)[order]

    print()
    print("=== caption→text-LLM intensity done ===")
    print(f"ok:    {len(scores)} / {len(files)}")
    print(f"fail:  {n_fail}")
    print(f"score range: [{scores_arr.min():.3f}, {scores_arr.max():.3f}]")
    print(f"score std:   {scores_arr.std():.3f}")
    print(f"wall: {wall:.1f}s ({len(scores)/max(wall,1e-6):.2f} c/s)")

    # Atomic write: tmp + os.replace so a crash mid-write never corrupts the
    # resume NPZ (G10 / network-resilience requirement). np.savez writes a
    # complete zip in memory and only flushes when the file handle closes,
    # so writing to a sibling .tmp then atomically renaming is a clean fix.
    args.out.parent.mkdir(parents=True, exist_ok=True)
    # Atomic write via file-handle: passing a file object to np.savez stops
    # numpy from auto-appending ".npz" to the path (which it does for str/
    # PathLike), so the tmp basename matches the rename source exactly.
    tmp_path = args.out.with_suffix(args.out.suffix + ".tmp")
    with open(tmp_path, "wb") as fh:
        np.savez(
            fh,
            track_ids=track_ids_arr,
            score=scores_arr,                # ∈ [0, 1]
            bucket_probs=bucket_arr,
            raw_first_token=raws_arr,        # for debug per spec § 10
            model_name=args.model,
        )
    os.replace(tmp_path, args.out)
    print(f"[rate] wrote {args.out}")
    return 0 if len(scores) > 0 else 1


if __name__ == "__main__":
    sys.exit(main(parse_args()))
