"""Round-7.6 judge-agnostic K=4 N-way ranking tournament.

Drop-in successor to run_nway_tournaments_r7_5.py. Same parallelism
design (12-16 inflight workers + continuous BT refit thread + lock-free
BALD picks + atomic JSON cache) but pluggable judge backend.

Modes:
  --judge {qwen3_omni|music_flamingo}
       which audio-LM does the ranking
  --pairs-source {bald|reuse-existing}
       bald: do uncertainty sampling on the corpus (round-7.5 default)
       reuse-existing: re-judge the same K-tuples that round-7.5 already
                       hit with Qwen3-Omni — direct judge-vs-judge
                       comparison, cheapest path
  --pairs-subset {all|uncertain-N}
       (only for reuse-existing) which subset of the existing pairs to
       re-judge: 'all' = 192k, 'uncertain-N' = top-N highest-BT-variance

Outputs:
  /home/data01/Music/mesh-track-grading/round7_6_pairs/<judge_id>/<axis_id>/<sorted_track_ids>.json
  per-call ranking + raw response + timing, atomic write via tmp+rename

Usage examples:
  # smoke: 50 reused pairs through Music Flamingo
  python run_judge_tournament.py \
      --judge music_flamingo --pairs-source reuse-existing \
      --pairs-subset uncertain-50 --axes timbre_roughness

  # production E1: 20k uncertain pairs across all 16 axes
  python run_judge_tournament.py \
      --judge music_flamingo --pairs-source reuse-existing \
      --pairs-subset uncertain-20000

  # full re-judge of all 192k pairs (longest run)
  python run_judge_tournament.py \
      --judge music_flamingo --pairs-source reuse-existing \
      --pairs-subset all
"""
from __future__ import annotations

import argparse
import json
import math
import random
import sys
import threading
import time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Sequence

import numpy as np

# Local imports — judges/ package
sys.path.insert(0, str(Path(__file__).parent))
from judges import Judge, RankingResult, ParseError, InferenceError, K, LETTERS
from judges.qwen3_omni import Qwen3OmniJudge
from judges.music_flamingo import MusicFlamingoJudge


# ───────────────────────────────────────────────────────────────────
# Tunables (carried over from round-7.5 with knowledge gained there)
# ───────────────────────────────────────────────────────────────────
PREVIEW_SECS = 30
WORKER_COUNT_DEFAULT = 12  # match round-7.5; vLLM saturates here on Qwen3
BALD_WORKING_SET = 500
BT_REFIT_TOP_TRACKS = 8000
CONTINUOUS_REFIT_GAP_SEC = 0.5
CONTINUOUS_REFIT_MIN_PAIRS = 200


# ───────────────────────────────────────────────────────────────────
# CLI
# ───────────────────────────────────────────────────────────────────
def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--judge", choices=["qwen3_omni", "music_flamingo"],
                   default="music_flamingo")
    p.add_argument("--pairs-source",
                   choices=["bald", "reuse-existing"],
                   default="reuse-existing",
                   help="bald = pick fresh tuples; reuse-existing = use the "
                        "tuples Qwen3-Omni already judged in round-7.5")
    p.add_argument("--pairs-subset", default="uncertain-20000",
                   help="all | uncertain-N | smoke-N (random)")
    p.add_argument("--axes", nargs="*", default=None,
                   help="restrict to these axis ids (default: all 16)")
    p.add_argument("--prompts-file", type=Path,
                   default=Path("spike/track-grading/round7_5_axis_prompts.json"))
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/audio"))
    p.add_argument("--existing-pairs-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_5_pairs"),
                   help="source dir for reuse-existing mode")
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_6_pairs"))
    p.add_argument("--bt-priors", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round7_5_priors.npz"),
                   help="round-7.5 BT priors — used for uncertainty sampling")
    p.add_argument("--workers", type=int, default=WORKER_COUNT_DEFAULT)
    p.add_argument("--max-tokens", type=int, default=80)
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--health-check-only", action="store_true",
                   help="ping the judge endpoint and exit")
    return p.parse_args()


# ───────────────────────────────────────────────────────────────────
# Judge factory
# ───────────────────────────────────────────────────────────────────
def make_judge(judge_id: str) -> Judge:
    if judge_id == "qwen3_omni":
        return Qwen3OmniJudge()
    elif judge_id == "music_flamingo":
        return MusicFlamingoJudge()
    raise ValueError(f"unknown judge_id: {judge_id}")


# ───────────────────────────────────────────────────────────────────
# Audio loading (judge-aware sample rate)
# ───────────────────────────────────────────────────────────────────
import soundfile as sf
import librosa


def load_audio(path: Path, sr: int) -> np.ndarray | None:
    try:
        wav, _ = librosa.load(str(path), sr=sr, mono=True, duration=PREVIEW_SECS)
    except Exception as e:
        print(f"[load] {path.name}: {e}", file=sys.stderr)
        return None
    target = sr * PREVIEW_SECS
    if len(wav) < target:
        wav = np.pad(wav, (0, target - len(wav)))
    elif len(wav) > target:
        wav = wav[:target]
    return wav.astype(np.float32)


# ───────────────────────────────────────────────────────────────────
# Cache paths + tuple keys (compatible with round-7.5)
# ───────────────────────────────────────────────────────────────────
def tuple_key(track_tuple: Sequence[int]) -> str:
    return "_".join(str(t) for t in sorted(track_tuple))


def call_cache_path(out_dir: Path, judge_id: str, axis_id: str,
                    track_tuple: Sequence[int]) -> Path:
    pdir = out_dir / judge_id / axis_id
    pdir.mkdir(parents=True, exist_ok=True)
    return pdir / f"{tuple_key(track_tuple)}.json"


# ───────────────────────────────────────────────────────────────────
# Pair-source: re-use existing round-7.5 tuples
# ───────────────────────────────────────────────────────────────────
def load_existing_tuples(axis_dir: Path) -> list[tuple[int, ...]]:
    """Load all sorted track-tuples already judged in round-7.5 for an axis."""
    tuples: list[tuple[int, ...]] = []
    for f in axis_dir.glob("*.json"):
        try:
            parts = f.stem.split("_")
            if len(parts) != K:
                continue
            tup = tuple(int(p) for p in parts)
            tuples.append(tup)
        except Exception:
            continue
    return tuples


def select_uncertain_tuples(
    tuples: list[tuple[int, ...]],
    bt_scores_for_axis: dict[int, float],
    n: int,
    rng: random.Random,
) -> list[tuple[int, ...]]:
    """Pick N tuples where the current BT model is most uncertain.

    Uncertainty = 1 / (1 + std(log_scores)) — same metric the BALD
    picker uses inside the runner. Tuples with similar tracks (close BT
    scores) score high here; tuples with one outlier score lower.
    """
    if n >= len(tuples):
        return list(tuples)
    log_scores = {t: math.log(max(s, 1e-9)) for t, s in bt_scores_for_axis.items()}
    scored: list[tuple[float, tuple[int, ...]]] = []
    for tup in tuples:
        ls = [log_scores.get(t) for t in tup]
        if any(x is None for x in ls):
            # unscored track — also high information value
            unc = 1.0
        else:
            mean = sum(ls) / len(ls)
            std = math.sqrt(sum((x - mean) ** 2 for x in ls) / len(ls))
            unc = 1.0 / (1.0 + std)
        scored.append((unc, tup))
    # Sort high-uncertainty first, then break ties randomly for diversity.
    rng.shuffle(scored)
    scored.sort(key=lambda x: -x[0])
    return [tup for _, tup in scored[:n]]


# ───────────────────────────────────────────────────────────────────
# Bradley-Terry MM solver (same as round-7.5, vectorised, float32)
# ───────────────────────────────────────────────────────────────────
def bt_mm(wins: dict[tuple[int, int], float],
          tracks: list[int],
          max_iter: int = 80,
          tol: float = 1e-6,
          prior_strength: float = 1.0,
          dtype=np.float32) -> dict[int, float]:
    if not tracks:
        return {}
    idx = {t: i for i, t in enumerate(tracks)}
    n = len(tracks)
    W = np.zeros((n, n), dtype=dtype)
    for (a, b), w in wins.items():
        if a in idx and b in idx:
            W[idx[a], idx[b]] += w
    N_mat = W + W.T
    W_row = W.sum(axis=1)
    a_prior = dtype(1.0 + prior_strength)
    b_prior = dtype(prior_strength)
    s = np.ones(n, dtype=dtype)
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


def ranking_to_pairs(ranking: tuple[str, ...],
                     letter_to_track: dict[str, int]
                     ) -> list[tuple[int, int]]:
    """K=4 ordering low→high → 6 pairwise (winner, loser) observations.

    For each i<j in ranking, ranking[j] (higher index = closer to HIGH
    pole) wins over ranking[i].
    """
    pairs: list[tuple[int, int]] = []
    for i in range(len(ranking)):
        for j in range(i + 1, len(ranking)):
            lo, hi = ranking[i], ranking[j]
            pairs.append((letter_to_track[hi], letter_to_track[lo]))
    return pairs


# ───────────────────────────────────────────────────────────────────
# Per-axis tournament (sequential across axes, parallel within)
# ───────────────────────────────────────────────────────────────────
def run_axis(
    *,
    axis: dict,
    judge_template: str,
    judge: Judge,
    track_tuples: list[tuple[int, ...]],
    audio_dir: Path,
    out_dir: Path,
    workers: int,
    max_tokens: int,
    seed: int,
) -> dict:
    aid = axis["id"]
    low_pole = axis["low_pole"]
    high_pole = axis["high_pole"]
    prompt_text = judge_template.format(
        axis_id=aid, low_pole=low_pole, high_pole=high_pole)

    rng = random.Random(seed + hash(aid) % (2**16))
    rng_np = np.random.default_rng(seed + hash(aid) % (2**16))

    # Resume scan: skip tuples we've already judged with this judge.
    pending = []
    n_resumed = 0
    for tup in track_tuples:
        if call_cache_path(out_dir, judge.judge_id, aid, tup).exists():
            n_resumed += 1
        else:
            pending.append(tup)
    print(f"[{aid}] {n_resumed} resumed, {len(pending)} pending "
          f"(judge={judge.judge_id})")

    if not pending:
        return {"axis": aid, "judge": judge.judge_id,
                "n_total": len(track_tuples), "n_done": n_resumed,
                "n_ok": 0, "n_parse_fail": 0, "n_infer_fail": 0,
                "wall_min": 0.0}

    # Audio cache: each track decoded once across all calls touching it.
    cache_lock = threading.Lock()
    audio_cache: dict[int, np.ndarray] = {}

    def get_audio(tid: int) -> np.ndarray | None:
        with cache_lock:
            if tid in audio_cache:
                return audio_cache[tid]
        wav = load_audio(audio_dir / f"dz_{tid}.mp3", judge.sample_rate)
        if wav is None:
            return None
        with cache_lock:
            # bound cache: BALD working set is ~500, but keep room for
            # spillage — 4000 entries × 30s × 16kHz × 4 = 2 GB
            if len(audio_cache) > 4000:
                audio_cache.pop(next(iter(audio_cache)))
            audio_cache[tid] = wav
        return wav

    # State for stats / progress
    state_lock = threading.Lock()
    state = {"ok": 0, "done": 0,
             "parse_fail": 0, "infer_fail": 0, "decode_fail": 0,
             "last_choice": None}
    start = time.time()

    def process_tuple(tup: tuple[int, ...]) -> None:
        cache = call_cache_path(out_dir, judge.judge_id, aid, tup)
        # Race: another worker might have written this since the resume scan.
        if cache.exists():
            with state_lock:
                state["done"] += 1
            return
        audios = [get_audio(t) for t in tup]
        if any(a is None for a in audios):
            with state_lock:
                state["decode_fail"] += 1
                state["done"] += 1
            return
        # Random presentation order to wash positional bias
        with cache_lock:  # rng_np is not thread-safe
            order = list(LETTERS)
            rng_np.shuffle(order)
        # Shuffle the audio array order to match the letter shuffle:
        # letter_to_track defines what the LLM sees, we want letter[i]
        # to point to track[order_idx[i]]. Easier: pass tup unchanged,
        # then shuffle who-gets-which-letter inside the judge.
        try:
            res = judge.rank(
                tup, audios, prompt_text,
                presentation_order=order,
                max_tokens=max_tokens,
            )
        except ParseError as e:
            with state_lock:
                state["parse_fail"] += 1
                state["done"] += 1
            # Persist the failure so resume skips it on retry. Keeps run
            # deterministic — bad pairs get dropped, BT just loses one obs.
            tmp = cache.with_suffix(".json.tmp")
            tmp.write_text(json.dumps({
                "tuple_key": tuple_key(tup),
                "track_tuple": list(tup),
                "judge": judge.judge_id,
                "error": "parse_error",
                "error_detail": str(e),
                "ts": int(time.time()),
            }, indent=2))
            tmp.rename(cache)
            return
        except InferenceError as e:
            # Don't cache infer failures — let them retry next run.
            with state_lock:
                state["infer_fail"] += 1
                state["done"] += 1
            print(f"[{aid}] infer fail on {tuple_key(tup)}: {e}", file=sys.stderr)
            return

        # Compute pairs for downstream BT (storing helps debug)
        pairs = ranking_to_pairs(res.ranking_low_to_high, res.letter_to_track)
        record = {
            "tuple_key": tuple_key(tup),
            "track_tuple": list(tup),
            "axis": aid,
            "judge": res.judge_id,
            "model": judge.model_name,
            "letter_to_track": {k: int(v) for k, v in res.letter_to_track.items()},
            "presentation_order": list(order),
            "ranking_low_to_high": list(res.ranking_low_to_high),
            "pairwise_observations": [[int(w), int(l)] for w, l in pairs],
            "raw_response": res.raw_response,
            "wall_time_s": res.wall_time_s,
            "ts": int(time.time()),
        }
        tmp = cache.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(record, indent=2))
        tmp.rename(cache)

        with state_lock:
            state["ok"] += 1
            state["done"] += 1
            d = state["done"]
            if d % 50 == 0 or d == len(pending):
                el = time.time() - start
                rate = state["ok"] / max(el, 0.001)
                eta = (len(pending) - d) / max(rate, 0.001)
                print(f"  [{aid}] {d}/{len(pending)} "
                      f"ok={state['ok']} parse_fail={state['parse_fail']} "
                      f"infer_fail={state['infer_fail']} "
                      f"({rate:.2f}/s, eta {eta/60:.1f}min)")

    print(f"  [{aid}] running {workers} concurrent workers ...")
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futures = [ex.submit(process_tuple, t) for t in pending]
        for _ in as_completed(futures):
            pass

    return {
        "axis": aid,
        "judge": judge.judge_id,
        "n_total": len(track_tuples),
        "n_resumed": n_resumed,
        "n_done": state["done"],
        "n_ok": state["ok"],
        "n_parse_fail": state["parse_fail"],
        "n_infer_fail": state["infer_fail"],
        "n_decode_fail": state["decode_fail"],
        "wall_min": (time.time() - start) / 60,
    }


# ───────────────────────────────────────────────────────────────────
# Main
# ───────────────────────────────────────────────────────────────────
def main() -> int:
    args = parse_args()

    # 1) Build judge + health check
    judge = make_judge(args.judge)
    print(f"[r7.6] judge: {judge.judge_id} ({judge.model_name})")
    print(f"[r7.6] checking endpoint at {judge.url} ...")
    if not judge.is_alive():
        print(f"[r7.6] judge endpoint not responding; start the serve script", file=sys.stderr)
        return 1
    print(f"[r7.6] judge ready")
    if args.health_check_only:
        return 0

    # 2) Load axis prompts
    cfg = json.loads(args.prompts_file.read_text())
    template = cfg["_meta"]["judge_template"]
    axes = cfg["axes"]
    if args.axes:
        axes = [a for a in axes if a["id"] in args.axes]
        if not axes:
            sys.exit(f"no axes match: {args.axes}")
    print(f"[r7.6] {len(axes)} axes to judge")

    # 3) Resolve pair source per axis
    rng = random.Random(args.seed)

    if args.pairs_source == "reuse-existing":
        # Optionally load BT priors for uncertainty sampling
        bt_priors_per_axis: dict[str, dict[int, float]] = {}
        if args.pairs_subset.startswith("uncertain-"):
            if not args.bt_priors.exists():
                sys.exit(f"missing {args.bt_priors} (needed for uncertainty sampling)")
            pri = np.load(args.bt_priors, allow_pickle=True)
            axis_names = list(pri["axes"])
            track_ids = pri["track_ids"]
            scores = pri["priors_0_10"].astype(np.float32)
            for i, name in enumerate(axis_names):
                bt_priors_per_axis[name] = {
                    int(t): float(scores[i, j]) for j, t in enumerate(track_ids)
                    if scores[i, j] > 0
                }

        per_axis_tuples: dict[str, list[tuple[int, ...]]] = {}
        for axis in axes:
            aid = axis["id"]
            src = args.existing_pairs_dir / aid
            if not src.is_dir():
                print(f"[r7.6] WARNING: {src} missing; skipping axis {aid}",
                      file=sys.stderr)
                per_axis_tuples[aid] = []
                continue
            all_tuples = load_existing_tuples(src)
            if args.pairs_subset == "all":
                selected = all_tuples
            elif args.pairs_subset.startswith("uncertain-"):
                n = int(args.pairs_subset.split("-")[1])
                selected = select_uncertain_tuples(
                    all_tuples, bt_priors_per_axis.get(aid, {}), n, rng)
            elif args.pairs_subset.startswith("smoke-"):
                n = int(args.pairs_subset.split("-")[1])
                shuf = list(all_tuples)
                rng.shuffle(shuf)
                selected = shuf[:n]
            else:
                sys.exit(f"unknown --pairs-subset: {args.pairs_subset}")
            per_axis_tuples[aid] = selected
            print(f"[r7.6] axis={aid}: {len(all_tuples)} existing → "
                  f"{len(selected)} selected ({args.pairs_subset})")
    elif args.pairs_source == "bald":
        # Future-work: full BALD scheduler, mirrors round-7.5 logic.
        # Not needed for the user's stated scope (re-judge existing).
        sys.exit("--pairs-source bald not implemented in r7.6 yet — use "
                 "reuse-existing for the judge-swap experiment")

    # 4) Run each axis
    args.out_dir.mkdir(parents=True, exist_ok=True)
    summary = []
    grand_start = time.time()
    for axis in axes:
        aid = axis["id"]
        tuples = per_axis_tuples.get(aid, [])
        if not tuples:
            print(f"[axis:{aid}] no tuples; skipping")
            continue
        s = run_axis(
            axis=axis,
            judge_template=template,
            judge=judge,
            track_tuples=tuples,
            audio_dir=args.audio_dir,
            out_dir=args.out_dir,
            workers=args.workers,
            max_tokens=args.max_tokens,
            seed=args.seed,
        )
        print(f"[axis:{aid}] DONE  ok={s['n_ok']} fail={s['n_parse_fail']} "
              f"wall={s['wall_min']:.1f}min")
        summary.append(s)

    summary_path = args.out_dir / f"_{judge.judge_id}_summary.json"
    summary_path.write_text(json.dumps({
        "judge": judge.judge_id,
        "model": judge.model_name,
        "pairs_source": args.pairs_source,
        "pairs_subset": args.pairs_subset,
        "wall_min": (time.time() - grand_start) / 60,
        "axes": summary,
    }, indent=2))
    print(f"\n[r7.6] all axes done; summary at {summary_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
