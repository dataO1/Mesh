"""Export the round-6 V15 linear probe to a polar-format JSON the existing
mesh-cue IntensityAxis loader accepts.

V15 = Linear(512 → 1). Mathematically a unit-norm 512-d projection plus a
scalar bias. Bias only shifts the score uniformly, so it doesn't affect
ranking — we drop it. The weight vector becomes `intensity_axis_vec` after
L2 normalisation.

We retrain the linear probe on the full 909 tracks (no CV split) before
exporting — the CV step in train_head_r6.py was for measurement; the
shipped model uses all available data.

Sub-axes: copied verbatim from V11 so the UI's per-axis sub-controls
("less distorted", "darker", etc.) keep working. The intensity column
becomes V15-driven; the sub-axes remain V11-style polar projections.

Usage:
  LD_LIBRARY_PATH=/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib \\
    ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/export_axis_json.py
Output: models/muq-mulan-aggression-axis-v15.json (place at canonical path
when ready to deploy)
"""
from __future__ import annotations

import datetime
import json
import sys
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

EMB_PATH = Path("/home/data01/Music/mesh-track-grading/embeddings.npz")
PRIORS_PATH = Path("documents/axis-eval-results/llm-pair-priors-r5.txt")
V11_PATH = Path("models/muq-mulan-aggression-axis.json")  # current default, source of sub_axes
OUT_PATH = Path("models/aggression-axes/V15_linear_probe_r6.json")


class LinearProbe(nn.Module):
    def __init__(self, in_dim: int = 512):
        super().__init__()
        self.lin = nn.Linear(in_dim, 1)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.lin(x).squeeze(-1)


def pairwise_margin_loss(scores, y, margin_floor=0.5):
    import torch.nn.functional as F
    n = scores.shape[0]
    y_diff = y.unsqueeze(0) - y.unsqueeze(1)
    s_diff = scores.unsqueeze(0) - scores.unsqueeze(1)
    mask_tri = torch.triu(torch.ones(n, n, device=y.device), diagonal=1).bool()
    mask = mask_tri & (y_diff.abs() > margin_floor)
    if not mask.any():
        return torch.tensor(0.0, device=y.device, requires_grad=True)
    sign = y_diff.sign(); margin = y_diff.abs()
    return F.relu(margin - sign * s_diff)[mask].mean()


def main() -> int:
    if not EMB_PATH.exists():
        sys.exit(f"missing {EMB_PATH} — run dump_embeddings.py first")
    if not PRIORS_PATH.exists():
        sys.exit(f"missing {PRIORS_PATH}")
    if not V11_PATH.exists():
        sys.exit(f"missing {V11_PATH} (need V11 sub_axes for passthrough)")

    npz = np.load(EMB_PATH)
    ids = npz["track_ids"]; X = npz["embeddings"]
    priors: dict[int, float] = {}
    with PRIORS_PATH.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"): continue
            parts = line.split("|", 2)
            if len(parts) == 3: priors[int(parts[0])] = float(parts[2])
    mask = np.array([int(t) in priors for t in ids])
    X = X[mask]; ids = ids[mask]
    y = np.array([priors[int(t)] for t in ids], dtype=np.float32)
    print(f"[export] {len(ids)} tracks, embedding dim {X.shape[1]}")

    torch.manual_seed(42)
    model = LinearProbe(in_dim=X.shape[1])
    X_t = torch.from_numpy(X).float()
    y_t = torch.from_numpy(y).float()
    opt = torch.optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
    for ep in range(400):
        model.train(); opt.zero_grad()
        loss = pairwise_margin_loss(model(X_t), y_t)
        loss.backward(); opt.step()
    model.eval()
    print(f"[export] final loss: {loss.item():.4f}")

    # Extract weight + bias. Linear(in, 1): weight shape [1, in], bias [1].
    w: np.ndarray = model.lin.weight.detach().numpy().squeeze(0).astype(np.float32)
    b: float = float(model.lin.bias.detach().numpy().squeeze())
    norm = float(np.linalg.norm(w))
    w_unit = (w / norm).astype(np.float32)
    print(f"[export] weight ||w||={norm:.4f}, bias={b:.4f} (bias dropped — no rank effect)")

    # Validate against the IntensityAxis projector behaviour: dot(emb, w_unit).
    proj = X @ w_unit
    # Compare ordering to model output: should be identical (same direction).
    with torch.no_grad():
        scores = model(X_t).numpy()
    rank_proj = np.argsort(np.argsort(proj))
    rank_scores = np.argsort(np.argsort(scores))
    sum_d2 = float(np.sum((rank_proj - rank_scores) ** 2))
    rho = 1 - 6 * sum_d2 / (len(ids) * (len(ids) ** 2 - 1))
    print(f"[export] sanity Spearman(unit-norm projection vs model) = {rho:+.6f} "
          f"(should be +1.0)")

    # Build the IntensityAxis JSON. Reuse V11's sub_axes for UI sub-controls.
    v11 = json.loads(V11_PATH.read_text())
    out = {
        "variant_id": "V15_linear_probe_r6",
        "name": "Linear probe (round-6) on round-5 BT priors",
        "rationale": (
            "Round-6 result: a single Linear(512→1) probe trained on "
            "MuQ-MuLan embeddings via RankNet pairwise margin loss against "
            "round-5 LLM-derived BT priors. Beats V11 by +8.6 pp pairwise "
            "agreement (71.1% vs 62.5% via 5-fold CV). Drop-in compatible "
            "with the existing polar IntensityAxis runtime — no code changes."
        ),
        "model": "OpenMuQ/MuQ-MuLan-large",
        "embedding_dim": 512,
        "method": "learned-linear-probe, l2-normalised",
        "intensity_formula": (
            "l2_normalize(LinearProbe(MuQ-MuLan-emb)); bias dropped "
            "(does not affect ranking)"
        ),
        "intensity_axis_vec": w_unit.tolist(),
        "sub_axes": v11.get("sub_axes", []),
        "generated_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "training_provenance": {
            "labels_source": str(PRIORS_PATH),
            "n_train_tracks": int(len(ids)),
            "loss": "pairwise margin (RankNet), margin_floor=0.5",
            "optimizer": "AdamW lr=1e-3 wd=1e-4",
            "epochs": 400,
            "cv_pairwise_agreement": 0.7109,
            "cv_spearman_vs_priors": 0.6002,
            "spearman_vs_47_hand_anchors": 0.4294,
        },
    }

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(json.dumps(out, indent=2))
    print(f"[export] wrote {OUT_PATH}")
    print(f"[export] to deploy: cp {OUT_PATH} {V11_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
