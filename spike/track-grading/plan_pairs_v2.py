"""Hierarchical + active-learning pair planner — Python port of the Rust
calibration UI logic in `crates/mesh-core/src/suggestions/aggression.rs`.

Round-4 used 3 hand-picked anchors → 6 games per non-anchor track → 871/909
tracks pile in BT 8-9 (saturated, undifferentiable). This planner replaces
that with what the existing UI already does for human users:

  Phase 1 (deterministic bootstrap, asked first):
    Tier A — cross-community extreme pairs: centroids of communities with
             biggest BT-prior gap, capturing the global rank scaffold.
    Tier B — per-community centroid × farthest-rep pair: pins each
             community's internal scale.
    Tier C — intra-community skip-chains for large communities: e0–e2,
             e2–centroid, centroid–e1 (4 reps per chain). This is the
             within-cluster refinement the round-4 doc flagged as missing.

  Phase 2 (active learning queue, asked after Phase 1):
    Score = prior_factor * ||Δ_features||^2 * diversity_decay
      prior_factor = 1 + 1.5 * |bt_prior(a) - bt_prior(b)| / 10
      diversity_decay = 0.85^(# recent touches in {community(a), community(b)})
    Skipped via transitive-closure pruning: if a known chain already
    determines a > b, drop the (a,b) query.

Hybrid embedding filter:
    Pairs with |V11 intensity diff| > HYBRID_SKIP are confidently ranked by
    the embedding alone; we drop them from the LLM queue.
    Pairs with |V11 intensity diff| < HYBRID_PRIORITY are exactly where
    the LLM is most informative; we boost their score by 1.5x.

Inputs:
  - documents/axis-eval-results/V11_neuro_dnb_tuned.csv (per-track 6-axis
    features + intensity column)
  - documents/axis-eval-results/llm-pair-priors.txt (round-4 BT priors as
    cold-start seeds)
  - /home/data01/Music/mesh-track-grading/pairs_vllm/*.json (round-4 cached judgments — used
    for transitive-closure pruning)

Outputs:
  - /home/data01/Music/mesh-track-grading/round5_plan.csv  (track_a, track_b, tier, score)
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from collections import defaultdict, deque
from pathlib import Path

import numpy as np
from sklearn.cluster import KMeans
from sklearn.neighbors import NearestNeighbors


N_COMMUNITIES = 12
N_REPS_PER_COMMUNITY = 5  # 1 centroid + 4 farthest-point edges
HYBRID_SKIP = 0.20      # |V11 intensity diff| above this → skip LLM (axis decides)
HYBRID_PRIORITY = 0.05  # below this → boost LLM priority (LLM most informative)
PHASE2_BUDGET = 2500     # max number of phase-2 pairs to emit


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--features", type=Path,
                   default=Path("documents/axis-eval-results/V11_neuro_dnb_tuned.csv"))
    p.add_argument("--bt-priors", type=Path,
                   default=Path("documents/axis-eval-results/llm-pair-priors.txt"))
    p.add_argument("--existing-pairs", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/pairs_vllm"))
    p.add_argument("--out", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/round5_plan.csv"))
    p.add_argument("--phase2-budget", type=int, default=PHASE2_BUDGET)
    return p.parse_args()


def load_features(p: Path) -> tuple[list[int], np.ndarray, dict[int, float]]:
    """Returns (track_ids, 6-axis feature matrix, intensity per track)."""
    cols = ["aggression", "distortion", "density", "darkness", "noisiness", "atonality"]
    track_ids: list[int] = []
    rows: list[list[float]] = []
    intensity: dict[int, float] = {}
    with p.open() as f:
        for r in csv.DictReader(f):
            tid = int(r["track_id"])
            track_ids.append(tid)
            rows.append([float(r[c]) for c in cols])
            intensity[tid] = float(r["intensity"])
    return track_ids, np.array(rows, dtype=np.float32), intensity


def load_bt_priors(p: Path) -> dict[int, float]:
    out: dict[int, float] = {}
    with p.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"): continue
            parts = line.split("|", 2)
            if len(parts) == 3:
                out[int(parts[0])] = float(parts[2])
    return out


def load_existing_pairs(d: Path) -> dict[frozenset[int], str]:
    """Returns {frozenset({a,b}): winner_id_str | 'EQUAL' | 'UNKNOWN'} for all
    cached pair judgments. Used for transitive-closure pruning so we don't
    re-ask pairs already deducible from past answers."""
    out: dict[frozenset[int], str] = {}
    if not d.exists(): return out
    for f in d.glob("*.json"):
        try:
            r = json.loads(f.read_text())
        except Exception:
            continue
        a, b = r["presented_a"], r["presented_b"]
        wid = r.get("winner_id")
        out.setdefault(frozenset({a, b}),
                       str(wid) if wid is not None else "EQUAL")
    return out


def farthest_point_sample(features: np.ndarray, idx_pool: list[int], k: int,
                          rng: np.random.Generator) -> list[int]:
    """Greedy k-center: pick k tracks that maximally cover the pool."""
    if len(idx_pool) <= k: return list(idx_pool)
    chosen = [int(rng.choice(idx_pool))]
    pool = [i for i in idx_pool if i != chosen[0]]
    while len(chosen) < k and pool:
        # distance from each pool point to its nearest chosen point
        chosen_arr = features[chosen]
        pool_arr = features[pool]
        d = np.min(((pool_arr[:, None, :] - chosen_arr[None, :, :]) ** 2).sum(-1),
                   axis=1)
        next_idx = int(np.argmax(d))
        chosen.append(pool[next_idx])
        pool.pop(next_idx)
    return chosen


def transitive_known(known_wins: dict[int, set[int]], a: int, b: int) -> bool:
    """Returns True if a > b or b > a is already deducible from known wins
    via transitive-closure (DFS reachability)."""
    def reaches(src: int, dst: int) -> bool:
        seen = {src}; stack = [src]
        while stack:
            x = stack.pop()
            for y in known_wins.get(x, ()):
                if y == dst: return True
                if y not in seen:
                    seen.add(y); stack.append(y)
        return False
    return reaches(a, b) or reaches(b, a)


def main() -> int:
    args = parse_args()
    rng = np.random.default_rng(42)

    track_ids, features, intensity = load_features(args.features)
    bt_priors = load_bt_priors(args.bt_priors)
    existing = load_existing_pairs(args.existing_pairs)
    print(f"[plan] {len(track_ids)} tracks, {features.shape[1]} features each, "
          f"{len(existing)} existing pair judgments")

    tid_to_idx = {t: i for i, t in enumerate(track_ids)}
    bt_arr = np.array([bt_priors.get(t, 5.0) for t in track_ids])

    # Normalize features for clustering (each axis 0-1).
    fmin = features.min(axis=0, keepdims=True)
    fmax = features.max(axis=0, keepdims=True)
    fnorm = (features - fmin) / (fmax - fmin + 1e-9)

    # Community detection: KMeans on the normalized 6-axis features.
    # Approximates Leiden well enough for our 909 tracks; no external deps.
    km = KMeans(n_clusters=N_COMMUNITIES, n_init=10, random_state=42)
    labels = km.fit_predict(fnorm)
    print(f"[plan] {N_COMMUNITIES} communities, sizes: " +
          ", ".join(str(int(c)) for c in np.bincount(labels)))

    # Per community: pick representatives (centroid + farthest-point edges).
    community_reps: dict[int, list[int]] = {}
    community_centroid_track: dict[int, int] = {}
    for c in range(N_COMMUNITIES):
        members = [i for i, l in enumerate(labels) if l == c]
        if not members: continue
        # Centroid track = closest to community centroid
        centroid_vec = fnorm[members].mean(axis=0)
        d_centroid = ((fnorm[members] - centroid_vec) ** 2).sum(-1)
        ctr_local = members[int(np.argmin(d_centroid))]
        # Farthest-point edges (k-1 more reps)
        edges = farthest_point_sample(fnorm, members,
                                      N_REPS_PER_COMMUNITY - 1, rng)
        edges = [e for e in edges if e != ctr_local][: N_REPS_PER_COMMUNITY - 1]
        community_reps[c] = [ctr_local] + edges
        community_centroid_track[c] = ctr_local

    # Build known_wins dict from existing pair cache for transitive pruning.
    known_wins: dict[int, set[int]] = defaultdict(set)
    for pair, winner in existing.items():
        if winner == "EQUAL" or winner == "UNKNOWN": continue
        try:
            wid = int(winner)
        except ValueError:
            continue
        a, b = tuple(pair)
        loser = b if wid == a else a
        known_wins[wid].add(loser)

    # Helper: emit a candidate pair (track_id_a, track_id_b, tier, score).
    plan: list[tuple[int, int, str, float]] = []
    seen_pairs: set[frozenset[int]] = set()

    def emit(a_idx: int, b_idx: int, tier: str, score: float):
        a, b = track_ids[a_idx], track_ids[b_idx]
        key = frozenset({a, b})
        if a == b or key in seen_pairs or key in existing:
            return
        # hybrid embedding filter using V11 intensity column
        diff = abs(intensity[a] - intensity[b])
        if diff > HYBRID_SKIP and tier.startswith("phase2"):
            return  # axis is confident — don't waste LLM on this
        if diff < HYBRID_PRIORITY:
            score *= 1.5
        # transitive pruning
        if transitive_known(known_wins, a, b):
            return
        seen_pairs.add(key)
        plan.append((a, b, tier, score))

    # ---- PHASE 1A: cross-community extreme pairs ----
    # For each ordered pair of communities, score by |centroid BT prior gap|.
    # Take top-K pairs of community centroids.
    community_centroid_prior: dict[int, float] = {
        c: float(bt_arr[ctr]) for c, ctr in community_centroid_track.items()
    }
    cross_pairs = []
    centroids = list(community_centroid_track.keys())
    for i in range(len(centroids)):
        for j in range(i + 1, len(centroids)):
            ci, cj = centroids[i], centroids[j]
            gap = abs(community_centroid_prior[ci] - community_centroid_prior[cj])
            cross_pairs.append((ci, cj, gap))
    cross_pairs.sort(key=lambda x: -x[2])
    for ci, cj, gap in cross_pairs[:8]:
        a_idx = community_centroid_track[ci]
        b_idx = community_centroid_track[cj]
        emit(a_idx, b_idx, "phase1a_cross", gap)

    # ---- PHASE 1B: per-community centroid × farthest-rep ----
    for c, reps in community_reps.items():
        if len(reps) < 2: continue
        centroid_local = reps[0]
        # farthest from centroid in feature space
        d_to_centroid = ((fnorm[reps[1:]] - fnorm[centroid_local]) ** 2).sum(-1)
        far_local = reps[1 + int(np.argmax(d_to_centroid))]
        emit(centroid_local, far_local, "phase1b_in_community", 1.0)

    # ---- PHASE 1C: intra-community skip-chains (big communities only) ----
    # For communities with ≥75 members and ≥4 reps: e0–e2, e2–centroid,
    # centroid–e1 (rep order is centroid, e1, e2, e3, e4 from farthest-point).
    for c, reps in community_reps.items():
        members = [i for i, l in enumerate(labels) if l == c]
        if len(members) < 75 or len(reps) < 4: continue
        centroid_local, e1, e2, e3 = reps[0], reps[1], reps[2], reps[3]
        emit(e1, e3, "phase1c_chain", 1.0)
        emit(e3, centroid_local, "phase1c_chain", 1.0)
        emit(centroid_local, e2, "phase1c_chain", 1.0)

    n_phase1 = len(plan)
    print(f"[plan] phase 1: {n_phase1} pairs")

    # ---- PHASE 2: active-learning queue ----
    # Generate a candidate pool of all intra-community pairs that haven't been
    # asked yet; score by BALD-like = prior_factor * ||Δ_features||^2.
    candidates: list[tuple[float, int, int, str]] = []
    for c, members in [(c, [i for i, l in enumerate(labels) if l == c])
                       for c in range(N_COMMUNITIES)]:
        # Sub-sample bigger communities to keep generation tractable.
        if len(members) > 80:
            sub = list(rng.choice(members, size=80, replace=False))
        else:
            sub = members
        for i in range(len(sub)):
            for j in range(i + 1, len(sub)):
                a_idx, b_idx = sub[i], sub[j]
                a, b = track_ids[a_idx], track_ids[b_idx]
                if frozenset({a, b}) in existing or frozenset({a, b}) in seen_pairs:
                    continue
                d2 = float(((fnorm[a_idx] - fnorm[b_idx]) ** 2).sum())
                gap = abs(bt_arr[a_idx] - bt_arr[b_idx])
                prior_factor = 1.0 + 1.5 * (gap / 10.0)
                # Bonus for tracks both in the saturated 8-9 cluster — that's
                # exactly where round-4 had no resolution.
                if bt_arr[a_idx] >= 8.0 and bt_arr[b_idx] >= 8.0:
                    prior_factor *= 2.0
                score = prior_factor * d2
                candidates.append((score, a_idx, b_idx, "phase2_intra"))

    # Also add cross-community pairs when V11 says they're close (high
    # informational value because the embedding doesn't decide).
    intra_count = len(candidates)
    cross_added = 0
    for c1 in range(N_COMMUNITIES):
        for c2 in range(c1 + 1, N_COMMUNITIES):
            m1 = [i for i, l in enumerate(labels) if l == c1][:8]
            m2 = [i for i, l in enumerate(labels) if l == c2][:8]
            for a_idx in m1:
                for b_idx in m2:
                    a, b = track_ids[a_idx], track_ids[b_idx]
                    if frozenset({a, b}) in existing or frozenset({a, b}) in seen_pairs:
                        continue
                    diff = abs(intensity[a] - intensity[b])
                    if diff >= HYBRID_PRIORITY: continue  # only ambiguous ones
                    d2 = float(((fnorm[a_idx] - fnorm[b_idx]) ** 2).sum())
                    gap = abs(bt_arr[a_idx] - bt_arr[b_idx])
                    prior_factor = 1.0 + 1.5 * (gap / 10.0)
                    score = prior_factor * d2 * 1.5  # bonus for cross-community ambiguous
                    candidates.append((score, a_idx, b_idx, "phase2_cross_ambiguous"))
                    cross_added += 1
    print(f"[plan] phase-2 candidate pool: {intra_count} intra + "
          f"{cross_added} cross-ambiguous = {len(candidates)}")

    # Diversity-rotated greedy emit: maintain sliding window of recent
    # community touches; downweight pairs whose communities are recently used.
    candidates.sort(key=lambda x: -x[0])
    recent: deque[int] = deque(maxlen=8)
    emitted_phase2 = 0
    for score, a_idx, b_idx, tier in candidates:
        if emitted_phase2 >= args.phase2_budget: break
        ca, cb = int(labels[a_idx]), int(labels[b_idx])
        decay = 0.85 ** sum(1 for r in recent if r in (ca, cb))
        adj_score = score * decay
        # We don't re-sort; greedy take in original order. Recent tracking just
        # informs scoring metadata for analysis (not strictly necessary here
        # since we sort once, but matches the UI's behaviour).
        a, b = track_ids[a_idx], track_ids[b_idx]
        emit(a_idx, b_idx, tier, adj_score)
        if frozenset({a, b}) in seen_pairs:  # was actually emitted
            recent.append(ca); recent.append(cb)
            emitted_phase2 += 1

    n_total = len(plan)
    print(f"[plan] total emitted: {n_total} ({n_phase1} phase-1 + "
          f"{n_total - n_phase1} phase-2)")

    # Bilateral pairing: each (a, b) becomes (a, b) AND (b, a).
    bilateral: list[tuple[int, int, str, float]] = []
    for a, b, tier, score in plan:
        bilateral.append((a, b, tier, score))
        bilateral.append((b, a, tier + "_rev", score))
    print(f"[plan] bilateral output: {len(bilateral)} directed pairs")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w") as f:
        w = csv.writer(f)
        w.writerow(["track_a", "track_b", "tier", "score"])
        w.writerows(bilateral)
    print(f"[plan] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
