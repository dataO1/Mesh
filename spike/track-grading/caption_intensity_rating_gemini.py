"""Stage S4 (Gemini variant) — Caption → Gemini Flash intensity rating (20-bucket).

Async port of `caption_intensity_rating.py` for the Google Gemini API.
Used as the 4th juror in Round-7.7 Phase-1b drift test (see
`Mesh — Round 7.7 Improvement Research.md` §Phase 1b).

Schema parity with the vLLM 3-juror NPZs:
    track_ids        int64       shape (N,)
    score            float32     shape (N,)        ∈ [0, 1]
    bucket_probs     float32     shape (N, 20)
    raw_first_token  object[str] shape (N,)
    model_name       str

Score recovery (best → worst):
  1. logprobs single-token bucket: top-K alternatives at output position 0
     match against "00".."19" tokens directly.
  2. logprobs tens × ones marginal: tens digit ∈ {"0","1"} at pos 0 ×
     ones digit ∈ {"0".."9"} at pos 1, joint product over the 20 valid
     pairs. Mirrors `soft_score_20_from_logprobs` in the vLLM rater.
  3. hard text parse: parse the leading 1-2 digits of the response.
     One-hot bucket, score = v / 19.

Resume safety: re-reading an existing --out NPZ skips already-rated
track_ids (same pattern as caption_intensity_rating.py).

Usage:
    GEMINI_API_KEY=... bash spike/track-grading/run_r7_step.sh \\
        caption_intensity_rating_gemini.py \\
        --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \\
        --out /home/data01/Music/mesh-track-grading/round7_6_caption_intensity_gemini_flash.npz \\
        --model gemini-2.5-flash \\
        --concurrency 50

Pre-requisite: `pip install google-genai` in the spike venv (the
companion `run_phase1b.sh` orchestrator does this for you).
"""
from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import sys
import time
from pathlib import Path
from typing import Optional

import numpy as np


SYSTEM_PROMPT = (
    "You are a music analyst rating DJ-set intensity from text descriptions."
)

# Verbatim copy of caption_intensity_rating.py USER_TEMPLATE.
# Drift test depends on prompt parity — do NOT diverge.
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
TENS_TOKENS = ("0", "1")
ONES_TOKENS = tuple(str(i) for i in range(10))


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--captions-root", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--model", default=os.environ.get(
        "GEMINI_MODEL", "gemini-3-flash-preview"))
    p.add_argument("--concurrency", type=int, default=int(
        os.environ.get("GEMINI_CONCURRENCY", "50")))
    p.add_argument("--top-logprobs", type=int, default=20,
                   help="top-K logprobs per output token position (max 20 on "
                        "current Gemini API)")
    p.add_argument("--max-tokens", type=int, default=6)
    p.add_argument("--temperature", type=float, default=0.0)
    p.add_argument("--retry-budget", type=int, default=5,
                   help="per-request retry attempts on 429/5xx/timeout")
    p.add_argument("--checkpoint-every", type=int, default=500,
                   help="flush partial results to NPZ every N successful "
                        "ratings (resume-safety against crashes)")
    p.add_argument("--limit", type=int, default=None,
                   help="cap pending captions for smoke testing")
    return p.parse_args()


def soft_score_single_token(top_candidates) -> Optional[tuple[float, list[float]]]:
    """Single-token bucket recovery: position 0 alternatives ↦ "00".."19".

    Works when Gemini tokenises "07" as ONE token. Returns (score, bp) only
    if at least 5 distinct buckets are matched (otherwise the marginal is
    too sparse to be useful and we fall through to two-token recovery).
    """
    if not top_candidates:
        return None
    pos0 = top_candidates[0]
    candidates = getattr(pos0, "candidates", None) or []
    bucket_lps = [-math.inf] * N_BUCKETS
    n_matched = 0
    for c in candidates:
        tok = ((getattr(c, "token", None) or "")).strip()
        digits = "".join(ch for ch in tok if ch.isdigit())
        if len(digits) == 2:
            try:
                v = int(digits)
            except ValueError:
                continue
            if 0 <= v < N_BUCKETS:
                lp = float(getattr(c, "log_probability", -math.inf))
                if lp > bucket_lps[v]:
                    if math.isinf(bucket_lps[v]):
                        n_matched += 1
                    bucket_lps[v] = lp
    if n_matched < 5:
        return None
    peak = max(lp for lp in bucket_lps if not math.isinf(lp))
    exps = [math.exp(lp - peak) if not math.isinf(lp) else 0.0
            for lp in bucket_lps]
    z = sum(exps)
    if z <= 0:
        return None
    bp = [e / z for e in exps]
    score = sum(p * i / (N_BUCKETS - 1) for i, p in enumerate(bp))
    return score, bp


def soft_score_two_token(top_candidates) -> Optional[tuple[float, list[float]]]:
    """Two-token tens × ones marginal product (mirrors vLLM rater)."""
    if not top_candidates or len(top_candidates) < 2:
        return None

    def marginal(pos, valid: tuple[str, ...]) -> Optional[dict[str, float]]:
        candidates = getattr(pos, "candidates", None) or []
        m = {t: -math.inf for t in valid}
        for c in candidates:
            tok = ((getattr(c, "token", None) or "")).strip()
            ds = "".join(ch for ch in tok if ch.isdigit())
            if ds and ds[0] in m:
                lp = float(getattr(c, "log_probability", -math.inf))
                if lp > m[ds[0]]:
                    m[ds[0]] = lp
        if all(math.isinf(v) for v in m.values()):
            return None
        peak = max(v for v in m.values() if not math.isinf(v))
        exps = {t: math.exp(v - peak) if not math.isinf(v) else 0.0
                for t, v in m.items()}
        z = sum(exps.values())
        if z <= 0:
            return None
        return {t: e / z for t, e in exps.items()}

    p_tens = marginal(top_candidates[0], TENS_TOKENS)
    p_ones = marginal(top_candidates[1], ONES_TOKENS)
    if p_tens is None or p_ones is None:
        return None
    bp = [0.0] * N_BUCKETS
    for t_str, pt in p_tens.items():
        for o_str, po in p_ones.items():
            v = 10 * int(t_str) + int(o_str)
            if v < N_BUCKETS:
                bp[v] = pt * po
    z = sum(bp)
    if z <= 0:
        return None
    bp = [b / z for b in bp]
    score = sum(p * i / (N_BUCKETS - 1) for i, p in enumerate(bp))
    return score, bp


def hard_score_from_text(text: str) -> tuple[float, list[float]]:
    """Fallback: parse the leading 1-2 digits, bucket 10 default."""
    bp = [0.0] * N_BUCKETS
    digits = [c for c in (text or "").strip() if c.isdigit()]
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


def decode_score(response) -> tuple[float, list[float], str]:
    """Try single-token → two-token → text. Returns (score, bp, raw_8)."""
    text = ""
    try:
        cands = getattr(response, "candidates", None) or []
        if cands:
            content = getattr(cands[0], "content", None)
            parts = getattr(content, "parts", None) or []
            for part in parts:
                # Skip parts marked as thoughts (Gemini 3 may emit thought
                # signatures even on thinking_level=MINIMAL).
                if getattr(part, "thought", False):
                    continue
                t = getattr(part, "text", None)
                if t:
                    text = t
                    break
    except Exception:
        text = ""
    text = (text or "").strip()

    top_candidates = None
    try:
        cands = getattr(response, "candidates", None) or []
        if cands:
            lp_result = getattr(cands[0], "logprobs_result", None)
            if lp_result is not None:
                top_candidates = getattr(lp_result, "top_candidates", None)
    except Exception:
        top_candidates = None

    if top_candidates:
        for fn in (soft_score_single_token, soft_score_two_token):
            try:
                out = fn(top_candidates)
            except Exception:
                out = None
            if out is not None:
                score, bp = out
                return score, bp, text[:8]

    score, bp = hard_score_from_text(text)
    return score, bp, text[:8]


async def detect_logprobs_support(client, types_mod, model: str) -> bool:
    """Probe with a tiny request whether the model honours response_logprobs.

    Some Gemini models (notably preview Flash variants) reject the
    `response_logprobs` config field with HTTP 400 INVALID_ARGUMENT and
    message "Logprobs is not enabled for this model". We detect that up
    front and skip the field for the rest of the run; score recovery
    falls back to hard text parsing.
    """
    cfg = types_mod.GenerateContentConfig(
        temperature=0.0,
        max_output_tokens=2,
        response_logprobs=True,
        logprobs=5,
    )
    try:
        await client.aio.models.generate_content(
            model=model,
            contents="Reply with the digit 5.",
            config=cfg,
        )
        return True
    except Exception as e:
        msg = (str(e) or "").lower()
        if "logprob" in msg and ("not enabled" in msg or "not supported" in msg
                                  or "invalid_argument" in msg):
            return False
        # Anything else (auth failure, model-not-found, quota) — re-raise.
        raise


def _thinking_kwargs(types_mod, model: str) -> dict:
    """Minimise thinking-token consumption per model family.

    Gemini 3 uses `thinking_level` (string enum); Gemini 2.5 uses the older
    `thinking_budget` (int). Gemini 3 cannot be fully disabled. For Flash /
    Flash-Lite the lowest accepted level is MINIMAL; for Pro the API rejects
    MINIMAL and requires at least LOW. Empirically determined 2026-05-10.
    """
    name = (model or "").lower()
    if name.startswith("gemini-3") and "pro" in name:
        return {"thinking_config": types_mod.ThinkingConfig(
            thinking_level="LOW")}
    if name.startswith("gemini-3"):
        return {"thinking_config": types_mod.ThinkingConfig(
            thinking_level="MINIMAL")}
    if name.startswith("gemini-2.5"):
        return {"thinking_config": types_mod.ThinkingConfig(thinking_budget=0)}
    return {}


def _build_cfg(types_mod, model: str, max_tokens: int, temperature: float,
               top_logprobs: int, logprobs_ok: bool):
    kwargs = dict(
        system_instruction=SYSTEM_PROMPT,
        temperature=temperature,
        max_output_tokens=max_tokens,
    )
    if logprobs_ok:
        kwargs["response_logprobs"] = True
        kwargs["logprobs"] = top_logprobs
    kwargs.update(_thinking_kwargs(types_mod, model))
    return types_mod.GenerateContentConfig(**kwargs)


async def rate_one(
    client,
    types_mod,
    model: str,
    caption: str,
    sem: asyncio.Semaphore,
    retry_budget: int,
    max_tokens: int,
    temperature: float,
    top_logprobs: int,
    logprobs_ok: bool,
):
    """One Gemini call with retry on 429/5xx/timeout."""
    cfg = _build_cfg(types_mod, model, max_tokens, temperature, top_logprobs,
                     logprobs_ok)
    backoff = 1.0
    last_exc: Optional[BaseException] = None
    for attempt in range(retry_budget + 1):
        try:
            async with sem:
                return await client.aio.models.generate_content(
                    model=model,
                    contents=USER_TEMPLATE.format(caption=caption),
                    config=cfg,
                )
        except Exception as e:
            last_exc = e
            msg = (str(e) or "").lower()
            transient = (
                "429" in msg or "rate" in msg or "quota" in msg
                or "503" in msg or "504" in msg or "502" in msg
                or "timeout" in msg or "deadline" in msg
                or "unavailable" in msg or "internal" in msg
            )
            if not transient or attempt == retry_budget:
                raise
            # Honour Retry-After when present (Gemini 429 sometimes returns it).
            sleep_s = backoff
            for tok in msg.replace(",", " ").split():
                if tok.startswith("retry-after"):
                    try:
                        sleep_s = max(sleep_s, float(tok.split("=")[-1]))
                    except Exception:
                        pass
            await asyncio.sleep(sleep_s)
            backoff = min(backoff * 2.0, 30.0)
    if last_exc is not None:
        raise last_exc
    raise RuntimeError("unreachable")


def write_npz_atomic(out: Path, records: list[dict], model_name: str) -> None:
    """Atomic write — tmp + os.replace."""
    if not records:
        return
    records.sort(key=lambda r: r["track_id"])
    track_ids = np.array([r["track_id"] for r in records], dtype=np.int64)
    scores = np.array([r["score"] for r in records], dtype=np.float32)
    buckets = np.array([r["bucket_probs"] for r in records], dtype=np.float32)
    raws = np.array([r["raw"] for r in records], dtype=object)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(out.suffix + ".tmp")
    with open(tmp, "wb") as fh:
        np.savez(
            fh,
            track_ids=track_ids,
            score=scores,
            bucket_probs=buckets,
            raw_first_token=raws,
            model_name=model_name,
        )
    os.replace(tmp, out)


def load_prior(out: Path) -> tuple[set[int], list[dict]]:
    if not out.exists():
        return set(), []
    z = np.load(out, allow_pickle=True)
    bp_arr = z["bucket_probs"]
    if bp_arr.ndim != 2 or bp_arr.shape[1] != N_BUCKETS:
        print(f"[gemini] existing NPZ shape mismatch ({bp_arr.shape}); "
              "starting fresh", file=sys.stderr)
        return set(), []
    tids = [int(t) for t in z["track_ids"]]
    scores = [float(s) for s in z["score"]]
    raws = ([str(r) for r in z["raw_first_token"]]
            if "raw_first_token" in z.files else [""] * len(tids))
    records = [
        {"track_id": tids[i], "score": scores[i],
         "bucket_probs": list(bp_arr[i]), "raw": raws[i]}
        for i in range(len(tids))
    ]
    print(f"[gemini] resume: {len(tids)} prior ratings on disk", flush=True)
    return set(tids), records


async def main_async(args: argparse.Namespace) -> int:
    try:
        from google import genai
        from google.genai import types as genai_types
    except ImportError as e:
        print(f"[gemini] google-genai not installed: {e}\n"
              "  install with: "
              "/home/data01/.cache/mesh-spike/vllm-env/bin/pip install google-genai",
              file=sys.stderr)
        return 2

    api_key = os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY")
    if not api_key:
        print("[gemini] GEMINI_API_KEY (or GOOGLE_API_KEY) not set",
              file=sys.stderr)
        return 2
    client = genai.Client(api_key=api_key)

    prior_ids, prior_records = load_prior(args.out)
    all_files = sorted(args.captions_root.glob("*.json"))
    pending: list[Path] = []
    for f in all_files:
        try:
            tid = int(f.stem)
        except ValueError:
            continue
        if tid not in prior_ids:
            pending.append(f)
    if args.limit is not None:
        pending = pending[: args.limit]
    print(f"[gemini] {len(all_files)} captions on disk, "
          f"{len(prior_ids)} already rated, {len(pending)} pending",
          flush=True)
    if not pending:
        print("[gemini] nothing to do — all captions already rated", flush=True)
        return 0
    print(f"[gemini] model={args.model} concurrency={args.concurrency} "
          f"top_logprobs={args.top_logprobs}", flush=True)

    # Probe logprobs support up front (one tiny request) so we don't burn
    # 39k requests learning that the model rejects the field.
    try:
        logprobs_ok = await detect_logprobs_support(
            client, genai_types, args.model)
    except Exception as e:
        print(f"[gemini] logprobs probe failed (non-logprobs error): {e}",
              file=sys.stderr)
        return 2
    if logprobs_ok:
        print(f"[gemini] model accepts response_logprobs — using soft-bucket "
              f"recovery", flush=True)
    else:
        print(f"[gemini] model rejects response_logprobs — falling back to "
              f"hard text parse (score = parsed_int / 19)", flush=True)

    sem = asyncio.Semaphore(args.concurrency)
    state_lock = asyncio.Lock()
    new_records: list[dict] = []
    n_fail = 0
    last_checkpoint = 0

    async def process(f: Path) -> None:
        nonlocal n_fail, last_checkpoint
        try:
            rec = json.loads(f.read_text())
            tid = int(rec["track_id"])
            cap = (rec.get("caption") or "").strip()
            if not cap:
                async with state_lock:
                    n_fail += 1
                return
        except Exception as e:
            async with state_lock:
                n_fail += 1
            print(f"  [gemini] read {f.name} failed: {e}", file=sys.stderr)
            return
        try:
            resp = await rate_one(
                client, genai_types, args.model, cap, sem,
                args.retry_budget, args.max_tokens, args.temperature,
                args.top_logprobs, logprobs_ok,
            )
        except Exception as e:
            async with state_lock:
                n_fail += 1
            print(f"  [gemini] tid={tid} failed after retries: {e}",
                  file=sys.stderr)
            return
        score, bp, raw = decode_score(resp)
        async with state_lock:
            new_records.append({
                "track_id": tid, "score": score,
                "bucket_probs": bp, "raw": raw,
            })
            n_done = len(new_records)
            if n_done - last_checkpoint >= args.checkpoint_every:
                last_checkpoint = n_done
                merged = list(prior_records) + list(new_records)
                # Atomic checkpoint while holding the state lock.
                write_npz_atomic(args.out, merged, args.model)

    t0 = time.time()
    last_report = t0
    tasks = [asyncio.create_task(process(f)) for f in pending]
    completed = 0
    for fut in asyncio.as_completed(tasks):
        await fut
        completed += 1
        now = time.time()
        if now - last_report >= 30.0:
            async with state_lock:
                ok = len(new_records)
                fail = n_fail
            rate = ok / max(now - t0, 1e-6)
            eta_s = (len(pending) - completed) / max(rate, 1e-6)
            print(f"  [{completed}/{len(pending)}] ok={ok} fail={fail} "
                  f"tput={rate:.2f} c/s eta={eta_s/60:.1f}m", flush=True)
            last_report = now
    wall = time.time() - t0

    async with state_lock:
        merged = list(prior_records) + list(new_records)
        write_npz_atomic(args.out, merged, args.model)
        ok = len(new_records)
        fail = n_fail

    print()
    print("=== caption→Gemini intensity done ===")
    print(f"new ok:   {ok} / {len(pending)}")
    print(f"new fail: {fail}")
    print(f"wall:     {wall:.1f}s ({ok / max(wall, 1e-6):.2f} c/s)")
    if merged:
        scores_arr = np.array([r["score"] for r in merged], dtype=np.float32)
        print(f"score range: [{scores_arr.min():.3f}, {scores_arr.max():.3f}]")
        print(f"score std:   {scores_arr.std():.3f}")
    print(f"[gemini] wrote {args.out}")
    return 0 if ok > 0 or len(pending) == 0 else 1


def main() -> int:
    args = parse_args()
    return asyncio.run(main_async(args))


if __name__ == "__main__":
    sys.exit(main())
