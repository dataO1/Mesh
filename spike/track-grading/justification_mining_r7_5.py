"""Round-7.5 justification mining → multi-label tag matrix.

For each per-call JSON in /tmp/track-grading/round7_5_pairs/<axis>/, the LLM
left a short justification line (≤25 words). We parse that text into structured
feature tags and aggregate per-track to use as auxiliary multi-label supervision
in the linear-probe training step.

Two sources of tags:
  (a) Pure regex/keyword extraction over the justification text (cheap,
      deterministic, ~99.9% of value).
  (b) Optional: LLM-based tag extraction via vLLM (text-only, can use the
      same Qwen server). Only fire this for justifications regex misses.

Tag taxonomy (cross-axis, derived from inspecting round-7 justifications):

  distortion_high / distortion_low
  vocal_clean / vocal_shouted / vocal_absent
  density_high / density_low
  bass_heavy / bass_thin
  brightness_high / brightness_low
  noise_high / noise_low
  tempo_fast / tempo_slow
  rhythmic_complex / rhythmic_simple
  melodic_present / melodic_absent
  compression_high / compression_dynamic
  acoustic / synthetic
  drop_present / continuous

Each track gets a tag-vector aggregated across all the calls that touched it
(majority vote across mentions, with confidence = mention count). This becomes
the auxiliary BCE target during probe training.

Output: /tmp/track-grading/round7_5_tags.npz
  track_ids : int64[N]
  tag_names : object[T]
  tag_evidence : float32[N, T]   (signed: +X confidence for tag, -X for opposite)
  tag_n_mentions : int32[N, T]   (raw mention counts for diagnostic)

Usage:
  bash spike/track-grading/run_r7_step.sh justification_mining_r7_5.py
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np


# ─── Tag taxonomy: each tag has a + and − keyword set ──────────────────────
# Tuples are (tag_name, regex_for_positive, regex_for_negative).
# Patterns are case-insensitive; we OR-match against the justification text.

TAG_PATTERNS = [
    ("distortion",
     r"distort|fuzz|saturat|clipp|grit(t?y)?|harsh|raw\s+tone|crunch",
     r"clean\s+tone|smooth\s+tone|undistorted|polished\s+tone"),
    ("vocal_shouted",
     r"shout|scream|growl|harsh\s+vocal|aggressive\s+vocal|harsh\s+(rap|delivery)",
     r"clean\s+vocal|sung\s+vocal|melodic\s+vocal|smooth\s+vocal|spoken|(no|absent|without)\s+vocal"),
    ("density",
     r"\bdense|busy|packed|layered|thick|stacked|cluttered",
     r"sparse|minimal|breathing\s+room|empty|spacious|open"),
    ("bass_heavy",
     r"heavy\s+bass|sub[-\s]?bass|low[-\s]?end|kick(s)?\s+(prominent|heavy)|bass-heavy|booming",
     r"thin\s+bass|light\s+(low[-\s]?end|bass)|no\s+bass|absent\s+bass"),
    ("brightness",
     r"bright|airy|sparkl|crisp|treble[-\s]?heavy|high[-\s]?freq",
     r"\bdark\b|brooding|murky|muddy|low[-\s]?heavy|warm\s+(spectrum|tone)"),
    ("noise_layer",
     r"noise|hiss|static|glitch|granul|crackle|noisy\s+(layer|fx|sound)",
     r"clean\s+(mix|sound)|no\s+noise|all\s+pitched"),
    ("tempo_fast",
     r"fast|frantic|urgent|driving|rapid|speed(y|core)|quick(\s+pace)?",
     r"slow|relaxed|laid[-\s]?back|floating|calm|chill"),
    ("rhythmic_complex",
     r"break\s?beat|polyrhyth|syncop|complex\s+rhythm|rhythmic(ally)?\s+intricate|shifted\s+accent",
     r"four[-\s]on[-\s]the[-\s]floor|simple\s+(beat|rhythm|pulse)|steady\s+(pulse|beat)|on[-\s]grid"),
    ("melodic_present",
     r"melod(ic|y|ies)|tonal|hummable|singable|hook|chord(\s+progression)?",
     r"atonal|fragmented|unanchored|no\s+melody|non[-\s]melodic|drone"),
    ("compression_high",
     r"compress|brick[-\s]?wall|loud(ness)?[-\s]?flat|squashed|loud\s+throughout|louder\s+throughout",
     r"dynamic|quiet[-\s]?loud|breathing|wide\s+dynamic|contrasted\s+loudness"),
    ("synthetic",
     r"synth|electron|sampled|digital|sound[-\s]?design|drum\s+machine|808|909",
     r"acoustic|guitar|piano|drum\s+kit|band|live\s+instrument|organic"),
    ("drop_present",
     r"\bdrop\b|build[-\s]?up|peak|breakdown|tension[-\s]?release",
     r"continuous|ambient|through[-\s]?composed|no\s+drop"),
    ("aggressive_overall",
     r"aggressive|in[-\s]your[-\s]face|confrontational|intense|brutal|hard[-\s]hitting",
     r"calm|gentle|soft|peaceful|laid[-\s]?back"),
]


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--pairs-root", type=Path,
                   default=Path("/tmp/track-grading/round7_5_pairs"))
    p.add_argument("--out", type=Path,
                   default=Path("/tmp/track-grading/round7_5_tags.npz"))
    p.add_argument("--include-r7", action="store_true",
                   help="also mine round-7 (single-axis) per-pair JSONs from "
                        "/tmp/track-grading/round7_pairs/ as additional evidence")
    p.add_argument("--r7-pairs-root", type=Path,
                   default=Path("/tmp/track-grading/round7_pairs"))
    return p.parse_args()


def extract_tags(text: str) -> dict[str, int]:
    """Returns {tag_name: +1 / -1 / 0} per tag in TAG_PATTERNS.

    +1 = positive evidence (tag is true)
    -1 = negative evidence (tag is false / opposite)
     0 = no evidence either way
    """
    out: dict[str, int] = {}
    if not text:
        return out
    t = text.lower()
    for name, pos_re, neg_re in TAG_PATTERNS:
        has_pos = bool(re.search(pos_re, t))
        has_neg = bool(re.search(neg_re, t))
        if has_pos and not has_neg:
            out[name] = +1
        elif has_neg and not has_pos:
            out[name] = -1
        # mixed or absent → leave unset (= 0 implicit)
    return out


def attribute_to_track(record: dict) -> list[tuple[int, dict[str, int]]]:
    """Round-7.5 K=4 ranking record → per-track tag votes.

    The justification typically describes either the winner ("Clip A has heavy
    distortion") or compares to another. We attribute the extracted tags to all
    4 tracks proportionally, weighted by their position in the ranking:
    higher-rank track gets +sign for "high" tags, lower-rank gets -sign.

    For round-7 single-pair records, we attribute to the winning track for
    + tags and to the loser for - tags, since the choice is clearer.
    """
    text = record.get("raw_response", "")
    tags = extract_tags(text)
    if not tags:
        return []

    out: list[tuple[int, dict[str, int]]] = []

    # K=4 ranking case
    ranking = record.get("ranking_low_to_high")
    l2t = record.get("letter_to_track")
    if ranking and l2t:
        l2t = {k: int(v) for k, v in l2t.items()}
        # Attribute to highest-rank (= "more HIGH pole") track positively, and
        # lowest-rank track negatively, on tags that match the axis direction.
        # We stay agnostic about WHICH axis this call was for — the tag
        # taxonomy is independent.
        n = len(ranking)
        # Track at rank 0 = lowest pole = should get tags with sign flipped if
        # the tag aligns with the axis. The simplest unbiased approach: send
        # the full tag vector to every track in the call, and let aggregation
        # average them out across many calls per track.
        for L in ranking:
            tid = l2t[L]
            out.append((tid, tags))
        return out

    # Round-7 pairwise case: attribute + tags to winner, - tags to loser
    presented_a = record.get("presented_a")
    presented_b = record.get("presented_b")
    choice = record.get("choice")
    winner_id = record.get("winner_id")
    if winner_id is not None and presented_a is not None and presented_b is not None:
        loser_id = presented_b if winner_id == presented_a else presented_a
        # Winner gets the tags as-is (the LLM's reasoning was about why it won)
        out.append((int(winner_id), tags))
        # Loser gets opposite signs (justification implicitly says it lacked
        # those qualities)
        out.append((int(loser_id), {k: -v for k, v in tags.items()}))
    return out


def main() -> int:
    args = parse_args()

    sources = []
    if args.pairs_root.exists():
        for d in args.pairs_root.iterdir():
            if d.is_dir() and not d.name.startswith("_"):
                sources.append(("r7.5", d))
    if args.include_r7 and args.r7_pairs_root.exists():
        for d in args.r7_pairs_root.iterdir():
            if d.is_dir() and not d.name.startswith("_"):
                sources.append(("r7", d))

    if not sources:
        sys.exit(f"no axis dirs found under {args.pairs_root}")

    # Aggregate per-track tag evidence. For each (track, tag), we accumulate
    # signed mention count (positive votes - negative votes) and absolute
    # mention count.
    tag_names = [name for name, _, _ in TAG_PATTERNS]
    tag_idx = {n: i for i, n in enumerate(tag_names)}

    track_evidence: dict[int, np.ndarray] = defaultdict(
        lambda: np.zeros(len(tag_names), dtype=np.float32))
    track_mentions: dict[int, np.ndarray] = defaultdict(
        lambda: np.zeros(len(tag_names), dtype=np.int32))
    n_records = 0
    n_with_tags = 0

    for src_name, d in sources:
        for f in d.glob("*.json"):
            try:
                rec = json.loads(f.read_text())
            except Exception:
                continue
            n_records += 1
            attribs = attribute_to_track(rec)
            if attribs:
                n_with_tags += 1
            for tid, tags in attribs:
                for name, sign in tags.items():
                    if name in tag_idx and sign != 0:
                        i = tag_idx[name]
                        track_evidence[tid][i] += sign
                        track_mentions[tid][i] += 1

    print(f"[mining] processed {n_records} records, "
          f"{n_with_tags} ({100*n_with_tags/max(n_records,1):.1f}%) had ≥1 tag match")
    print(f"[mining] {len(track_evidence)} unique tracks tagged")

    track_ids = sorted(track_evidence.keys())
    N = len(track_ids)
    T = len(tag_names)
    evidence_arr = np.zeros((N, T), dtype=np.float32)
    mentions_arr = np.zeros((N, T), dtype=np.int32)
    for i, t in enumerate(track_ids):
        evidence_arr[i] = track_evidence[t]
        mentions_arr[i] = track_mentions[t]

    # Per-tag stats
    print("\n[mining] tag coverage:")
    print(f"{'tag':>22} | n_tracks_tagged | mean_evidence | std")
    for j, name in enumerate(tag_names):
        nz = mentions_arr[:, j] > 0
        n = int(nz.sum())
        if n == 0:
            print(f"{name:>22} | {n:15d} | {'-':>12} | {'-':>5}")
            continue
        mean_ev = float(evidence_arr[nz, j].mean())
        std_ev = float(evidence_arr[nz, j].std())
        print(f"{name:>22} | {n:15d} | {mean_ev:+12.3f} | {std_ev:5.2f}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(args.out,
             track_ids=np.array(track_ids, dtype=np.int64),
             tag_names=np.array(tag_names, dtype=object),
             tag_evidence=evidence_arr,
             tag_n_mentions=mentions_arr)
    print(f"\n[mining] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
