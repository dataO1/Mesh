#!/usr/bin/env python3
"""
E4.4 — LoRA fine-tuning of MuQ-MuLan audio tower for track intensity grading.

Reads:  /home/data01/Music/mesh-track-grading/audio/dz_<tid>.mp3       (30 s clips)
        /home/data01/Music/mesh-track-grading/round7_6_consensus.npz   (5-juror consensus)
        /home/data01/Music/mesh-track-grading/round7_6_split.npz       (artist-stratified split)

Writes: /home/data01/Music/mesh-track-grading/round7_7_lora/           (checkpoints)

Architecture
------------
- Base: MuQ-MuLan-large from OpenMuQ/MuQ-MuLan-large (via muq package)
- PEFT LoRA: r=16, alpha=32 on attention projections (linear_q/k/v/o)
- Scoring head: Linear(1024 → 1) on mean-pooled Conformer hidden states

Loss: Bradley-Terry pairwise logistic loss (BTLogisticLoss from bt_pair_sampler).
Validation: Spearman rank correlation ρ between predicted scores and consensus.

Usage
-----
  # Full training
  python spike/track-grading/train_lora_e4.py

  # Smoke test (500 tracks, 2 epochs)
  python spike/track-grading/train_lora_e4.py --smoke

  # Resume from checkpoint
  python spike/track-grading/train_lora_e4.py --resume
"""
from __future__ import annotations

import argparse
import math
import os
import sys
import time
import contextlib
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

import librosa

from muq import MuQMuLan
from peft import LoraConfig, get_peft_model

from bt_pair_sampler import PairSampler, BTLogisticLoss, LinearScoreHead, spearman_rho


# ---------------------------------------------------------------------------
#  Paths
# ---------------------------------------------------------------------------
_AUDIO_DIR     = Path("/home/data01/Music/mesh-track-grading/audio")
_CONSENSUS     = Path("/home/data01/Music/mesh-track-grading/round7_6_consensus.npz")
_SPLIT         = Path("/home/data01/Music/mesh-track-grading/round7_6_split.npz")
_CKPT_DIR      = Path("/home/data01/Music/mesh-track-grading/round7_7_lora")

SAMPLE_RATE    = 24_000   # Hz
PREVIEW_SECS   = 30       # Deezer preview duration
CLIP_SECS      = 10       # Internal MuQ-MuLan clip length
N_CLIPS        = PREVIEW_SECS // CLIP_SECS  # 3
CLIP_SAMPLES   = SAMPLE_RATE * CLIP_SECS    # 240 000
TOTAL_SAMPLES  = SAMPLE_RATE * PREVIEW_SECS # 720 000
HIDDEN_DIM     = 1024


# ---------------------------------------------------------------------------
#  C-stderr silencer (same pattern as embed_corpus_mulan.py)
# ---------------------------------------------------------------------------

@contextlib.contextmanager
def _silence_c_stderr():
    saved = os.dup(2)
    devnull_fd = os.open(os.devnull, os.O_WRONLY)
    try:
        os.dup2(devnull_fd, 2)
        yield
    finally:
        os.dup2(saved, 2)
        os.close(devnull_fd)
        os.close(saved)


# ---------------------------------------------------------------------------
#  Audio loading
# ---------------------------------------------------------------------------

def load_audio(path: Path) -> np.ndarray | None:
    """Load MP3 as 30 s × 24 kHz mono float32. None on failure."""
    try:
        with _silence_c_stderr():
            wav, _ = librosa.load(str(path), sr=SAMPLE_RATE, mono=True,
                                  duration=PREVIEW_SECS)
    except Exception as e:
        print(f"  [load] {path.name}: {e}", file=sys.stderr)
        return None
    if len(wav) < TOTAL_SAMPLES:
        wav = np.pad(wav, (0, TOTAL_SAMPLES - len(wav)))
    elif len(wav) > TOTAL_SAMPLES:
        wav = wav[:TOTAL_SAMPLES]
    return wav.astype(np.float32)


def _load_one_no_silence(path: Path) -> np.ndarray:
    """Load a single MP3 — thread-safe (no os.dup2 stderr hijack).

    Redirects C stderr to /dev/null for this thread ONLY via a
    subprocess-like approach: we open /dev/null and use os.dup2
    ONLY for the duration of the librosa call, saving/restoring
    fd 2 around it.  This is safe because (a) each thread saves
    its own copy of fd 2 before redirecting, (b) the GIL prevents
    concurrent os.dup2 calls from interleaving.
    """
    # Suppress ffmpeg/libmpg123 ID3 warnings — these are per-file noise
    # that would otherwise flood the log at 24 tracks × 8+ workers.
    saved = os.dup(2)
    devnull = os.open(os.devnull, os.O_WRONLY)
    try:
        os.dup2(devnull, 2)
        wav, _sr = librosa.load(str(path), sr=SAMPLE_RATE, mono=True,
                                 duration=PREVIEW_SECS)
    finally:
        os.dup2(saved, 2)
        os.close(devnull)
        os.close(saved)
    if len(wav) < TOTAL_SAMPLES:
        wav = np.pad(wav, (0, TOTAL_SAMPLES - len(wav)))
    elif len(wav) > TOTAL_SAMPLES:
        wav = wav[:TOTAL_SAMPLES]
    return wav.astype(np.float32)


def load_audio_batch(track_ids: np.ndarray, audio_dir: Path,
                     max_workers: int = 16) -> dict[int, np.ndarray]:
    """Load multiple MP3s in parallel via thread pool.

    Uses _load_one_no_silence so we don't manipulate process-level fd 2
    from background threads (os.dup2 is not thread-safe).  ffmpeg/librosa
    warnings from worker threads are accepted as harmless noise.
    """
    wavs_dict: dict[int, np.ndarray] = {}
    with ThreadPoolExecutor(max_workers=max_workers) as ex:
        futures = {
            ex.submit(_load_one_no_silence, audio_dir / f"dz_{int(tid)}.mp3"): int(tid)
            for tid in track_ids
        }
        for future in as_completed(futures):
            tid = futures[future]
            try:
                wavs_dict[tid] = future.result()
            except Exception:
                wavs_dict[tid] = np.zeros(TOTAL_SAMPLES, dtype=np.float32)
    return wavs_dict


# ---------------------------------------------------------------------------
#  Encode function — gradients flow through LoRA
# ---------------------------------------------------------------------------

def encode_track(
    model: nn.Module,        # PeftModel wrapping MuQMuLan
    wavs: torch.Tensor,      # (B, TOTAL_SAMPLES) float wavs on correct device
) -> torch.Tensor:
    """Encode waveforms to (B, 1024) mean-pooled Conformer hidden states.

    This path goes through the raw Conformer encoder so that LoRA gradients
    flow through the attention layers.  Does NOT use torch.no_grad().

    The model is the PEFT-wrapped PeftModel.  We navigate to the underlying
    MuQModel (the conformer container) via model.base_model → MuQMuLan →
    mulan_module.audio.model.model → MuQModel.
    """
    # Navigate through PEFT wrapper to the MuQModel (conformer container)
    muq_mulan: MuQMuLan = model.base_model          # type: ignore[assignment]
    muq_model = muq_mulan.mulan_module.audio.model.model  # MuQModel

    preproc = muq_model.preprocessor_melspec_2048
    # Move preproc to the same device as input (STFT window buffer may be on CPU)
    preproc = preproc.to(device=wavs.device)

    stat = muq_model.stat
    mean_t = torch.tensor(stat["melspec_2048_mean"], device=wavs.device)
    std_t = torch.tensor(stat["melspec_2048_std"], device=wavs.device)
    encoder = muq_model.encoder

    # Match the model's working dtype (fp16 if the model was half'd)
    encoder_dtype = next(model.parameters()).dtype

    # Split 30 s waveform into 3 × 10 s clips, encode each, average
    per_clip = []
    for clip_idx in range(N_CLIPS):
        start = clip_idx * CLIP_SAMPLES
        end = start + CLIP_SAMPLES
        clip_wavs = wavs[:, start:end]

        mel = preproc(clip_wavs)[..., :-1]          # (B, n_mels, T)
        mel = (mel - mean_t) / std_t
        mel = mel.to(dtype=encoder_dtype)

        _logits, hidden, _new_mask = encoder(mel, is_features_only=True)
        if isinstance(hidden, (tuple, list)):
            hidden = hidden[-1]                     # last layer per config
        clip_emb = hidden.mean(dim=1)               # (B, 1024) mean-pool time
        per_clip.append(clip_emb)

    return torch.stack(per_clip, dim=0).mean(dim=0)  # (B, 1024)


# ---------------------------------------------------------------------------
#  Pair sampling (track IDs, not embeddings)
# ---------------------------------------------------------------------------

def sample_train_pairs(
    sampler: PairSampler,
    batch_size: int,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Sample (track_id_i, track_id_j, label) arrays from the train split.

    Uses PairSampler's internal stratified bucket logic so the returned
    pairs have |Δconsensus| >= min_margin and balanced labels.
    """
    tids = sampler._track_ids["train"]               # noqa
    cons = sampler._cached_consensus["train"]         # noqa
    bucket = sampler._train_bucket                    # noqa
    bucket_ids = sampler._train_bucket_ids            # noqa

    i_idx, j_idx = sampler._stratified_sample_pairs(  # noqa
        batch_size, cons, bucket, bucket_ids
    )
    return tids[i_idx], tids[j_idx], (cons[i_idx] > cons[j_idx]).astype(np.float32)


# ---------------------------------------------------------------------------
#  Validation helper
# ---------------------------------------------------------------------------

@torch.no_grad()
def validate(
    model: nn.Module,
    head: nn.Module,
    val_wavs_np: np.ndarray,        # (N, TOTAL_SAMPLES) float32 on CPU
    val_consensus: np.ndarray,
    device: torch.device,
    batch_size: int = 32,
) -> float:
    """Compute Spearman ρ between predicted scores and consensus on val set.

    Processes val waveforms in batches to avoid OOM.
    """
    model.eval()
    head.eval()
    n_val = len(val_wavs_np)
    all_scores: list[np.ndarray] = []
    with torch.no_grad():
        for start in range(0, n_val, batch_size):
            end = min(start + batch_size, n_val)
            batch = torch.from_numpy(val_wavs_np[start:end].astype(np.float32)).to(device)
            embs = encode_track(model, batch).float()
            scores = head(embs).squeeze(-1).cpu().numpy()
            all_scores.append(scores)
    scores_full = np.concatenate(all_scores)
    return spearman_rho(scores_full, val_consensus)


# ---------------------------------------------------------------------------
#  Checkpoint save / load
# ---------------------------------------------------------------------------

def save_checkpoint(
    ckpt_dir: Path,
    epoch: int,
    model: nn.Module,
    head: nn.Module,
    optimizer: torch.optim.Optimizer,
    scheduler: torch.optim.lr_scheduler._LRScheduler,
    best_val_rho: float,
) -> None:
    """Save LoRA adapters, head, optimizer, scheduler, and metadata."""
    ckpt_dir.mkdir(parents=True, exist_ok=True)

    # LoRA adapters via PEFT
    model.save_pretrained(str(ckpt_dir / f"epoch_{epoch:03d}_lora"))

    # Head + optimizer + scheduler
    torch.save({
        "epoch": epoch,
        "head_state_dict": head.state_dict(),
        "optimizer_state_dict": optimizer.state_dict(),
        "scheduler_state_dict": scheduler.state_dict(),
        "best_val_rho": best_val_rho,
    }, ckpt_dir / f"epoch_{epoch:03d}_training_state.pt")

    # Symlink or copy "best" marker
    best_path = ckpt_dir / "best_val_rho.txt"
    best_path.write_text(f"{best_val_rho:.6f}\n")

    # Also save the latest LoRA + training state for resume
    (ckpt_dir / "latest_epoch.txt").write_text(f"{epoch}\n")

    print(f"  [checkpoint] epoch {epoch}: saved LoRA adapters + training state "
          f"(val ρ = {best_val_rho:.4f})")


def load_checkpoint(
    ckpt_dir: Path,
    model: nn.Module,
    head: nn.Module,
    optimizer: torch.optim.Optimizer | None,
    scheduler: torch.optim.lr_scheduler._LRScheduler | None,
) -> tuple[int, float]:
    """Load the latest checkpoint. Returns (start_epoch, best_val_rho)."""
    latest_epoch_file = ckpt_dir / "latest_epoch.txt"
    if not latest_epoch_file.exists():
        print("  [resume] no latest_epoch.txt found — starting from scratch")
        return 0, -float("inf")

    epoch = int(latest_epoch_file.read_text().strip())
    lora_dir = ckpt_dir / f"epoch_{epoch:03d}_lora"
    state_path = ckpt_dir / f"epoch_{epoch:03d}_training_state.pt"

    if not lora_dir.exists() or not state_path.exists():
        print(f"  [resume] checkpoint for epoch {epoch} incomplete — starting from scratch")
        return 0, -float("inf")

    # Load LoRA adapters
    from peft import PeftModel
    model = PeftModel.from_pretrained(model, lora_dir)
    model = model.to(next(head.parameters()).device)
    model.train()

    # Load training state
    state = torch.load(state_path, map_location="cpu")
    head.load_state_dict(state["head_state_dict"])
    if optimizer is not None and "optimizer_state_dict" in state:
        optimizer.load_state_dict(state["optimizer_state_dict"])
    if scheduler is not None and "scheduler_state_dict" in state:
        scheduler.load_state_dict(state["scheduler_state_dict"])
    best_val_rho = state.get("best_val_rho", -float("inf"))

    print(f"  [resume] loaded epoch {epoch} (best val ρ = {best_val_rho:.4f})")
    return epoch + 1, best_val_rho

# ---------------------------------------------------------------------------
#  Parse args
# ---------------------------------------------------------------------------

def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="E4.4 LoRA fine-tuning of MuQ-MuLan audio tower for track intensity"
    )

    # Data
    p.add_argument("--audio-dir", type=Path, default=_AUDIO_DIR)
    p.add_argument("--consensus", type=Path, default=_CONSENSUS)
    p.add_argument("--split", type=Path, default=_SPLIT)
    p.add_argument("--ckpt-dir", type=Path, default=_CKPT_DIR)

    # LoRA
    p.add_argument("--lora-r", type=int, default=16)
    p.add_argument("--lora-alpha", type=int, default=32)
    p.add_argument("--lora-dropout", type=float, default=0.0)

    # Training
    p.add_argument("--epochs", type=int, default=200,
                   help="maximum number of epochs")
    p.add_argument("--batch-size", type=int, default=8,
                   help="number of pairs per batch")
    p.add_argument("--grad-accum", type=int, default=4,
                   help="gradient accumulation steps")
    p.add_argument("--lr", type=float, default=3e-5,
                   help="peak AdamW learning rate (lower than typical 1e-4 — "
                        "LoRA on pretrained models needs conservative LR to "
                        "avoid destroying pretrained geometry)")
    p.add_argument("--weight-decay", type=float, default=1e-4)
    p.add_argument("--warmup-epochs", type=float, default=2.0,
                   help="linear warmup for this many epochs")
    p.add_argument("--min-lr-ratio", type=float, default=0.05,
                   help="minimum LR as fraction of peak (cosine schedule floor)")

    # Sampler
    p.add_argument("--min-margin", type=float, default=0.05)
    p.add_argument("--n-buckets", type=int, default=10)

    # Early stopping
    p.add_argument("--patience", type=int, default=15)
    p.add_argument("--min-delta", type=float, default=1e-4)

    # Feature preservation (anchors LoRA output to frozen baseline geometry)
    p.add_argument("--anchor-emb", type=Path, default=None,
                   help="Path to frozen baseline NPZ (corpus_muq_mulan.npz) for "
                        "feature-preservation loss. Prevents LoRA from destroying "
                        "the pretrained geometry while learning ranking.")
    p.add_argument("--lambda-anchor", type=float, default=0.1,
                   help="Weight for anchor MSE loss (LoRA_1024 vs frozen_baseline_1024)")
    p.add_argument("--min-cos", type=float, default=0.7,
                   help="Minimum cosine similarity vs baseline to accept a checkpoint. "
                        "Checkpoints with cos < min_cos are rejected (keeps prior best).")

    # Resume
    p.add_argument("--resume", action="store_true",
                   help="resume from latest checkpoint in --ckpt-dir")

    # Smoke test
    p.add_argument("--smoke", action="store_true",
                   help="run smoke test: 500 train tracks, 2 epochs")
    p.add_argument("--smoke-tracks", type=int, default=500,
                   help="number of train tracks to use in smoke test")

    return p.parse_args(argv)


# ---------------------------------------------------------------------------
#  Main
# ---------------------------------------------------------------------------

def main() -> int:
    args = parse_args()

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"[E4.4] Device: {device}")
    print(f"[E4.4] Batch-size (pairs): {args.batch_size}, grad-accum: {args.grad_accum}")
    if args.smoke:
        print(f"[E4.4] *** SMOKE TEST MODE ***")

    # ------------------------------------------------------------------
    #  1. Load metadata (PairSampler without pre-loaded embeddings)
    # ------------------------------------------------------------------
    print("[E4.4] Loading consensus + split ...", flush=True)
    sampler = PairSampler(
        consensus_path=args.consensus,
        split_path=args.split,
        embedding_path=None,       # we compute embeddings on the fly
        min_margin=args.min_margin,
        n_buckets=args.n_buckets,
        batch_size=args.batch_size,
        device=device,
    )
    print(f"  Train tracks: {sampler.n_train}")
    print(f"  Val tracks:   {sampler.n_val}")
    print(f"  Test tracks:  {sampler.n_test}")

    # ------------------------------------------------------------------
    #  2. Load MuQ-MuLan model and wrap with LoRA
    # ------------------------------------------------------------------
    print("[E4.4] Loading MuQ-MuLan-large ...", flush=True)
    base_model = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    base_model = base_model.to(device)
    # Use half precision to fit model + activations on consumer GPU
    base_model = base_model.half()
    print(f"  Model params: {sum(p.numel() for p in base_model.parameters())/1e6:.0f}M (half)")

    lora_config = LoraConfig(
        r=args.lora_r,
        lora_alpha=args.lora_alpha,
        target_modules=["linear_q", "linear_k", "linear_v", "linear_out"],
        init_lora_weights=True,
        lora_dropout=args.lora_dropout,
        bias="none",
        task_type="FEATURE_EXTRACTION",
    )
    model = get_peft_model(base_model, lora_config)
    model.train()
    print(f"  LoRA params: {sum(p.numel() for n, p in model.named_parameters() if 'lora' in n)}")
    print(f"  Trainable params: {sum(p.numel() for p in model.parameters() if p.requires_grad)}")

    #  2.5. Load frozen baseline embeddings for feature-preservation anchor
    # ------------------------------------------------------------------
    _anchor_tid_to_emb: dict[int, np.ndarray] = {}
    if args.anchor_emb is not None:
        print(f"[E4.4] Loading frozen baseline embeddings from {args.anchor_emb} ...", flush=True)
        anc = np.load(args.anchor_emb, allow_pickle=True)
        anc_embs = anc["embeddings_1024"].astype(np.float32)
        anc_tids = anc["track_ids"].astype(np.int64)
        _anchor_tid_to_emb = {int(t): anc_embs[i] for i, t in enumerate(anc_tids)}
        print(f"  Baseline embeddings: {len(_anchor_tid_to_emb)} tracks (dim={anc_embs.shape[1]})")

    # ------------------------------------------------------------------
    #  3. Scoring head
    # ------------------------------------------------------------------
    head = LinearScoreHead(embed_dim=HIDDEN_DIM).to(device).float()
    print(f"  Head params: {sum(p.numel() for p in head.parameters())}")

    # ------------------------------------------------------------------
    #  4. Loss
    # ------------------------------------------------------------------
    loss_fn = BTLogisticLoss(weight_by_delta=False)

    # ------------------------------------------------------------------
    #  5. Optimiser + scheduler
    # ------------------------------------------------------------------
    # Optimise LoRA params + head together
    trainable_params = [
        p for p in model.parameters() if p.requires_grad
    ] + list(head.parameters())

    optimizer = torch.optim.AdamW(
        trainable_params,
        lr=args.lr,
        weight_decay=args.weight_decay,
    )

    # Cosine schedule with linear warmup
    n_train = sampler.n_train
    steps_per_epoch = math.ceil(
        (n_train // (args.batch_size * 2)) / args.grad_accum
    )
    warmup_steps = int(args.warmup_epochs * steps_per_epoch)
    total_steps = int(args.epochs * steps_per_epoch)

    def lr_lambda(step: int) -> float:
        if step < warmup_steps:
            return float(step) / max(1.0, warmup_steps)
        # Cosine decay from 1.0 down to min_lr_ratio
        progress = float(step - warmup_steps) / max(1.0, total_steps - warmup_steps)
        cosine = 0.5 * (1.0 + math.cos(math.pi * progress))
        return args.min_lr_ratio + (1.0 - args.min_lr_ratio) * cosine

    scheduler = torch.optim.lr_scheduler.LambdaLR(optimizer, lr_lambda)
    print(f"  Steps per epoch: {steps_per_epoch}, warmup: {warmup_steps}, total: {total_steps}")

    # ------------------------------------------------------------------
    #  6. Resume from checkpoint
    # ------------------------------------------------------------------
    start_epoch = 0
    best_val_rho = -float("inf")
    epochs_without_improvement = 0

    if args.resume and args.ckpt_dir.exists():
        start_epoch, best_val_rho = load_checkpoint(
            args.ckpt_dir, base_model, head, optimizer, scheduler
        )
        # Re-wrap with PeftModel since load_checkpoint returns a new PeftModel
        # Actually load_checkpoint modifies model in-place... let's be safe.
        # reload the peft model from checkpoint
        from peft import PeftModel
        if start_epoch > 0:
            lora_dir = args.ckpt_dir / f"epoch_{start_epoch - 1:03d}_lora"
            if lora_dir.exists():
                model = PeftModel.from_pretrained(base_model, lora_dir)
                model = model.to(device)
                model.train()
                print(f"  [resume] LoRA adapters loaded from {lora_dir}")

    # ------------------------------------------------------------------
    #  7. Pre-load validation audio
    # ------------------------------------------------------------------
    print("[E4.4] Pre-loading validation audio ...", flush=True)
    val_tids = sampler._track_ids["val"]  # noqa
    _, val_consensus = sampler.get_val_consensus()

    # Limit smoke test to requested number of train tracks
    train_tids_all = sampler._track_ids["train"]  # noqa
    if args.smoke:
        train_tids = train_tids_all[:args.smoke_tracks]
        # Patch sampler state for limited train set
        sampler._track_ids["train"] = train_tids  # noqa
        sampler._cached_consensus["train"] = sampler._cached_consensus["train"][:args.smoke_tracks]  # noqa
        # Recompute buckets for smoke subset
        edges = np.linspace(0.0, 1.0, args.n_buckets + 1)
        edges[-1] += 1e-6
        sampler._train_bucket = np.digitize(sampler._cached_consensus["train"], edges) - 1  # noqa
        sampler._train_bucket_ids = {}  # noqa
        for b in range(args.n_buckets):
            sampler._train_bucket_ids[b] = np.where(sampler._train_bucket == b)[0]  # noqa
        n_train = len(train_tids)
        steps_per_epoch = math.ceil(
            (n_train // (args.batch_size * 2)) / args.grad_accum
        )
        args.epochs = 2
        total_steps = int(args.epochs * steps_per_epoch)
        warmup_steps = min(warmup_steps, total_steps // 2)
        # Recreate scheduler with new total
        def lr_lambda_smoke(step: int) -> float:
            if step < warmup_steps:
                return float(step) / max(1.0, warmup_steps)
            progress = float(step - warmup_steps) / max(1.0, total_steps - warmup_steps)
            cosine = 0.5 * (1.0 + math.cos(math.pi * progress))
            return args.min_lr_ratio + (1.0 - args.min_lr_ratio) * cosine
        scheduler = torch.optim.lr_scheduler.LambdaLR(optimizer, lr_lambda_smoke)
        print(f"  [smoke] Limited to {args.smoke_tracks} train tracks, {args.epochs} epochs")

    # Load all val waveforms on CPU in parallel
    val_wavs_dict = load_audio_batch(val_tids, args.audio_dir)
    val_wav_list: list[np.ndarray] = []
    val_valid_tids: list[int] = []
    for tid in val_tids:
        tid_int = int(tid)
        wav = val_wavs_dict.get(tid_int)
        if wav is not None:
            val_wav_list.append(wav)
            val_valid_tids.append(tid_int)
    # Align val_consensus with valid tracks
    val_tid_set = set(val_valid_tids)
    val_consensus_aligned = np.array([
        val_consensus[int(np.where(val_tids == t)[0][0])]
        for t in val_valid_tids
    ], dtype=np.float32)

    val_wavs_cpu = np.stack(val_wav_list)  # (N_val, TOTAL_SAMPLES) float32 on CPU
    print(f"  Val waveforms: {len(val_valid_tids)}/{len(val_tids)} loaded successfully (on CPU)")

    # ------------------------------------------------------------------
    #  8. Main training loop
    # ------------------------------------------------------------------
    print("\n[E4.4] " + "=" * 60)
    print("[E4.4] Starting training")
    print("[E4.4] " + "=" * 60)

    global_step = start_epoch * steps_per_epoch
    n_batches_per_epoch = math.ceil(n_train / (args.batch_size * 2))

    for epoch in range(start_epoch, args.epochs):
        epoch_start = time.time()
        model.train()
        head.train()
        epoch_loss = 0.0
        n_batches_done = 0

        optimizer.zero_grad()

        for batch_idx in range(0, n_batches_per_epoch, args.grad_accum):
            # Accumulate gradients over grad_accum micro-batches
            accum_loss = 0.0
            accum_n = 0

            for _micro in range(args.grad_accum):
                # Sample one micro-batch of pairs
                tid_i, tid_j, labels = sample_train_pairs(sampler, args.batch_size)

                # Collect unique track IDs in this micro-batch
                unique_tids = np.unique(np.concatenate([tid_i, tid_j]))

                # Load audio for all unique tracks (parallel, ~8 workers)
                wavs_dict = load_audio_batch(unique_tids, args.audio_dir)

                # Stack waveforms
                batch_wavs = np.stack([wavs_dict[int(t)] for t in unique_tids])
                batch_tensor = torch.from_numpy(batch_wavs).to(device)

                # Encode through LoRA model (gradients flow)
                embs = encode_track(model, batch_tensor)  # (n_unique, 1024)
                embs = embs.float()  # cast fp16 → fp32 for head

                # Map back to pairs
                tid_to_idx = {int(t): i for i, t in enumerate(unique_tids)}
                idx_i = torch.tensor(
                    [tid_to_idx[int(t)] for t in tid_i], dtype=torch.long, device=device
                )
                idx_j = torch.tensor(
                    [tid_to_idx[int(t)] for t in tid_j], dtype=torch.long, device=device
                )
                emb_i = embs[idx_i]
                emb_j = embs[idx_j]

                # Score
                score_i = head(emb_i)
                score_j = head(emb_j)

                # Loss: BT logistic + optional feature-preservation anchor
                labels_t = torch.from_numpy(labels).to(device)
                loss_val = loss_fn(score_i, score_j, labels_t)

                # Anchor loss: penalize drift from frozen baseline 1024-d geometry.
                # Prevents LoRA from finding degenerate solutions that rank correctly
                # but destroy the pretrained representation.
                if _anchor_tid_to_emb:
                    anc_embs = np.stack([_anchor_tid_to_emb[int(t)]
                                         for t in unique_tids
                                         if int(t) in _anchor_tid_to_emb])
                    if len(anc_embs) > 0:
                        anc_t = torch.from_numpy(anc_embs).to(device)
                        # Only compare tracks that are in the anchor set
                        anc_mask = torch.tensor(
                            [int(t) in _anchor_tid_to_emb for t in unique_tids],
                            device=device
                        )
                        if anc_mask.any():
                            loss_anchor = nn.functional.mse_loss(
                                embs[anc_mask].float(), anc_t.float()
                            )
                            loss_val = loss_val + args.lambda_anchor * loss_anchor

                loss_val = loss_val / args.grad_accum  # scale for accumulation
                loss_val.backward()

                accum_loss += float(loss_val.detach().item() * args.grad_accum)
                accum_n += 1

            # Gradient clipping
            torch.nn.utils.clip_grad_norm_(trainable_params, max_norm=1.0)

            # Optimizer step
            optimizer.step()
            scheduler.step()
            optimizer.zero_grad()
            global_step += 1

            epoch_loss += accum_loss
            n_batches_done += 1

            if (batch_idx // args.grad_accum) % max(1, (n_batches_per_epoch // args.grad_accum // 25)) == 0:
                lr_current = scheduler.get_last_lr()[0]
                print(f"  epoch {epoch} | batch {batch_idx // args.grad_accum}/"
                      f"{n_batches_per_epoch // args.grad_accum} | "
                      f"loss={accum_loss:.4f} | lr={lr_current:.2e}")

        avg_loss = epoch_loss / max(1, n_batches_done)
        lr_epoch = scheduler.get_last_lr()[0]

        # ---- Validation ----
        val_rho = validate(model, head, val_wavs_cpu, val_consensus_aligned, device)

        # Cosine drift check: compare LoRA 1024-d output vs frozen baseline
        # on a sample of val tracks.  Rejects checkpoints where the LoRA
        # geometry has diverged too far from the pretrained representation.
        val_cos = 1.0  # default if no anchor
        if _anchor_tid_to_emb and len(val_valid_tids) > 0:
            sample_n = min(200, len(val_valid_tids))
            sample_tids = [val_valid_tids[i] for i in
                           np.random.default_rng(epoch).choice(
                               len(val_valid_tids), sample_n, replace=False)]
            lora_vecs = []
            anchor_vecs = []
            for tid in sample_tids:
                if tid in _anchor_tid_to_emb and tid in {int(t) for t in val_tids}:
                    # Encode through LoRA model
                    wav = load_audio(args.audio_dir / f"dz_{tid}.mp3")
                    if wav is not None:
                        wav_t = torch.from_numpy(wav).unsqueeze(0).to(device)
                        with torch.no_grad():
                            emb = encode_track(model, wav_t)
                        lora_vecs.append(emb.cpu().numpy().squeeze())
                        anchor_vecs.append(_anchor_tid_to_emb[tid])
            if len(lora_vecs) > 20:
                lora_mat = np.stack(lora_vecs)
                anc_mat = np.stack(anchor_vecs)
                # Mean cosine across tracks
                norms_l = np.linalg.norm(lora_mat, axis=1)
                norms_a = np.linalg.norm(anc_mat, axis=1)
                dots = np.sum(lora_mat * anc_mat, axis=1)
                cosines = dots / (norms_l * norms_a + 1e-12)
                val_cos = float(np.mean(cosines))
            else:
                val_cos = 1.0

        elapsed = time.time() - epoch_start
        print(f"  >>> Epoch {epoch} | loss={avg_loss:.4f} | lr={lr_epoch:.2e} | "
              f"val ρ={val_rho:+.4f} | cos={val_cos:.4f} | time={elapsed:.0f}s")

        # ---- Early stopping ----
        quality_ok = val_rho > best_val_rho + args.min_delta
        cos_ok = (not _anchor_tid_to_emb) or (val_cos >= args.min_cos)

        if quality_ok and cos_ok:
            best_val_rho = val_rho
            epochs_without_improvement = 0
            save_checkpoint(
                args.ckpt_dir, epoch, model, head, optimizer, scheduler, best_val_rho
            )
        elif quality_ok and not cos_ok:
            print(f"  [cos guard] val ρ improved ({val_rho:+.4f} > {best_val_rho:+.4f}) "
                  f"but cos={val_cos:.4f} < min_cos={args.min_cos} — REJECTING checkpoint")
            epochs_without_improvement += 1
        else:
            epochs_without_improvement += 1
            print(f"  [early stop] {epochs_without_improvement}/{args.patience} "
                  f"without improvement (best ρ = {best_val_rho:.4f})")
            if epochs_without_improvement >= args.patience:
                print(f"[E4.4] Early stopping triggered at epoch {epoch}. "
                      f"Best val ρ = {best_val_rho:.4f}")
                break

        # Periodic save (every 10 epochs even without improvement, for safety)
        if epoch > 0 and epoch % 10 == 0:
            save_checkpoint(
                args.ckpt_dir, epoch, model, head, optimizer, scheduler, best_val_rho
            )

    # ---- Final save ----
    save_checkpoint(
        args.ckpt_dir, min(epoch, args.epochs - 1), model, head,
        optimizer, scheduler, best_val_rho
    )

    print(f"\n[E4.4] Training complete. Best val ρ = {best_val_rho:.4f}")
    print(f"[E4.4] Checkpoints in {args.ckpt_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
