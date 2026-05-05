"""Round-7.5 K=4 N-way ranking tournaments with BALD active sampling.

For each of the 16 polar axes in `round7_5_axis_prompts.json`:

  1. Bootstrap 20% of the call budget with uniform-random K=4 tuples
     to seed BT (which has nothing to be uncertain about on call 1).
  2. Re-fit BT on bootstrap data.
  3. Active phase 80%: at each step pick a K=4 tuple that
     a) spans the BT model's uncertainty band (BALD: tracks whose pairwise
        win probabilities cluster near 0.5 with the tuple-mates), and
     b) prefers low pair-coverage tracks (each tuple touches 4 tracks
        with ~5–10 game target across the run).
     Re-fit BT every `BT_REFIT_EVERY` calls so the uncertainty estimate
     stays current.
  4. Persist per-call JSON under `/tmp/track-grading/round7_5_pairs/<axis>/`
     with full ranking + every-pair derived observation + LLM justification
     (preserved verbatim for round-7.5 mining step).

Each call presents 4 audio clips (A–D), the LLM returns a 4-letter
ordering "BACD" meaning B<A<C<D on the LOW→HIGH scale. Strict parser:
exactly 4 letters from {A,B,C,D}, each exactly once. Retry once on parse
failure, drop on second failure (BALD will re-cover the gap naturally).

Random presentation-order shuffle per call washes positional bias before
it can accumulate into the BT scores.

Usage (after vLLM is serving with audio=4):
  bash spike/track-grading/run_r7_step.sh run_nway_tournaments_r7_5.py
  bash spike/track-grading/run_r7_step.sh run_nway_tournaments_r7_5.py --axes timbre_roughness
  bash spike/track-grading/run_r7_step.sh run_nway_tournaments_r7_5.py --calls-per-axis 1500 --workers 12
"""
from __future__ import annotations

import contextlib
import argparse
import base64
import io
import json
import math
import os
import random
import re
import string
import sys
import threading
import time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from itertools import combinations
from pathlib import Path

import numpy as np
import requests
import soundfile as sf
import librosa


@contextlib.contextmanager
def _suppress_stderr():
    """Temporarily redirect stderr to /dev/null.
    Suppresses libmpg123 ID3v2 warnings during librosa audio loading."""
    old_fd = os.dup(2)
    devnull_fd = os.open(os.devnull, os.O_WRONLY)
    os.dup2(devnull_fd, 2)
    os.close(devnull_fd)
    try:
        yield
    finally:
        os.dup2(old_fd, 2)
        os.close(old_fd)


SAMPLE_RATE = 16_000
PREVIEW_SECS = 30
MODEL_NAME = "cpatonn/Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit"
VLLM_URL = os.environ.get("VLLM_URL", "http://localhost:8000/v1/chat/completions")
HEALTH_URL = VLLM_URL.rsplit("/v1/", 1)[0] + "/health"
REQUEST_TIMEOUT = 240
LETTERS = ("A", "B", "C", "D")  # K=4 fixed for now; re-derive if K changes

# How often to re-fit BT during the active phase. Smaller = more responsive
# uncertainty tracking, larger = cheaper. 100 hits the sweet spot: BT solver
# converges in <1 s on N≤2000 unique tracks even at the tail of the run.
BT_REFIT_EVERY = 100
# BALD scoring parameters.
TUPLE_CANDIDATES_PER_PICK = 64        # propose this many random tuples, score, pick best
LOW_COVERAGE_BONUS_WEIGHT = 0.4       # how much we boost under-sampled tracks
TARGET_GAMES_PER_TRACK = 8            # convergence aim for BT scores


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--prompts-file", type=Path,
                   default=Path("spike/track-grading/round7_5_axis_prompts.json"))
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/tmp/track-grading/audio"))
    p.add_argument("--embeddings", type=Path,
                   default=Path("/tmp/track-grading/embeddings/corpus_muq_mulan.npz"),
                   help="L2-normalised MuQ-MuLan embeddings; used for BALD candidate proposal")
    p.add_argument("--out-dir", type=Path,
                   default=Path("/tmp/track-grading/round7_5_pairs"))
    p.add_argument("--calls-per-axis", type=int, default=None,
                   help="override n_calls_per_axis from prompts file")
    p.add_argument("--bootstrap-fraction", type=float, default=None,
                   help="fraction of calls used for uniform bootstrap (default from prompts file)")
    p.add_argument("--axes", nargs="*", default=None,
                   help="restrict to these axis ids (default: all 16)")
    p.add_argument("--workers", type=int, default=12)
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--smoke", type=int, default=0,
                   help="if >0, run only this many calls per axis")
    return p.parse_args()


def wait_for_endpoint(timeout_s: float = 900) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            r = requests.get(HEALTH_URL, timeout=5)
            if r.status_code == 200:
                return True
        except Exception:
            pass
        time.sleep(3)
    return False


def parse_ranking(text: str) -> list[str] | None:
    """Strict K=4 parser: exactly 4 letters from {A,B,C,D}, each exactly once.

    Returns the 4 letters in low→high order, or None if unparseable.
    """
    if not text:
        return None
    first_line = text.strip().split("\n")[0].strip().upper()
    # Strip non-letter chars, then look for 4-letter run
    cleaned = "".join(c for c in first_line if c in "ABCD")
    if len(cleaned) != 4:
        return None
    if set(cleaned) != set("ABCD"):
        return None
    return list(cleaned)


def load_audio_b64(path: Path) -> str | None:
    try:
        with _suppress_stderr():
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
    buf = io.BytesIO()
    sf.write(buf, wav.astype(np.float32), SAMPLE_RATE,
             format="WAV", subtype="PCM_16")
    return base64.b64encode(buf.getvalue()).decode("ascii")


def vllm_judge_kway(prompt_text: str, b64_clips: list[str]) -> str:
    """POST K audio clips + text prompt; returns the model's text response."""
    content = []
    for b64 in b64_clips:
        content.append({"type": "input_audio",
                        "input_audio": {"data": b64, "format": "wav"}})
    content.append({"type": "text", "text": prompt_text})
    payload = {
        "model": MODEL_NAME,
        "messages": [
            {"role": "system", "content": "You are an expert music analyst."},
            {"role": "user", "content": content},
        ],
        "max_tokens": 120,
        "temperature": 0.0,
    }
    r = requests.post(VLLM_URL, json=payload, timeout=REQUEST_TIMEOUT)
    r.raise_for_status()
    return r.json()["choices"][0]["message"]["content"]


# ─── BT solver (vectorised Hunter MM, same math as round-7) ────────────────
def bt_mm(wins: dict[tuple[int, int], float],
          tracks: list[int],
          max_iter: int = 200, tol: float = 1e-6,
          prior_strength: float = 1.0) -> dict[int, float]:
    if not tracks:
        return {}
    idx = {t: i for i, t in enumerate(tracks)}
    n = len(tracks)
    W = np.zeros((n, n), dtype=np.float64)
    for (a, b), w in wins.items():
        if a in idx and b in idx:
            W[idx[a], idx[b]] += w
    N_mat = W + W.T
    W_row = W.sum(axis=1)
    a_prior = 1.0 + prior_strength
    b_prior = prior_strength
    s = np.ones(n, dtype=np.float64)
    for _ in range(max_iter):
        S = s[:, None] + s[None, :]
        S[S == 0] = 1.0
        ratio = N_mat / S
        np.fill_diagonal(ratio, 0.0)
        denom = b_prior + ratio.sum(axis=1)
        numer = W_row + (a_prior - 1)
        new_s = np.where(denom > 0, numer / denom, s)
        gm = math.exp(np.log(np.clip(new_s, 1e-12, None)).mean())
        if gm > 0:
            new_s = new_s / gm
        delta = float(np.max(np.abs(new_s - s)))
        s = new_s
        if delta < tol:
            break
    return {t: float(s[idx[t]]) for t in tracks}


def ranking_to_pairs(ranking_low_to_high: list[str],
                     letter_to_track: dict[str, int]) -> list[tuple[int, int]]:
    """A K=4 ranking 'BACD' (B<A<C<D) implies 6 pairwise win observations:
    higher-ranked beats lower-ranked. Returns list of (winner, loser) pairs."""
    out = []
    n = len(ranking_low_to_high)
    for i in range(n):
        for j in range(i + 1, n):
            loser = letter_to_track[ranking_low_to_high[i]]
            winner = letter_to_track[ranking_low_to_high[j]]
            out.append((winner, loser))
    return out


# ─── BALD-style tuple proposal ─────────────────────────────────────────────
def bald_pick_tuple(track_ids: np.ndarray,
                    bt_scores: dict[int, float],
                    games_count: dict[int, int],
                    rng: random.Random) -> tuple[int, int, int, int]:
    """Pick a K=4 tuple maximising expected information gain.

    Heuristic (cheap proxy for full BALD):
      - Sample TUPLE_CANDIDATES_PER_PICK random 4-tuples
      - For each, score = uncertainty_score - LOW_COVERAGE_BONUS_WEIGHT * coverage_penalty
        - uncertainty_score: standard deviation of the 4 BT scores. Lower std =
          tighter cluster = more uncertain about the ordering = higher BALD value.
          We invert so higher = better, with a floor.
        - coverage_penalty: average games_count across the 4 tracks (more games
          = more redundant)
      - Pick highest-scoring tuple
    """
    if len(track_ids) < 4:
        sys.exit(f"need ≥4 tracks for K=4, have {len(track_ids)}")

    track_list = list(map(int, track_ids))
    best = None
    best_score = -1e18

    # Pre-compute log-scores for efficient uncertainty calc
    log_scores = {t: math.log(max(bt_scores.get(t, 1.0), 1e-9)) for t in track_list}

    # Cap candidates by sampling without replacement of 4
    for _ in range(TUPLE_CANDIDATES_PER_PICK):
        cand = rng.sample(track_list, 4)
        # Uncertainty: tight cluster of BT log-scores = LLM should be uncertain
        # when ranking them. Use 1 / (1 + std) so tighter = higher.
        ls = [log_scores[t] for t in cand]
        ls_mean = sum(ls) / 4.0
        ls_var = sum((v - ls_mean) ** 2 for v in ls) / 4.0
        ls_std = math.sqrt(ls_var)
        uncertainty = 1.0 / (1.0 + ls_std)
        # Coverage penalty: prefer under-sampled tracks
        cov = sum(games_count.get(t, 0) for t in cand) / 4.0
        cov_penalty = cov / max(TARGET_GAMES_PER_TRACK, 1)
        score = uncertainty - LOW_COVERAGE_BONUS_WEIGHT * cov_penalty
        if score > best_score:
            best_score = score
            best = cand

    return tuple(best)  # type: ignore


def call_cache_path(out_dir: Path, axis_id: str, tuple_key: str) -> Path:
    pdir = out_dir / axis_id
    pdir.mkdir(parents=True, exist_ok=True)
    return pdir / f"{tuple_key}.json"


def tuple_key(track_ids: tuple[int, ...]) -> str:
    """Canonical identifier for a tuple's call cache (sorted IDs)."""
    return "_".join(str(t) for t in sorted(track_ids))


# ─── Per-axis runner ───────────────────────────────────────────────────────
def run_axis(axis: dict,
             template: str,
             corpus_track_ids: np.ndarray,
             audio_dir: Path,
             out_dir: Path,
             n_calls: int,
             bootstrap_fraction: float,
             workers: int,
             rng: random.Random) -> dict:
    aid = axis["id"]
    low = axis["low_pole"]
    high = axis["high_pole"]
    prompt_text = template.format(axis_id=aid, low_pole=low, high_pole=high)

    # State
    pair_wins: dict[tuple[int, int], float] = defaultdict(float)
    games_count: dict[int, int] = defaultdict(int)
    seen_tuples: set[str] = set()
    bt_scores: dict[int, float] = {}
    state_lock = threading.Lock()

    # Resume: scan existing per-call JSONs for this axis and replay them
    existing = list((out_dir / aid).glob("*.json")) if (out_dir / aid).exists() else []
    n_resumed = 0
    n_resumed_invalid = 0
    for f in existing:
        try:
            d = json.loads(f.read_text())
        except Exception:
            n_resumed_invalid += 1; continue
        ranking = d.get("ranking_low_to_high")
        l2t = d.get("letter_to_track")
        if not ranking or not l2t:
            n_resumed_invalid += 1; continue
        l2t = {k: int(v) for k, v in l2t.items()}
        try:
            pairs = ranking_to_pairs(ranking, l2t)
        except Exception:
            n_resumed_invalid += 1; continue
        for w, l in pairs:
            pair_wins[(w, l)] += 1.0
        for t in l2t.values():
            games_count[t] += 1
        seen_tuples.add(d["tuple_key"])
        n_resumed += 1
    print(f"[{aid}] resumed {n_resumed} prior calls "
          f"({n_resumed_invalid} invalid), {len(games_count)} unique tracks touched")

    # Compute initial BT if we have anything
    if pair_wins:
        seen_tracks = sorted(set(t for pair in pair_wins for t in pair))
        bt_scores = bt_mm(pair_wins, seen_tracks)

    # Audio cache: bound to ~6000 entries (~7 GB raw WAV) since axis run touches
    # at most n_calls*4 unique tracks; corpus is 15314 so we cap.
    audio_cache: dict[int, str] = {}
    cache_lock = threading.Lock()
    AUDIO_CACHE_CAP = 6000

    def get_b64(tid: int) -> str | None:
        with cache_lock:
            if tid in audio_cache:
                return audio_cache[tid]
        b64 = load_audio_b64(audio_dir / f"dz_{tid}.mp3")
        with cache_lock:
            if b64 is not None:
                if len(audio_cache) >= AUDIO_CACHE_CAP:
                    # FIFO drop a few entries
                    to_drop = max(1, AUDIO_CACHE_CAP // 50)
                    for k in list(audio_cache.keys())[:to_drop]:
                        audio_cache.pop(k, None)
                audio_cache[tid] = b64
        return b64

    # Plan: bootstrap_calls uniform, then BALD-driven up to n_calls total
    bootstrap_calls = int(round(n_calls * bootstrap_fraction))
    print(f"[{aid}] plan: {n_calls} calls = {bootstrap_calls} bootstrap + "
          f"{n_calls - bootstrap_calls} BALD")

    # Pre-build the bootstrap tuple list (uniform random, no replacement on
    # tuple_key). Each tuple is shuffled into a random presentation order to
    # cancel positional bias.
    def make_tuple_uniform() -> tuple[int, int, int, int]:
        for _ in range(20):
            cand = tuple(int(x) for x in rng.sample(corpus_track_ids.tolist(), 4))
            if tuple_key(cand) not in seen_tuples:
                return cand  # type: ignore
        # rare: fall back to allowing collision (will be resume-cached)
        return tuple(int(x) for x in rng.sample(corpus_track_ids.tolist(), 4))  # type: ignore

    def make_tuple_bald() -> tuple[int, int, int, int]:
        for _ in range(20):
            with state_lock:
                cand = bald_pick_tuple(corpus_track_ids, bt_scores, games_count, rng)
            if tuple_key(cand) not in seen_tuples:
                return cand
        return cand  # accept collision

    # Track progress
    state = {"done": n_resumed, "ok": 0, "fail_parse": 0, "fail_infer": 0,
             "fail_decode": 0, "skipped_resumed": 0}
    start = time.time()

    def process_tuple(call_idx: int, track_tuple: tuple[int, ...]) -> None:
        # Map tracks → letters with random presentation order
        letters = list(LETTERS)
        rng_local = random.Random(rng.randint(0, 1 << 31) ^ call_idx)
        rng_local.shuffle(letters)
        letter_to_track = dict(zip(letters, track_tuple))
        # We always present in A-B-C-D order audio-wise; the letter scrambling
        # above just permutes which track gets which letter, which is what
        # cancels positional bias.
        tk = tuple_key(track_tuple)
        with state_lock:
            if tk in seen_tuples:
                state["skipped_resumed"] += 1; state["done"] += 1
                return
            seen_tuples.add(tk)

        # Order audio clips by letter A,B,C,D
        ordered_tracks = [letter_to_track[L] for L in LETTERS]
        b64s = []
        for tid in ordered_tracks:
            b64 = get_b64(tid)
            if b64 is None:
                with state_lock:
                    state["fail_decode"] += 1; state["done"] += 1
                return
            b64s.append(b64)

        t0 = time.time()
        try:
            text = vllm_judge_kway(prompt_text, b64s)
        except Exception as e:
            with state_lock:
                state["fail_infer"] += 1; state["done"] += 1
            return

        ranking = parse_ranking(text)
        if ranking is None:
            # Retry once with same prompt + clips (zero-temp deterministic; if
            # the model failed once it may have spat extra prose, ask again
            # with stricter wording in a follow-up turn)
            try:
                text2 = vllm_judge_kway(
                    prompt_text + "\n\nRespond with JUST the 4-letter ordering on line 1.",
                    b64s)
                ranking = parse_ranking(text2)
            except Exception:
                ranking = None
            if ranking is None:
                with state_lock:
                    state["fail_parse"] += 1; state["done"] += 1
                # Persist a stub so we don't try this same tuple again
                stub = {
                    "axis": aid, "tuple_key": tk,
                    "track_tuple": list(track_tuple),
                    "letter_to_track": {L: int(letter_to_track[L]) for L in LETTERS},
                    "presentation_order": list(LETTERS),
                    "model": MODEL_NAME,
                    "raw_response": text,
                    "ranking_low_to_high": None,
                    "parse_error": True,
                    "wall_time_s": round(time.time() - t0, 2),
                    "ts": int(time.time()),
                }
                cache = call_cache_path(out_dir, aid, tk)
                tmp = cache.with_suffix(".json.tmp")
                tmp.write_text(json.dumps(stub, indent=2))
                tmp.rename(cache)
                return

        # Convert ranking → 6 pairwise observations
        pairs = ranking_to_pairs(ranking, letter_to_track)
        with state_lock:
            for w, l in pairs:
                pair_wins[(w, l)] += 1.0
            for t in track_tuple:
                games_count[t] += 1
            state["ok"] += 1; state["done"] += 1
            # periodic BT refit to keep BALD's uncertainty estimate fresh
            if state["ok"] % BT_REFIT_EVERY == 0:
                seen_tracks = sorted(set(t for pair in pair_wins for t in pair))
                bt_scores.clear()
                bt_scores.update(bt_mm(pair_wins, seen_tracks, max_iter=80))
                el = time.time() - start
                rate = state["ok"] / max(el, 0.001)
                eta = (n_calls - n_resumed - state["ok"]) / max(rate, 0.001)
                print(f"  [{aid}] {n_resumed + state['ok']}/{n_calls}  "
                      f"ok={state['ok']} parse_fail={state['fail_parse']} "
                      f"infer_fail={state['fail_infer']} "
                      f"({rate:.2f}/s, eta {eta/60:.1f} min, "
                      f"unique_tracks={len(games_count)})")

        record = {
            "axis": aid,
            "tuple_key": tk,
            "track_tuple": list(track_tuple),
            "letter_to_track": {L: int(letter_to_track[L]) for L in LETTERS},
            "presentation_order": list(LETTERS),
            "model": MODEL_NAME,
            "raw_response": text,
            "ranking_low_to_high": ranking,
            "wall_time_s": round(time.time() - t0, 2),
            "ts": int(time.time()),
        }
        cache = call_cache_path(out_dir, aid, tk)
        tmp = cache.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(record, indent=2))
        tmp.rename(cache)

    print(f"  [{aid}] running {workers} concurrent workers ...")
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futures: list = []
        # Bootstrap phase
        for i in range(bootstrap_calls):
            tup = make_tuple_uniform()
            futures.append(ex.submit(process_tuple, i, tup))
        # Drain bootstrap before scheduling BALD picks (need fresh BT)
        for _ in as_completed(futures):
            pass
        # First BT fit after bootstrap
        with state_lock:
            seen_tracks = sorted(set(t for pair in pair_wins for t in pair))
            bt_scores.clear()
            if seen_tracks:
                bt_scores.update(bt_mm(pair_wins, seen_tracks))
        print(f"  [{aid}] bootstrap done ({state['ok']} ok), "
              f"BT seeded on {len(bt_scores)} tracks; entering BALD phase")

        # BALD phase: schedule in chunks so the BT refit cycle gives BALD
        # fresh uncertainty data. Each BALD-chunk is BT_REFIT_EVERY tuples.
        bald_total = n_calls - bootstrap_calls
        ofs = bootstrap_calls
        while bald_total > 0:
            chunk = min(BT_REFIT_EVERY * 2, bald_total)  # 2x because workers parallelise
            futures = []
            for i in range(chunk):
                tup = make_tuple_bald()
                futures.append(ex.submit(process_tuple, ofs + i, tup))
            ofs += chunk
            for _ in as_completed(futures):
                pass
            bald_total -= chunk

    # Final pass: clean BT solve on all collected pairs.
    seen_tracks = sorted(set(t for pair in pair_wins for t in pair))
    if seen_tracks:
        bt_scores.clear()
        bt_scores.update(bt_mm(pair_wins, seen_tracks, max_iter=400))

    return {
        "axis": aid,
        "n_calls": n_calls,
        "n_done": state["done"],
        "n_ok": state["ok"],
        "n_resumed": n_resumed,
        "n_skipped_resumed": state["skipped_resumed"],
        "n_parse_fail": state["fail_parse"],
        "n_infer_fail": state["fail_infer"],
        "n_decode_fail": state["fail_decode"],
        "n_unique_tracks": len(games_count),
        "wall_min": (time.time() - start) / 60,
    }


def main() -> int:
    args = parse_args()
    rng = random.Random(args.seed)

    cfg = json.loads(args.prompts_file.read_text())
    template = cfg["_meta"]["judge_template"]
    n_calls = args.calls_per_axis or cfg["_meta"].get("n_calls_per_axis", 12000)
    bootstrap_fraction = (args.bootstrap_fraction
                          if args.bootstrap_fraction is not None
                          else cfg["_meta"].get("bootstrap_fraction", 0.20))
    if args.smoke:
        n_calls = args.smoke
        bootstrap_fraction = 1.0  # all uniform for smoke

    axes = cfg["axes"]
    if args.axes:
        axes = [a for a in axes if a["id"] in args.axes]
        if not axes:
            sys.exit(f"no axes match: {args.axes}")
    print(f"[r7.5] {len(axes)} axes, {n_calls} K=4 N-way calls per axis "
          f"(bootstrap {bootstrap_fraction:.0%})")

    # vLLM endpoint health check (will only happen at runtime, fine if fails here)
    print(f"[r7.5] checking vLLM at {VLLM_URL} ...")
    if not wait_for_endpoint(timeout_s=600):
        sys.exit(f"vLLM endpoint never came up at {VLLM_URL} — "
                 f"start serve_qwen3_omni.sh (with audio=4) first")
    print("[r7.5] vLLM ready")

    if not args.embeddings.exists():
        sys.exit(f"missing {args.embeddings} — run embed_corpus_mulan.py first")
    npz = np.load(args.embeddings, allow_pickle=True)
    track_ids_all = npz["track_ids"]
    print(f"[r7.5] corpus: {len(track_ids_all)} tracks with embeddings")

    # Filter to tracks with mp3 on disk
    have = set()
    for f in args.audio_dir.glob("dz_*.mp3"):
        try:
            have.add(int(f.stem.removeprefix("dz_")))
        except ValueError:
            continue
    mask = np.array([int(t) in have for t in track_ids_all])
    track_ids = track_ids_all[mask]
    print(f"[r7.5] aligned with audio on disk: {len(track_ids)}")

    args.out_dir.mkdir(parents=True, exist_ok=True)
    summary = []
    for axis in axes:
        s = run_axis(axis, template, track_ids, args.audio_dir, args.out_dir,
                     n_calls, bootstrap_fraction, args.workers, rng)
        print(f"[axis:{axis['id']}] DONE  ok={s['n_ok']} "
              f"parse_fail={s['n_parse_fail']} infer_fail={s['n_infer_fail']} "
              f"unique_tracks={s['n_unique_tracks']} wall={s['wall_min']:.1f}min")
        summary.append(s)

    summary_path = args.out_dir / "_summary.json"
    summary_path.write_text(json.dumps(summary, indent=2))
    print(f"\n[r7.5] all axes done; summary at {summary_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
