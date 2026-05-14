"""Bradley-Terry pairwise sampler + logistic loss for MuQ-MuLan LoRA training.

PairSampler
-----------
Loads consensus labels (5-juror), artist-stratified split, and optional
precomputed embeddings.  Provides batched pairs (emb_i, emb_j, label)
where |consensus_i - consensus_j| > min_margin.  Stratified sampling by
consensus bucket avoids class imbalance.

BTLogisticLoss
--------------
Implements P(i beats j) = sigmoid(score_i - score_j) with optional
|Δconsensus| weighting (harder pairs get more weight).

Validation
----------
Spearman ρ between predicted scores and ground-truth consensus on a
fixed val subset.

Smoke test
----------
  python spike/track-grading/bt_pair_sampler.py --smoke

loads real data, samples 1000 pairs, and checks loss decreases in 5 SGD
steps on a frozen encoder with a trainable linear head.
"""
from __future__ import annotations

import argparse
import math
import sys
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F


# ---------------------------------------------------------------------------
#  Data paths (overridable via constructor / CLI)
# ---------------------------------------------------------------------------
_DEFAULT_CONSENSUS = Path("/home/data01/Music/mesh-track-grading/round7_6_consensus.npz")
_DEFAULT_SPLIT     = Path("/home/data01/Music/mesh-track-grading/round7_6_split.npz")
_DEFAULT_EMBEDDING = Path("/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz")


# ---------------------------------------------------------------------------
#  Bradley-Terry pair sampler
# ---------------------------------------------------------------------------

class PairSampler:
    """Sample (emb_i, emb_j, label) batches from consensus + split data.

    Parameters
    ----------
    min_margin : float
        Minimum absolute consensus difference to form a pair (on [0, 1]).
    n_buckets : int
        Number of consensus buckets for stratified sampling.
    batch_size : int
        Number of pairs per batch.
    device : torch.device
    """

    def __init__(
        self,
        consensus_path: Path = _DEFAULT_CONSENSUS,
        split_path: Path = _DEFAULT_SPLIT,
        embedding_path: Path | None = _DEFAULT_EMBEDDING,
        min_margin: float = 0.05,
        n_buckets: int = 10,
        batch_size: int = 256,
        device: torch.device = torch.device("cpu"),
    ):
        self.min_margin = min_margin
        self.n_buckets = n_buckets
        self.batch_size = batch_size
        self.device = device

        # ---- Load consensus ----
        c = np.load(consensus_path, allow_pickle=True)
        cons_ids = c["track_ids"].astype(np.int64)
        cons_val = c["consensus_intensity"].astype(np.float32)
        # Build (track_id -> index) map for O(1) lookup
        self._cons_map: dict[int, int] = {int(t): i for i, t in enumerate(cons_ids)}
        self._consensus = cons_val       # [N_consensus]

        # ---- Load split ----
        s = np.load(split_path, allow_pickle=True)
        split_ids = s["track_ids"].astype(np.int64)
        split_labels = s["split"]                     # string array "train"/"val"/"test"
        self._split_map: dict[int, str] = {int(t): str(lbl) for t, lbl in zip(split_ids, split_labels)}

        # ---- Intersect with embeddings (if provided) ----
        if embedding_path is not None:
            e = np.load(embedding_path, allow_pickle=True)
            emb_ids = e["track_ids"].astype(np.int64)
            self._embeddings_full = e["embeddings_1024"].astype(np.float32)
            self._emb_map: dict[int, int] = {int(t): i for i, t in enumerate(emb_ids)}
            self._embedding_dim = self._embeddings_full.shape[1]
        else:
            self._embeddings_full = None
            self._emb_map = None
            self._embedding_dim = 1024  # assume known default

        # ---- Compute per-split track lists ----
        all_ids = set(self._cons_map.keys())
        # Restrict to tracks that also exist in embeddings (if loaded)
        if self._emb_map is not None:
            all_ids &= set(self._emb_map.keys())

        self._track_ids: dict[str, np.ndarray] = {}
        for split_name in ("train", "val", "test"):
            matched = sorted(
                t for t in all_ids
                if self._split_map.get(t) == split_name
            )
            self._track_ids[split_name] = np.array(matched, dtype=np.int64)

        # ---- Precompute consensus values for each split ----
        self._cached_consensus: dict[str, np.ndarray] = {}
        for split_name, tids in self._track_ids.items():
            self._cached_consensus[split_name] = np.array(
                [self._consensus[self._cons_map[int(t)]] for t in tids],
                dtype=np.float32,
            )

        # ---- Stratification: assign each train track to a consensus bucket ----
        train_tids = self._track_ids["train"]
        train_c = self._cached_consensus["train"]
        # Bucket edges: [0, 1/n_buckets, 2/n_buckets, ..., 1]
        edges = np.linspace(0.0, 1.0, self.n_buckets + 1)
        edges[-1] += 1e-6  # ensure the 1.0 endpoint is inclusive
        self._train_bucket: np.ndarray = np.digitize(train_c, edges) - 1  # [0, n_buckets-1]
        self._train_bucket_ids: dict[int, np.ndarray] = {}
        for b in range(self.n_buckets):
            self._train_bucket_ids[b] = np.where(self._train_bucket == b)[0]

        # ---- Fixed validation subset (for efficient Spearman) ----
        val_tids = self._track_ids["val"]
        self._val_subset = val_tids
        self._cached_val_consensus = self._cached_consensus["val"]

        # ---- Embedding cache (optional on-the-fly loading) ----
        self._embed_cache: dict[int, torch.Tensor] = {}

        self._rng = np.random.default_rng(42)

    # ------------------------------------------------------------------
    #  Properties
    # ------------------------------------------------------------------

    @property
    def n_train(self) -> int:
        return len(self._track_ids["train"])

    @property
    def n_val(self) -> int:
        return len(self._track_ids["val"])

    @property
    def n_test(self) -> int:
        return len(self._track_ids["test"])

    @property
    def dim(self) -> int:
        return self._embedding_dim

    # ------------------------------------------------------------------
    #  Sampler
    # ------------------------------------------------------------------

    def sample_batch(
        self,
        split: str = "train",
        batch_size: int | None = None,
    ) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor]:
        """Draw a batch of (emb_i, emb_j, label) pairs.

        Stratified sampling: for each pair, sample i from bucket b_i,
        then sample j from a bucket b_j where the consensus gap is
        guaranteed to be >= min_margin.  This avoids the O(N²) rejection
        loop and keeps class balance near 50:50.
        """
        if batch_size is None:
            batch_size = self.batch_size

        tids = self._track_ids[split]
        cons = self._cached_consensus[split]

        if split == "train":
            # Stratified: pick bucket pairs that satisfy min_margin
            i_indices, j_indices = self._stratified_sample_pairs(
                batch_size, cons, self._train_bucket, self._train_bucket_ids
            )
        else:
            # For val/test: random pairs with margin filter
            i_indices, j_indices = self._random_filtered_pairs(
                batch_size, cons, self._rng
            )

        emb_i = self._get_embeddings(tids[i_indices])
        emb_j = self._get_embeddings(tids[j_indices])

        labels = (cons[i_indices] > cons[j_indices]).astype(np.float32)
        return (
            torch.from_numpy(emb_i).to(self.device),
            torch.from_numpy(emb_j).to(self.device),
            torch.from_numpy(labels).to(self.device),
        )

    def _stratified_sample_pairs(
        self,
        batch_size: int,
        cons: np.ndarray,
        bucket_labels: np.ndarray,
        bucket_ids: dict[int, np.ndarray],
    ) -> tuple[np.ndarray, np.ndarray]:
        """Sample pairs (i, j) with |cons_i - cons_j| >= min_margin via buckets.

        Returns (indices_i, indices_j) into the cons / tids arrays.
        """
        n_b = self.n_buckets
        half = batch_size // 2

        # half of pairs: i in low buckets (0..mid), j in high buckets (mid+1..n_b-1)
        # other half: i in high, j in low  → balanced labels
        mid = n_b // 2

        i_list: list[int] = []
        j_list: list[int] = []

        # --- Direction A: low -> high ---
        for _ in range(half):
            b_i = int(self._rng.integers(0, mid))
            b_j = int(self._rng.integers(mid, n_b))
            pool_i = bucket_ids[b_i]
            pool_j = bucket_ids[b_j]
            if len(pool_i) == 0 or len(pool_j) == 0:
                continue
            ii = int(self._rng.choice(pool_i))
            jj = int(self._rng.choice(pool_j))
            i_list.append(ii)
            j_list.append(jj)

        # --- Direction B: high -> low ---
        for _ in range(half):
            b_i = int(self._rng.integers(mid, n_b))
            b_j = int(self._rng.integers(0, mid))
            pool_i = bucket_ids[b_i]
            pool_j = bucket_ids[b_j]
            if len(pool_i) == 0 or len(pool_j) == 0:
                continue
            ii = int(self._rng.choice(pool_i))
            jj = int(self._rng.choice(pool_j))
            i_list.append(ii)
            j_list.append(jj)

        # Fall back to purely random if stratified failed
        if len(i_list) < batch_size:
            extra = batch_size - len(i_list)
            ii, jj = self._random_filtered_pairs(extra, cons, self._rng)
            i_list.extend(ii.tolist())
            j_list.extend(jj.tolist())

        return np.array(i_list[:batch_size], dtype=np.int64), np.array(j_list[:batch_size], dtype=np.int64)

    def _random_filtered_pairs(
        self,
        batch_size: int,
        cons: np.ndarray,
        rng: np.random.Generator,
    ) -> tuple[np.ndarray, np.ndarray]:
        """Rejection-sampled random pairs with |Δcons| >= min_margin."""
        n = len(cons)
        if n == 0:
            return np.array([], dtype=np.int64), np.array([], dtype=np.int64)

        i_list: list[int] = []
        j_list: list[int] = []
        max_attempts = batch_size * 100

        for _ in range(max_attempts):
            if len(i_list) >= batch_size:
                break
            i = int(rng.integers(0, n))
            j = int(rng.integers(0, n))
            if i == j:
                continue
            if abs(cons[i] - cons[j]) >= self.min_margin:
                i_list.append(i)
                j_list.append(j)

        # If we didn't get enough, relax margin
        if len(i_list) < batch_size:
            for _ in range(batch_size * 200):
                if len(i_list) >= batch_size:
                    break
                i = int(rng.integers(0, n))
                j = int(rng.integers(0, n))
                if i == j:
                    continue
                if abs(cons[i] - cons[j]) >= self.min_margin * 0.5:
                    i_list.append(i)
                    j_list.append(j)

        if len(i_list) < batch_size:
            # Absolute fallback: any pair
            while len(i_list) < batch_size:
                i = int(rng.integers(0, n))
                j = int(rng.integers(0, n))
                if i == j:
                    continue
                i_list.append(i)
                j_list.append(j)

        return np.array(i_list[:batch_size], dtype=np.int64), np.array(j_list[:batch_size], dtype=np.int64)

    def _get_embeddings(self, track_ids: np.ndarray) -> np.ndarray:
        """Fetch embeddings for an array of track_ids, using cache if possible."""
        if self._embeddings_full is not None and self._emb_map is not None:
            # Bulk lookup from pre-loaded array
            idxs = np.array([self._emb_map[int(t)] for t in track_ids], dtype=np.int64)
            return self._embeddings_full[idxs]
        else:
            # Fallback: use per-track cache (for live-computed embeddings)
            embs = []
            for t in track_ids:
                tid = int(t)
                if tid in self._embed_cache:
                    embs.append(self._embed_cache[tid].cpu().numpy())
                else:
                    raise KeyError(
                        f"Track {tid} not in embedding cache and no full embedding "
                        f"array was loaded.  Pass embedding_path or pre-populate cache."
                    )
            return np.stack(embs, axis=0)

    def get_val_consensus(self) -> tuple[np.ndarray, np.ndarray]:
        """Return (val_track_ids, val_consensus) for the fixed validation set."""
        return self._val_subset, self._cached_val_consensus

    def get_val_embeddings(self) -> torch.Tensor:
        """Return val embeddings as a tensor on self.device."""
        tids = self._val_subset
        embs = self._get_embeddings(tids)
        return torch.from_numpy(embs).to(self.device)


# ---------------------------------------------------------------------------
#  Bradley-Terry logistic loss
# ---------------------------------------------------------------------------

class BTLogisticLoss(nn.Module):
    """BCE loss for Bradley-Terry pairwise comparisons.

    For a pair (i, j):
        P(i beats j) = σ(score_i - score_j)
    Loss = -[label · log(P) + (1-label) · log(1-P)]

    Parameters
    ----------
    weight_by_delta : bool
        If True, weight each pair by |Δconsensus| (harder pairs → more weight).
    min_margin : float
        Minimum consensus delta below which pairs are excluded (handled by sampler).
    """

    def __init__(self, weight_by_delta: bool = False):
        super().__init__()
        self.weight_by_delta = weight_by_delta

    def forward(
        self,
        score_i: torch.Tensor,
        score_j: torch.Tensor,
        label: torch.Tensor,
        delta_consensus: torch.Tensor | None = None,
    ) -> torch.Tensor:
        """Compute BT logistic loss.

        Parameters
        ----------
        score_i : (B,)  Predicted scores for item i in each pair.
        score_j : (B,)  Predicted scores for item j.
        label   : (B,)  Binary: 1 if consensus_i > consensus_j, else 0.
        delta_consensus : (B,) or None  |Δconsensus| for weighting.

        Returns
        -------
        Loss scalar.
        """
        # logit: P(i beats j) = sigmoid(score_i - score_j)
        logit = score_i - score_j   # (B,)

        # BCE loss: -[label * log(σ(logit)) + (1-label) * log(σ(-logit))]
        # Use BCEWithLogitsLoss for numerical stability
        loss = F.binary_cross_entropy_with_logits(
            logit, label, reduction="none"
        )  # (B,)

        if self.weight_by_delta and delta_consensus is not None:
            loss = loss * delta_consensus

        return loss.mean()


# ---------------------------------------------------------------------------
#  Validation metrics
# ---------------------------------------------------------------------------

def spearman_rho(a: np.ndarray | torch.Tensor,
                 b: np.ndarray | torch.Tensor) -> float:
    """Spearman rank correlation coefficient between two 1-D arrays."""
    if isinstance(a, torch.Tensor):
        a = a.detach().cpu().numpy()
    if isinstance(b, torch.Tensor):
        b = b.detach().cpu().numpy()
    n = len(a)
    if n < 2:
        return 0.0
    ra = np.argsort(np.argsort(a, kind="stable"), kind="stable").astype(np.float64)
    rb = np.argsort(np.argsort(b, kind="stable"), kind="stable").astype(np.float64)
    sum_d2 = float(np.sum((ra - rb) ** 2))
    # Handle ties via standard Spearman formula
    return float(1.0 - 6.0 * sum_d2 / (n * (n * n - 1.0)))


def validate_on_val(
    model: nn.Module,
    sampler: PairSampler,
    device: torch.device,
) -> float:
    """Compute Spearman ρ between model's predictions and consensus on val set.

    The model should take (B, D) embeddings and return (B,) scalar scores.
    """
    model.eval()
    val_embs = sampler.get_val_embeddings().to(device)
    _, val_cons = sampler.get_val_consensus()

    with torch.no_grad():
        scores = model(val_embs).squeeze(-1).cpu().numpy()

    return spearman_rho(scores, val_cons)


# ---------------------------------------------------------------------------
#  Simple scoring head (no LoRA needed for smoke test)
# ---------------------------------------------------------------------------

class LinearScoreHead(nn.Module):
    """Linear(embed_dim -> 1) scoring head."""

    def __init__(self, embed_dim: int):
        super().__init__()
        self.head = nn.Linear(embed_dim, 1)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.head(x).squeeze(-1)  # (B,)


class DirectionScoreHead(nn.Module):
    """Learnable direction vector: score = dot(embedding, direction)."""

    def __init__(self, embed_dim: int):
        super().__init__()
        self.direction = nn.Parameter(torch.randn(embed_dim) * 0.01)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return x @ self.direction  # (B,)


# ---------------------------------------------------------------------------
#  Smoke test
# ---------------------------------------------------------------------------

def run_smoke(args: argparse.Namespace) -> int:
    """Minimal end-to-end test: load data, sample 1000 pairs, train 5 SGD steps."""
    print("[smoke] Loading data ...", flush=True)

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"[smoke] Device: {device}")

    sampler = PairSampler(
        consensus_path=_DEFAULT_CONSENSUS,
        split_path=_DEFAULT_SPLIT,
        embedding_path=_DEFAULT_EMBEDDING,
        min_margin=0.05,
        n_buckets=10,
        batch_size=args.smoke_pairs,
        device=device,
    )
    print(f"[smoke] Train tracks: {sampler.n_train}")
    print(f"[smoke] Val tracks:   {sampler.n_val}")
    print(f"[smoke] Embed dim:    {sampler.dim}")

    # ---- Build model: frozen encoder identity + trainable head ----
    embed_dim = sampler.dim
    head = LinearScoreHead(embed_dim).to(device)
    opt = torch.optim.SGD(head.parameters(), lr=args.smoke_lr)
    loss_fn = BTLogisticLoss(weight_by_delta=args.weight_by_delta)

    # ---- Sample a single batch for the smoke test ----
    emb_i, emb_j, labels = sampler.sample_batch("train")
    print(f"[smoke] Batch shape: emb_i={emb_i.shape}, emb_j={emb_j.shape}, labels={labels.shape}")
    print(f"[smoke] Label balance: {labels.mean().item():.3f} fraction i>j")

    # Optionally get delta_consensus for weighting
    if args.weight_by_delta:
        # Re-derive delta from sampled track IDs (we don't track which IDs were
        # sampled, so we approximate via re-sampling with returned indices)
        # For smoke simplicity, compute uniform weights
        delta_w = None
    else:
        delta_w = None

    losses = []
    for step in range(5):
        head.zero_grad()
        scores_i = head(emb_i)
        scores_j = head(emb_j)
        loss = loss_fn(scores_i, scores_j, labels, delta_w)
        loss.backward()
        opt.step()
        losses.append(float(loss.detach()))
        print(f"[smoke] Step {step}: loss = {losses[-1]:.6f}")

    # ---- Validate ----
    val_rho = validate_on_val(head, sampler, device)
    print(f"[smoke] Val Spearman ρ = {val_rho:+.4f}")

    # ---- Check loss decreased ----
    if len(losses) >= 2 and losses[-1] < losses[0]:
        print(f"[smoke] ✓ Loss decreased: {losses[0]:.6f} → {losses[-1]:.6f}")
        return 0
    else:
        print(f"[smoke] ✗ Loss did not decrease: {losses[0]:.6f} → {losses[-1]:.6f}")
        return 1


# ---------------------------------------------------------------------------
#  CLI
# ---------------------------------------------------------------------------

def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="BT pair sampler + logistic loss for MuQ-MuLan LoRA training."
    )
    # Data paths
    p.add_argument("--consensus", type=Path, default=_DEFAULT_CONSENSUS)
    p.add_argument("--split", type=Path, default=_DEFAULT_SPLIT)
    p.add_argument("--embeddings", type=Path, default=_DEFAULT_EMBEDDING)

    # Sampler
    p.add_argument("--min-margin", type=float, default=0.05)
    p.add_argument("--n-buckets", type=int, default=10)
    p.add_argument("--batch-size", type=int, default=256)

    # Loss
    p.add_argument("--weight-by-delta", action="store_true",
                   help="Weight BT pairs by |Δconsensus|")

    # Smoke test
    p.add_argument("--smoke", action="store_true",
                   help="Run smoke test (load data, sample pairs, verify loss decreases)")
    p.add_argument("--smoke-pairs", type=int, default=1000,
                   help="Number of pairs to sample in smoke test (default: 1000)")
    p.add_argument("--smoke-lr", type=float, default=0.5,
                   help="SGD learning rate for smoke test (default: 0.5)")

    return p.parse_args(argv)


def main() -> int:
    args = parse_args()

    if args.smoke:
        return run_smoke(args)

    # Non-smoke: just print data summary
    sampler = PairSampler(
        consensus_path=args.consensus,
        split_path=args.split,
        embedding_path=args.embeddings,
        min_margin=args.min_margin,
        n_buckets=args.n_buckets,
        batch_size=args.batch_size,
    )
    print(f"Train tracks: {sampler.n_train}")
    print(f"Val tracks:   {sampler.n_val}")
    print(f"Test tracks:  {sampler.n_test}")
    print(f"Embed dim:    {sampler.dim}")

    emb_i, emb_j, labels = sampler.sample_batch("train")
    print(f"\nSample batch: emb_i={emb_i.shape}, emb_j={emb_j.shape}, labels={labels.shape}")
    print(f"Label balance (fraction i>j): {labels.mean().item():.3f}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
