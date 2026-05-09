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
    p.add_argument("--audio-emb-key", default="embeddings_1024",
                   choices=["embeddings_1024", "embeddings"],
                   help="must match the audio head used during teacher + student "
                        "training. Defaults to round-7.7's 1024-d Conformer hidden.")
    p.add_argument("--split", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--no-deploy", action="store_true",
                   help="don't overwrite the deployed-aggression-axis pointer")
    return p.parse_args()


def main(args) -> int:
    import torch

    state = torch.load(args.student_pt, map_location="cpu", weights_only=True)

    # Detect arch from state_dict layout. The old linear student stores the
    # head as `intensity.weight` / `intensity.bias` (a single nn.Linear).
    # The new mlp student (added 2026-05-08, spec §765-768 escalation) stores
    # it as a Sequential, so keys look like `intensity.0.weight`,
    # `intensity.0.bias`, `intensity.3.weight`, `intensity.3.bias`.
    if "intensity.weight" in state:
        arch = "linear"
    elif "intensity.0.weight" in state and "intensity.3.weight" in state:
        arch = "mlp"
    else:
        sys.exit(f"unrecognized student state_dict keys: {list(state.keys())[:8]}")
    print(f"[export] detected arch: {arch}")

    if arch == "linear":
        W = state["intensity.weight"].numpy().squeeze().astype(np.float32)
        b = float(state["intensity.bias"].numpy().squeeze())
        if W.ndim != 1:
            sys.exit(f"unexpected student vec shape: {W.shape}, expected 1-d")
        in_dim = int(W.shape[0])
        print(f"[export] student vec: {in_dim}d, bias={b:+.4f}")

        def score_fn(audio_arr: np.ndarray) -> np.ndarray:
            return audio_arr @ W + b

    else:  # mlp
        W1 = state["intensity.0.weight"].numpy().astype(np.float32)   # (hidden, in_dim)
        b1 = state["intensity.0.bias"].numpy().astype(np.float32)     # (hidden,)
        W2 = state["intensity.3.weight"].numpy().astype(np.float32)   # (1, hidden)
        b2 = float(state["intensity.3.bias"].numpy().squeeze())       # scalar
        if W2.shape[0] != 1:
            sys.exit(f"unexpected mlp W2 shape: {W2.shape}, expected (1, ?)")
        in_dim = int(W1.shape[1])
        hidden = int(W1.shape[0])
        print(f"[export] student mlp: {in_dim} → {hidden} (GELU) → 1, bias={b2:+.4f}")
        # GELU approximation matching torch.nn.GELU default ('none', not 'tanh'):
        # gelu(x) = 0.5 * x * (1 + erf(x / sqrt(2)))
        from math import sqrt
        SQRT2 = sqrt(2.0)
        from scipy.special import erf as _erf

        def _gelu(x: np.ndarray) -> np.ndarray:
            return 0.5 * x * (1.0 + _erf(x / SQRT2))

        def score_fn(audio_arr: np.ndarray) -> np.ndarray:
            h = audio_arr @ W1.T + b1            # (N, hidden)
            h = _gelu(h)                         # dropout is identity at inference
            return (h @ W2.T + b2).squeeze(-1)   # (N,)

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
    if args.audio_emb_key not in e.files:
        sys.exit(f"[export] audio_emb NPZ at {args.audio_emb} has no "
                 f"'{args.audio_emb_key}' field (available: {list(e.files)}). "
                 f"Re-run embed_corpus_mulan.py with the round-7.7 dual-head "
                 f"version, or pass --audio-emb-key embeddings to use the "
                 f"v18.1-era 512-d substrate.")
    audio_arr  = e[args.audio_emb_key].astype(np.float32)
    audio_tid_to_i = {int(t): i for i, t in enumerate(audio_tids)}
    if audio_arr.shape[1] != in_dim:
        sys.exit(f"[export] audio_emb dim {audio_arr.shape[1]} doesn't match "
                 f"student weight in_dim {in_dim}. Pick the matching "
                 f"--audio-emb-key.")

    cs = np.load(args.consensus, allow_pickle=True)
    cs_tids = cs["track_ids"].astype(np.int64)
    cs_arr  = cs["consensus_intensity"].astype(np.float32)
    cs_tid_to_i = {int(t): i for i, t in enumerate(cs_tids)}

    aud = np.stack([audio_arr[audio_tid_to_i[t]] for t in test_tids], axis=0)
    y   = np.array([cs_arr[cs_tid_to_i[t]] for t in test_tids], dtype=np.float32)
    score = score_fn(aud)

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
        "embedding_dim": in_dim,
        "embedding_head": args.audio_emb_key,
        "model_type": arch,            # "linear" or "mlp"
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
            "MF caption used at training time only; deployed weights are over "
            "MuQ-MuLan only; user-redistributable subject to MuQ-MuLan license."
        ),
        "deprecates": ["V15", "V17b"],
        "spec": "documents/round-7-6-pipeline-spec.md",
    }
    if arch == "linear":
        payload["intensity_axis_vec"] = [float(x) for x in W.tolist()]
        payload["bias"] = b
    else:  # mlp
        payload["mlp"] = {
            "hidden_dim": int(hidden),
            "activation": "gelu",
            "dropout_train_only": True,
            # Layer 1: in (512) → hidden
            "W1": [[float(x) for x in row] for row in W1.tolist()],
            "b1": [float(x) for x in b1.tolist()],
            # Layer 2: hidden → out (1)
            "W2": [[float(x) for x in row] for row in W2.tolist()],
            "b2": b2,
            # Inference: y = W2 @ gelu(W1 @ audio_emb + b1) + b2
            "inference_pseudocode": (
                "h = audio_emb @ W1.T + b1; "
                "h = 0.5 * h * (1 + erf(h / sqrt(2))); "  # exact GELU
                "y = h @ W2.T + b2"
            ),
        }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2))
    print(f"[export] wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
