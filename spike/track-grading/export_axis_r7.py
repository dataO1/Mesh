"""Export round-7 blended axis as the V*.json schema mesh-cue/player loads.

The Rust runtime in `crates/mesh-core/src/intensity_axis.rs` strictly
validates:
  - embedding_dim == 512
  - intensity_axis_vec is length 512 AND unit norm
  - each sub_axes[*].axis_vec is length 512 AND unit norm

So we L2-normalize the blended `effective_direction` before writing, and
expose the 12 learned per-axis directions as `sub_axes` (each L2-norm).

Output: models/aggression-axes/V16_round7_blend.json
        + symlink-style copy at <collection_root>/muq-mulan-aggression-axis.json
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import shutil
import sys
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--blend", type=Path,
                   default=Path("/tmp/track-grading/round7_blend.npz"))
    p.add_argument("--axes-file", type=Path,
                   default=Path("/tmp/track-grading/round7_axes.npz"))
    p.add_argument("--metrics", type=Path,
                   default=Path("/tmp/track-grading/round7_train_metrics.json"))
    p.add_argument("--out", type=Path,
                   default=Path("models/aggression-axes/V16_round7_blend.json"))
    p.add_argument("--deploy-collection", type=Path,
                   default=Path("/home/data01/Music/mesh-collection"),
                   help="copy as muq-mulan-aggression-axis.json into this collection")
    p.add_argument("--no-deploy", action="store_true",
                   help="don't copy into the collection root")
    return p.parse_args()


def l2_normalize(v: np.ndarray) -> np.ndarray:
    n = np.linalg.norm(v)
    if n < 1e-9:
        raise ValueError(f"cannot L2-normalize zero vector (norm={n})")
    return v / n


def main() -> int:
    args = parse_args()
    if not args.blend.exists():
        sys.exit(f"missing {args.blend}")
    blend = np.load(args.blend, allow_pickle=True)
    axes_npz = np.load(args.axes_file, allow_pickle=True)

    eff_dir = blend["effective_direction"].astype(np.float32)
    eff_bias = float(blend["effective_bias"])
    pa = float(blend["pa"])
    rho = float(blend["rho"])
    target = str(blend["target_axis"])

    if eff_dir.shape[0] != 512:
        sys.exit(f"effective_direction must be 512-d, got {eff_dir.shape[0]}")

    # L2-normalize the blended axis (Rust requires unit norm).
    eff_dir_unit = l2_normalize(eff_dir).astype(np.float32)

    axes = list(axes_npz["axes"])
    directions = axes_npz["directions"].astype(np.float32)  # already row-normalised
    biases = axes_npz["biases"].astype(np.float32)
    weights = blend["weights"].astype(np.float32)
    cv_pa = axes_npz["cv_pa"].astype(np.float32)
    cv_rho = axes_npz["cv_rho"].astype(np.float32)

    metrics_extra = {}
    if args.metrics.exists():
        try:
            metrics_extra = json.loads(args.metrics.read_text())
        except Exception:
            pass

    sub_axes = []
    for k, name in enumerate(axes):
        v = l2_normalize(directions[k]).astype(np.float32)
        sub_axes.append({
            "name": name,
            "axis_vec": [float(x) for x in v],
            "prompts_positive": [],
            "prompts_negative": [],
            "n_positive": 0,
            "n_negative": 0,
            "weight_in_intensity": float(weights[k]),
        })

    out_doc = {
        "variant_id": "V16_round7_blend",
        "name": "Round-7 multi-axis blend (12 LLM-judged axes → ListMLE blend → aggression)",
        "rationale": (
            "Round-7 result: 12 per-axis linear probes trained jointly on the "
            "21k-track everynoise→Deezer corpus with per-axis Bradley-Terry priors "
            "from Qwen3-Omni-30B-A3B-Instruct-AWQ pairwise judging. The single "
            "intensity axis is a softmax-blended combination of the 12 per-axis "
            "scores, optimised via ListMLE against the per-axis aggression target. "
            "Per-axis directions are exposed as sub_axes for future per-axis UI "
            "controls and per-library blend re-fitting (round-8). Drop-in "
            "compatible with the existing polar IntensityAxis runtime."
        ),
        "model": "OpenMuQ/MuQ-MuLan-large",
        "embedding_dim": 512,
        "method": "multi-axis-linear-probes + ListMLE blend, all L2-normalised",
        "intensity_formula": (
            "score(x) = x · intensity_axis_vec  "
            "(equivalent to softmax-blended sum_k w_k * z(x · sub_k))"
        ),
        "intensity_axis_vec": [float(x) for x in eff_dir_unit],
        "sub_axes": sub_axes,
        "generated_at": dt.datetime.utcnow().isoformat() + "+00:00",
        "training_provenance": {
            "labels_source": "Qwen3-Omni-30B-A3B-Instruct-AWQ pairwise tournament, 12 axes",
            "corpus": "everynoise → Deezer round-7 (~15k unique tracks, 30s previews)",
            "n_axes": int(len(axes)),
            "axes": axes,
            "blend_weights": {axes[k]: float(weights[k]) for k in range(len(axes))},
            "loss": "per-axis: RankNet pairwise margin / blend: ListMLE",
            "optimizer": "AdamW",
            "cv_per_axis_pa": {axes[k]: float(cv_pa[k]) for k in range(len(axes))},
            "cv_per_axis_rho": {axes[k]: float(cv_rho[k]) for k in range(len(axes))},
            "blend_pairwise_agreement_corpus": pa,
            "blend_spearman_corpus": rho,
            "target_axis": target,
            **{k: v for k, v in metrics_extra.items() if k.startswith("cv_")},
        },
    }

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(out_doc, indent=2))
    print(f"[export] wrote {args.out}")
    print(f"[export] blend pa={pa:.4f} rho={rho:+.4f}")
    print(f"[export] weights:")
    for k in np.argsort(-weights):
        print(f"  {axes[k]:>22}: w={weights[k]:.4f}  cv_pa={cv_pa[k]:.3f}")

    # Verify by re-loading + roundtrip-projecting a unit vector to ensure
    # we didn't break the schema.
    rt = json.loads(args.out.read_text())
    assert rt["embedding_dim"] == 512
    assert len(rt["intensity_axis_vec"]) == 512
    norm = np.linalg.norm(rt["intensity_axis_vec"])
    assert abs(norm - 1.0) < 1e-3, f"intensity_axis_vec norm = {norm}"
    for sub in rt["sub_axes"]:
        sn = np.linalg.norm(sub["axis_vec"])
        assert abs(sn - 1.0) < 1e-3, f"sub '{sub['name']}' norm = {sn}"
    print(f"[export] schema validated (intensity norm = {norm:.6f})")

    if not args.no_deploy:
        if args.deploy_collection.is_dir():
            target_path = args.deploy_collection / "muq-mulan-aggression-axis.json"
            # Backup the previously-deployed file first.
            if target_path.exists():
                bk = args.deploy_collection / f"muq-mulan-aggression-axis.PRE-V16.{int(dt.datetime.utcnow().timestamp())}.json"
                shutil.copy2(target_path, bk)
                print(f"[export] backed up old axis → {bk}")
            shutil.copy2(args.out, target_path)
            print(f"[export] deployed to {target_path}")
        else:
            print(f"[export] WARN: --deploy-collection {args.deploy_collection} does not exist; skipping")
    return 0


if __name__ == "__main__":
    sys.exit(main())
