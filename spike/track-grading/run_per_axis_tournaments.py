"""Round-7 per-axis pairwise LLM tournaments via vLLM Qwen3-Omni.

For each of the k axes defined in `round7_axis_prompts.json`:
  1. Sample ~N pairs from the corpus (community-aware + bilateral).
  2. For each directed pair (a, b), POST the per-axis prompt + 30 s WAVs
     of clip A and clip B to the local vLLM server.
  3. Cache each judgement as JSON in
     `/tmp/track-grading/round7_pairs/<axis_id>/<a>_vs_<b>.json`.

Runs sequentially over axes, parallelism inside an axis (vLLM batches
async-engine inflight requests).

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/run_per_axis_tournaments.py
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/run_per_axis_tournaments.py --axes aggression distortion
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/run_per_axis_tournaments.py --pairs-per-axis 1500
"""
from __future__ import annotations

import argparse
import base64
import io
import json
import os
import random
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


SAMPLE_RATE = 16_000  # vLLM Qwen3-Omni input
PREVIEW_SECS = 30
MODEL_NAME = "cpatonn/Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit"
VLLM_URL = os.environ.get("VLLM_URL", "http://localhost:8000/v1/chat/completions")
HEALTH_URL = VLLM_URL.rsplit("/v1/", 1)[0] + "/health"
REQUEST_TIMEOUT = 240


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--prompts-file", type=Path,
                   default=Path("spike/track-grading/round7_axis_prompts.json"))
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/tmp/track-grading/audio"))
    p.add_argument("--embeddings", type=Path,
                   default=Path("/tmp/track-grading/embeddings/corpus_muq_mulan.npz"),
                   help="used for community-aware sampling (kmeans on embeddings)")
    p.add_argument("--out-dir", type=Path,
                   default=Path("/tmp/track-grading/round7_pairs"))
    p.add_argument("--pairs-per-axis", type=int, default=None,
                   help="override n_pairs_per_axis from prompts file")
    p.add_argument("--n-clusters", type=int, default=24,
                   help="kmeans k for community-aware sampling")
    p.add_argument("--axes", nargs="*", default=None,
                   help="restrict to these axis ids (default: all)")
    p.add_argument("--workers", type=int, default=12,
                   help="concurrent vLLM inflight requests")
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--smoke", type=int, default=0,
                   help="if >0, run only this many pairs per axis")
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


def parse_choice(text: str) -> str | None:
    if not text:
        return None
    first_line = text.strip().split("\n")[0].strip()
    m = re.match(r"^[\s\W]*([A-Z]+|equal|EQUAL)", first_line, re.IGNORECASE)
    if not m:
        return None
    tok = m.group(1).upper()
    if tok.startswith("A") and not tok.startswith("EQ"):
        return "A"
    if tok.startswith("B"):
        return "B"
    if tok.startswith("EQ"):
        return "EQUAL"
    return None


def load_audio_b64(path: Path) -> str | None:
    try:
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


def vllm_judge(prompt_text: str, b64_a: str, b64_b: str) -> str:
    payload = {
        "model": MODEL_NAME,
        "messages": [
            {"role": "system", "content": "You are an expert music analyst."},
            {"role": "user", "content": [
                {"type": "input_audio",
                 "input_audio": {"data": b64_a, "format": "wav"}},
                {"type": "input_audio",
                 "input_audio": {"data": b64_b, "format": "wav"}},
                {"type": "text", "text": prompt_text},
            ]},
        ],
        "max_tokens": 80,
        "temperature": 0.0,
    }
    r = requests.post(VLLM_URL, json=payload, timeout=REQUEST_TIMEOUT)
    r.raise_for_status()
    return r.json()["choices"][0]["message"]["content"]


def community_sample_pairs(track_ids: np.ndarray,
                           embeddings: np.ndarray,
                           n_pairs: int,
                           n_clusters: int,
                           rng: random.Random) -> list[tuple[int, int]]:
    """Generate bilateral pairs balanced across clusters.

    Strategy:
      - kmeans on L2-normalised embeddings (cosine clusters).
      - 50% intra-cluster pairs (similar tracks → fine-grained discrimination).
      - 50% inter-cluster pairs (across clusters → broad coverage).
      - Each undirected pair is emitted twice, A→B and B→A, to cancel
        positional bias (Bradley-Terry handles direction during MM iter).
    """
    from sklearn.cluster import KMeans

    n = len(track_ids)
    if n_clusters > n // 4:
        n_clusters = max(2, n // 4)

    # L2-normalise so kmeans on sklearn ≈ cosine clustering.
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    embn = embeddings / np.clip(norms, 1e-9, None)
    print(f"[sample] kmeans k={n_clusters} on {n} embeddings...")
    km = KMeans(n_clusters=n_clusters, n_init=4, random_state=rng.randint(0, 1<<30))
    labels = km.fit_predict(embn)
    cluster_to_ids: dict[int, list[int]] = {}
    for tid, lab in zip(track_ids, labels):
        cluster_to_ids.setdefault(int(lab), []).append(int(tid))
    sizes = sorted([len(v) for v in cluster_to_ids.values()])
    print(f"[sample] cluster sizes: min={sizes[0]} median={sizes[len(sizes)//2]} max={sizes[-1]}")

    # Number of UNIQUE undirected pairs we need; bilateral doubles it.
    n_undirected = n_pairs // 2
    n_intra = n_undirected // 2
    n_inter = n_undirected - n_intra

    seen: set[tuple[int, int]] = set()
    pairs: list[tuple[int, int]] = []

    # Intra: pick a random cluster (weighted by size), sample 2 distinct tracks.
    cluster_ids = list(cluster_to_ids.keys())
    cluster_weights = [len(cluster_to_ids[c]) for c in cluster_ids]
    tries = 0
    while len(pairs) < n_intra and tries < n_intra * 50:
        tries += 1
        c = rng.choices(cluster_ids, weights=cluster_weights, k=1)[0]
        pool = cluster_to_ids[c]
        if len(pool) < 2:
            continue
        a, b = rng.sample(pool, 2)
        key = (min(a, b), max(a, b))
        if key in seen:
            continue
        seen.add(key)
        pairs.append((a, b))

    # Inter: pick two distinct clusters, one track from each.
    tries = 0
    while len(pairs) < n_intra + n_inter and tries < (n_inter * 50):
        tries += 1
        if len(cluster_ids) < 2:
            break
        c1, c2 = rng.sample(cluster_ids, 2)
        a = rng.choice(cluster_to_ids[c1])
        b = rng.choice(cluster_to_ids[c2])
        key = (min(a, b), max(a, b))
        if key in seen:
            continue
        seen.add(key)
        pairs.append((a, b))

    print(f"[sample] generated {len(pairs)} undirected pairs "
          f"({n_intra} intra-target / {n_inter} inter-target)")

    # Bilateral expansion: each (a,b) emits A→B and B→A.
    directed: list[tuple[int, int]] = []
    for a, b in pairs:
        directed.append((a, b))
        directed.append((b, a))
    rng.shuffle(directed)
    return directed


def pair_cache(out_dir: Path, axis_id: str, a: int, b: int) -> Path:
    pdir = out_dir / axis_id
    pdir.mkdir(parents=True, exist_ok=True)
    return pdir / f"{a}_vs_{b}.json"


def run_axis(axis: dict, judge_template: str,
             pairs: list[tuple[int, int]],
             audio_dir: Path, out_dir: Path,
             workers: int) -> dict:
    """Returns per-axis stats."""
    aid = axis["id"]
    label = axis["label"]
    question = axis["question"]
    prompt_text = judge_template.format(question=question, axis_label=label)

    pending = [(a, b) for a, b in pairs
               if not pair_cache(out_dir, aid, a, b).exists()]
    print(f"\n[axis:{aid}] {len(pending)}/{len(pairs)} pending after resume filter")
    if not pending:
        return {"axis": aid, "n_total": len(pairs), "n_done": len(pairs),
                "n_failed": 0, "wall_min": 0.0}

    # Audio cache: we'll see the same track many times across pairs; cache
    # the b64 WAV bytes so we don't re-decode.
    cache_lock = threading.Lock()
    audio_cache: dict[int, str] = {}

    def get_b64(tid: int) -> str | None:
        with cache_lock:
            if tid in audio_cache:
                return audio_cache[tid]
        b64 = load_audio_b64(audio_dir / f"dz_{tid}.mp3")
        with cache_lock:
            if b64 is not None:
                # bound cache size to avoid OOM (each WAV is ~960 KB raw,
                # ~1.3 MB b64). Cap at 4000 entries → ~5 GB. The 15k-track
                # corpus + 3000 unique pairs × 2 directions means we'll
                # touch <6000 unique tracks per axis anyway.
                if len(audio_cache) > 4000:
                    audio_cache.pop(next(iter(audio_cache)))
                audio_cache[tid] = b64
        return b64

    state = {"done": 0, "failed": 0, "infer_fail": 0, "decode_fail": 0,
             "last": None, "choices": {"A": 0, "B": 0, "EQUAL": 0, None: 0}}
    state_lock = threading.Lock()
    start = time.time()

    def process(a: int, b: int) -> None:
        cache = pair_cache(out_dir, aid, a, b)
        t0 = time.time()
        b64_a = get_b64(a)
        b64_b = get_b64(b)
        if b64_a is None or b64_b is None:
            with state_lock:
                state["decode_fail"] += 1; state["failed"] += 1; state["done"] += 1
                state["last"] = (a, b, "DECODE_FAIL")
            return
        try:
            text = vllm_judge(prompt_text, b64_a, b64_b)
        except Exception as e:
            with state_lock:
                state["infer_fail"] += 1; state["failed"] += 1; state["done"] += 1
                state["last"] = (a, b, f"INFER_FAIL:{str(e)[:40]}")
            return
        choice = parse_choice(text)
        winner = a if choice == "A" else (b if choice == "B" else None)
        record = {
            "axis": aid, "pair": [min(a, b), max(a, b)],
            "presented_a": a, "presented_b": b,
            "model": MODEL_NAME, "raw_response": text,
            "choice": choice, "winner_id": winner,
            "wall_time_s": round(time.time() - t0, 2),
            "ts": int(time.time()),
        }
        tmp = cache.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(record, indent=2))
        tmp.rename(cache)
        with state_lock:
            state["done"] += 1
            state["choices"][choice] = state["choices"].get(choice, 0) + 1
            state["last"] = (a, b, choice)
            d = state["done"]
            if d % 50 == 0 or d == len(pending):
                el = time.time() - start
                rate = d / max(el, 0.001)
                eta = (len(pending) - d) / max(rate, 0.001)
                print(f"  [{aid}] {d}/{len(pending)} "
                      f"A={state['choices']['A']} B={state['choices']['B']} "
                      f"EQ={state['choices']['EQUAL']} ?={state['choices'].get(None, 0)} "
                      f"({rate:.2f}/s, eta {eta/60:.1f}min, fail={state['failed']})")

    print(f"  [{aid}] running {workers} concurrent workers ...")
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futures = [ex.submit(process, a, b) for a, b in pending]
        for _ in as_completed(futures):
            pass

    return {
        "axis": aid,
        "n_total": len(pairs),
        "n_done": state["done"],
        "n_failed": state["failed"],
        "n_infer_fail": state["infer_fail"],
        "n_decode_fail": state["decode_fail"],
        "n_A": state["choices"]["A"], "n_B": state["choices"]["B"],
        "n_EQUAL": state["choices"]["EQUAL"],
        "n_unparsed": state["choices"].get(None, 0),
        "wall_min": (time.time() - start) / 60,
    }


def main() -> int:
    args = parse_args()
    rng = random.Random(args.seed)

    cfg = json.loads(args.prompts_file.read_text())
    template = cfg["_meta"]["judge_template"]
    n_pairs = args.pairs_per_axis or cfg["_meta"].get("n_pairs_per_axis", 1500)
    if args.smoke:
        n_pairs = args.smoke

    axes = cfg["axes"]
    if args.axes:
        axes = [a for a in axes if a["id"] in args.axes]
        if not axes:
            sys.exit(f"no axes match: {args.axes}")
    print(f"[r7] {len(axes)} axes, {n_pairs} directed pairs per axis")

    print(f"[r7] waiting for vLLM at {VLLM_URL} ...")
    if not wait_for_endpoint(timeout_s=900):
        sys.exit(f"vLLM endpoint never came up at {VLLM_URL} — start serve_qwen3_omni.sh")
    print("[r7] vLLM ready")

    if not args.embeddings.exists():
        sys.exit(f"missing {args.embeddings} — run embed_corpus_mulan.py first")
    npz = np.load(args.embeddings, allow_pickle=True)
    track_ids = npz["track_ids"]
    embs = npz["embeddings"]
    print(f"[r7] corpus: {len(track_ids)} tracks with embeddings")

    # Filter to tracks that have an MP3 on disk.
    have_mp3 = set()
    for f in args.audio_dir.glob("dz_*.mp3"):
        try:
            have_mp3.add(int(f.stem.removeprefix("dz_")))
        except ValueError:
            continue
    mask = np.array([int(t) in have_mp3 for t in track_ids])
    track_ids = track_ids[mask]
    embs = embs[mask]
    print(f"[r7] aligned with audio on disk: {len(track_ids)}")

    pairs = community_sample_pairs(track_ids, embs, n_pairs,
                                   args.n_clusters, rng)
    print(f"[r7] {len(pairs)} directed pairs (bilateral)")

    args.out_dir.mkdir(parents=True, exist_ok=True)
    summary = []
    for axis in axes:
        s = run_axis(axis, template, pairs, args.audio_dir, args.out_dir,
                     args.workers)
        print(f"[axis:{axis['id']}] DONE  done={s['n_done']} "
              f"fail={s['n_failed']} wall={s['wall_min']:.1f}min")
        summary.append(s)

    summary_path = args.out_dir / "_summary.json"
    summary_path.write_text(json.dumps(summary, indent=2))
    print(f"\n[r7] all axes done; summary at {summary_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
