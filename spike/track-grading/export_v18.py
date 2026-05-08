"""Stage S13 — V18 export.

Per spec § 19. Writes V18 in the same JSON shape as V15 / V17b so the
mesh-collection / mesh-cue inference paths pick it up unchanged.

Pass criteria:
  - intensity_axis_vec is exactly 512 floats
  - bias is a scalar
  - Reproducing test PA from the JSON's vec/bias matches the eval report
    to ≤ 1e-4 absolute error.

Usage:
    bash spike/track-grading/run_r7_step.sh export_v18.py \\
         --student-pt /home/data01/Music/mesh-track-grading/round7_6_student.pt \\
         --eval /home/data01/Music/mesh-track-grading/round7_6_eval.json \\
         --consensus /home/data01/Music/mesh-track-grading/round7_6_consensus.npz \\
         --audio-emb /home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz \\
         --split /home/data01/Music/mesh-track-grading/round7_6_split.npz \\
         --out models/aggression-axes/V18_round7_6_consensus_distilled.json
"""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--student-pt", type=Path, required=True)
    p.add_argument("--eval", type=Path, required=True)
    p.add_argument("--consensus", type=Path, required=True)
    p.add_argument("--audio-emb", type=Path, required=True)
    p.add_argument("--split", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--no-deploy", action="store_true",
                   help="don't overwrite the deployed-aggression-axis pointer")
    return p.parse_args()


def main(args) -> int:
    import torch

    state = torch.load(args.student_pt, map_location="cpu", weights_only=True)
    W = state["intensity.weight"].numpy().squeeze().astype(np.float32)
    b = float(state["intensity.bias"].numpy().squeeze())
    if W.shape != (512,):
        sys.exit(f"unexpected student vec shape: {W.shape}, expected (512,)")
    print(f"[export] student vec: 512d, bias={b:+.4f}")

    # ── Reproduce test PA from the JSON-extracted vec ────────────────
    # Use the EXACT test track IDs persisted by eval (so the intersection
    # we reproduce here matches the one eval used; otherwise differing
    # feature schemas would yield different test sets and thus different
    # PA values).
    eval_report = json.loads(args.eval.read_text())
    test_tids = [int(t) for t in eval_report.get("test_track_ids", [])]
    if not test_tids:
        sys.exit("eval.json missing 'test_track_ids' — re-run eval_v18.py to "
                 "produce it before exporting.")

    e = np.load(args.audio_emb, allow_pickle=True)
    audio_tids = e["track_ids"].astype(np.int64)
    audio_arr  = e["embeddings"].astype(np.float32)
    audio_tid_to_i = {int(t): i for i, t in enumerate(audio_tids)}

    cs = np.load(args.consensus, allow_pickle=True)
    cs_tids = cs["track_ids"].astype(np.int64)
    cs_arr  = cs["consensus_intensity"].astype(np.float32)
    cs_tid_to_i = {int(t): i for i, t in enumerate(cs_tids)}

    aud = np.stack([audio_arr[audio_tid_to_i[t]] for t in test_tids], axis=0)
    y   = np.array([cs_arr[cs_tid_to_i[t]] for t in test_tids], dtype=np.float32)
    score = aud @ W + b

    n = len(score)
    ds = score[:, None] - score[None, :]; dy = y[:, None] - y[None, :]
    tri = np.triu(np.ones((n, n), dtype=bool), k=1)
    valid = tri & (ds != 0) & (dy != 0)
    repro_pa = float((valid & ((ds > 0) == (dy > 0))).sum() / max(valid.sum(), 1))

    reported_pa = eval_report["metrics"]["test_pa_student"]
    print(f"[export] reported test PA: {reported_pa:.6f}")
    print(f"[export] reproduced test PA from V18 JSON: {repro_pa:.6f}")
    if abs(repro_pa - reported_pa) > 1e-4:
        sys.exit(f"V18 export reproduction error: |Δ|={abs(repro_pa-reported_pa):.4e} > 1e-4")
    print("[export] reproduction OK ✓")

    # ── Read source reliabilities from consensus NPZ (spec § 14) ──────
    src_names = list(cs["source_names"])
    src_rel = cs["source_reliabilities"]
    label_sources = src_names
    src_rel_dict = {str(s): float(r) for s, r in zip(src_names, src_rel)}

    # ── Per-cluster PA from eval ─────────────────────────────────────
    per_cluster_pa = {f"cluster_{c['k']}": c["pa_student"]
                      for c in eval_report.get("per_cluster", [])
                      if not (isinstance(c.get("pa_student"), float)
                              and np.isnan(c["pa_student"]))}

    payload = {
        "version": "V18_round7_6_consensus_distilled",
        "embedding": "muq-mulan",
        "embedding_dim": 512,
        "intensity_axis_vec": [float(x) for x in W.tolist()],
        "bias": b,
        "trained_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "trained_on_corpus": "deezer-everynoise-expanded-2026-05-07",
        "label_sources": label_sources,
        "source_reliabilities": src_rel_dict,
        "test_pa": reported_pa,
        "test_spearman": eval_report["metrics"].get("test_spearman_student"),
        "test_r2": eval_report["metrics"].get("test_r2_student"),
        "distillation_gap_pp": eval_report["metrics"].get("distillation_gap_pp"),
        "per_cluster_pa": per_cluster_pa,
        "license_note": (
            "MF caption used at training time only; deployed weights are linear "
            "over MuQ-MuLan; user-redistributable subject to MuQ-MuLan license."
        ),
        "deprecates": ["V15", "V17b"],
        "spec": "documents/round-7-6-pipeline-spec.md",
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2))
    print(f"[export] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
