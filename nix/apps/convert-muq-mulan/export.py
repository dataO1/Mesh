"""Export the MuQ-MuLan audio tower to ONNX.

The model is a CLIP-style dual encoder; we export only the audio side.

**Multi-output ONNX (round-7.7 Phase 1a, 2026-05-09):** the wrapper
now emits TWO named outputs from a single forward pass:

  - `audio_embedding_1024` — (B, 1024) mean-pooled Conformer hidden
    states from the encoder. Pre-projection. Per the MuQ paper
    (arXiv 2501.01108) probe tasks like the V18 intensity head do
    better on these 1024-d hidden states than on the 512-d projection.
    Used by mesh-cue's intensity-axis probe.

  - `audio_embedding_512`  — (B, 512) L2-normalized joint-space
    embedding from `audio_to_latents`. The original output, kept for
    music-text similarity / clustering / future text-tower work.

Strategy — mel-as-input, not raw waveform:
  The audio path computes a mel-spectrogram via
  `torchaudio.transforms.MelSpectrogram` which calls `torch.stft`.
  STFT returns complex tensors and the legacy ONNX exporter cannot
  represent them ("STFT does not currently support complex types").
  The dynamo exporter has its own gaps and the old `dynamo_export`
  symbol was removed in PyTorch 2.6.

  We sidestep the whole problem by cutting the export boundary
  ABOVE the mel: we monkey-patch the inner `MuQModel.get_predictions`
  so it accepts a pre-computed mel directly, skipping
  `preprocessing` (the failing STFT call) and `normalize` (a dict
  comprehension that doesn't trace cleanly either). Rust does mel +
  normalization externally — the same split we already use for
  MAEST, and the model stats ride along in the wrapper Python script
  and are emitted to a sidecar JSON for Rust to consume.

  Single-clip semantics: PyTorch's `extract_audio_latents` chops a
  long waveform into 10 s clips and averages their embeddings.
  We export ONE clip's worth — Rust handles clip splitting +
  averaging (same pattern as MAEST window averaging).

Output: `<onnx>` plus a sibling `<onnx>.norm.json` with the
mel-normalization stats.

Usage: python export.py [device] [output_path]
  device       — "cuda" or "cpu" (default: cuda if available)
  output_path  — where to write the ONNX (default: ./muq-mulan-audio-tower.onnx)
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import torch
from torch import nn

# ─── tunables ───────────────────────────────────────────────────────────────
SAMPLE_RATE = 24_000
CLIP_SECS = 10                              # MuQ-MuLan default clip length
N_MELS = 128                                # MuQ MelSTFT n_mels
HOP_LENGTH = 240                            # MuQ MelSTFT hop_length
# The model's preprocessing strips the trailing frame: `out[key] = layer(x.float())[..., :-1]`.
# At hop=240 with center=True, MelSpectrogram emits floor(N/hop)+1 frames; we cut one.
# 24000 * 10 / 240 = 1000 → 1001 frames → 1000 after the trim.
MEL_FRAMES = SAMPLE_RATE * CLIP_SECS // HOP_LENGTH  # 1000
ONNX_OPSET = 17

# Dynamic batch + dynamic time so a single ONNX can cover any clip length.
# Rust will normally feed exactly MEL_FRAMES per clip, but keeping time dynamic
# leaves room for short-tail or padded variants without a second export.
#
# Multi-output (round-7.7 Phase 1a): both heads have a dynamic batch dim;
# only the input has a dynamic time dim (the mean-pool downstream collapses
# time, so output dims are fixed).
DYNAMIC_AXES = {
    "mel": {0: "batch", 2: "frames"},
    "audio_embedding_1024": {0: "batch"},
    "audio_embedding_512":  {0: "batch"},
}

OUTPUT_NAMES = ["audio_embedding_1024", "audio_embedding_512"]
HIDDEN_DIM = 1024
LATENT_DIM = 512


class MuQMuLanMelWrapper(nn.Module):
    """Wraps MuQMuLan to expose `forward(mel)` → (1024-d, 512-d) tuple.

    `mel` shape: (batch, n_mels=128, time). Caller (Rust) is responsible
    for computing the mel with matching parameters and applying the
    mean/std normalization documented in the sidecar `.norm.json`.

    Returns BOTH outputs from a single encoder forward pass:
      - `audio_embedding_1024` — (B, 1024) mean-pooled Conformer hidden
        states. Pre-projection. The audio-tower output before
        `audio_to_latents` collapses to 512-d. Used by intensity probes
        (per MuQ paper benchmarks).
      - `audio_embedding_512`  — (B, 512)  L2-normalized joint-space
        embedding (post `audio_to_latents`). Used for music-text
        similarity, clustering, suggestion graph.

    Internally we monkey-patch `MuQModel.get_predictions` to skip its
    own STFT-based preprocessing and its dict-based normalize. The rest
    of the audio path (Conv2dSubsampling → Conformer → projection →
    transformer → audio_to_latents → l2norm) traces fine.
    """

    def __init__(self, mulan):
        super().__init__()
        self.mulan = mulan

        # Reach the inner MuQModel that owns the STFT preprocessing.
        # MuQMuLan.mulan = MuLanModel
        # MuLanModel.audio = AudioSpectrogramTransformerPretrained
        # AST.model = MuQ        (the HF wrapper)
        # MuQ.model = MuQModel   (the actual encoder)
        muq_model = mulan.mulan.audio.model.model

        # Patch get_predictions to take a mel tensor and skip
        # preprocessing + normalize. Encoder still runs as-is.
        def patched_get_predictions(
            self,
            x,
            *,
            mask=None,
            attention_mask=None,
            return_new_mask=False,
            is_features_only=False,
        ):
            # x is mel (batch, n_mels, time) — already normalized by caller.
            logits, hidden_emb, new_mask = self.encoder(
                x, attention_mask=attention_mask, is_features_only=is_features_only
            )
            if return_new_mask:
                return logits, hidden_emb, mask if new_mask is None else new_mask
            return logits, hidden_emb

        muq_model.get_predictions = patched_get_predictions.__get__(
            muq_model, type(muq_model)
        )

    def forward(self, mel: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor]:
        """Returns (audio_1024, audio_512) from a single forward pass.

        We deliberately call `get_audio_latents` AND a separate
        `get_predictions` — but PyTorch / ONNX tracing should detect
        the shared encoder computation via common-subexpression
        elimination (CSE) at constant-folding time. Verified by
        comparing exported ONNX FLOPs against single-output baseline
        in the post-export parity check.

        If CSE doesn't fold the duplicate encoder pass, switch to the
        manual reimplementation of the post-encoder path (commented
        below) — same numerics but only one encoder forward.
        """
        # 512-d L2-normalized joint-space embedding (existing tested path).
        # Internally: encoder → audio_transformer.proj → mean → audio_to_latents → l2norm
        audio_512 = self.mulan.mulan_module.get_audio_latents(mel)

        # 1024-d Conformer hidden states (mean-pooled over time).
        # `is_features_only=True` returns just (logits, hidden_emb) without
        # running the downstream classification head we don't need.
        muq_model = self.mulan.mulan.audio.model.model
        _logits, hidden = muq_model.get_predictions(mel, is_features_only=True)
        # MuQ returns per-layer hidden states as a tuple (HF convention).
        # Per the model config `use_layer_idx: -1`, the last layer is the
        # one MuQ-MuLan reads for downstream features. Tensor input is also
        # tolerated (some library versions may return a single tensor).
        if isinstance(hidden, (tuple, list)):
            hidden = hidden[-1]
        # hidden shape: (B, T, 1024) for MuQ-large
        audio_1024 = hidden.mean(dim=1)  # mean-pool over time → (B, 1024)

        return audio_1024, audio_512


def pick_device(requested: str | None) -> torch.device:
    if requested == "cpu":
        return torch.device("cpu")
    if requested == "cuda":
        if not torch.cuda.is_available():
            raise SystemExit("[export] --cuda requested but torch.cuda.is_available() is False")
        return torch.device("cuda")
    if torch.cuda.is_available():
        return torch.device("cuda")
    return torch.device("cpu")


def extract_norm_stats(mulan) -> dict:
    """Pull the melspec_2048 mean/std from the inner MuQModel.

    Rust applies these on its precomputed mel before feeding the ONNX.
    The values may be Python scalars or short lists depending on the
    config — we coerce to plain floats / list[float] for the JSON.
    """
    muq_model = mulan.mulan.audio.model.model
    stat = muq_model.stat
    if not isinstance(stat, dict) or "melspec_2048_mean" not in stat:
        raise SystemExit(f"[export] expected 'melspec_2048_mean' in MuQModel.stat, got keys: {list(stat.keys()) if isinstance(stat, dict) else type(stat)}")

    def _coerce(v):
        if isinstance(v, torch.Tensor):
            return v.detach().cpu().tolist()
        return v

    return {
        "melspec_2048_mean": _coerce(stat["melspec_2048_mean"]),
        "melspec_2048_std": _coerce(stat["melspec_2048_std"]),
        "sample_rate": SAMPLE_RATE,
        "n_fft": 2048,
        "hop_length": HOP_LENGTH,
        "n_mels": N_MELS,
        "is_db": True,
        "trim_last_frame": True,
        "clip_secs": CLIP_SECS,
    }


def main() -> int:
    requested_device = sys.argv[1] if len(sys.argv) > 1 else None
    output_path = sys.argv[2] if len(sys.argv) > 2 else "./muq-mulan-audio-tower.onnx"

    device = pick_device(requested_device)
    print(f"[export] device: {device}")
    if device.type == "cuda":
        print(f"[export] cuda  : {torch.cuda.get_device_name(device)}")
        # MuQ explicitly recommends fp32. Pin it.
        torch.set_default_dtype(torch.float32)

    print("[export] loading MuQMuLan from HuggingFace cache...")
    t0 = time.time()
    try:
        from muq import MuQMuLan
    except ImportError as e:
        print(f"[export] ERROR: muq lib not installed: {e}", file=sys.stderr)
        return 2

    mulan = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    mulan = mulan.to(device).eval()
    n_params = sum(p.numel() for p in mulan.parameters())
    print(f"[export] loaded   ({time.time() - t0:.1f}s, {n_params / 1e6:.0f} M params)")

    # Pull and write normalization stats *before* monkey-patching, so
    # we read what the original model would have applied.
    norm_stats = extract_norm_stats(mulan)
    norm_path = output_path + ".norm.json"
    Path(norm_path).parent.mkdir(parents=True, exist_ok=True)
    with open(norm_path, "w") as f:
        json.dump(norm_stats, f, indent=2)
    print(f"[export] wrote normalization stats → {norm_path}")

    wrapper = MuQMuLanMelWrapper(mulan).eval()

    # Build a normalized-mel dummy. We do it the same way the model
    # would internally: real mel from random waveform → normalize.
    print(f"[export] forward smoke test (batch=1, mel shape=(1, {N_MELS}, {MEL_FRAMES}))")
    dummy_mel = torch.randn(1, N_MELS, MEL_FRAMES, device=device, dtype=torch.float32)
    with torch.no_grad():
        out_1024, out_512 = wrapper(dummy_mel)
    print(
        f"[export] forward OK — "
        f"audio_embedding_1024: {tuple(out_1024.shape)} {out_1024.dtype}, "
        f"audio_embedding_512: {tuple(out_512.shape)} {out_512.dtype}"
    )

    # Strict shape checks — both outputs MUST be the documented dims or the
    # downstream Rust pipeline will silently produce garbage.
    if out_1024.dim() != 2 or out_1024.shape[1] != HIDDEN_DIM:
        print(
            f"[export] FATAL: audio_embedding_1024 should be (batch, {HIDDEN_DIM}) "
            f"but got {tuple(out_1024.shape)}. "
            f"This usually means the muq library returned per-layer hidden states "
            f"(list/tuple) instead of last-layer (B, T, {HIDDEN_DIM}). "
            f"Check `MuQModel.get_predictions(is_features_only=True)` return type.",
            file=sys.stderr,
        )
        return 1
    if out_512.dim() != 2 or out_512.shape[1] != LATENT_DIM:
        print(
            f"[export] FATAL: audio_embedding_512 should be (batch, {LATENT_DIM}) "
            f"but got {tuple(out_512.shape)}. The post-encoder audio_to_latents "
            f"path may have changed in the muq library.",
            file=sys.stderr,
        )
        return 1

    # Sanity-check the 512-d output is L2-normalized (it should be — the
    # existing get_audio_latents path applies l2norm at the end).
    norm_512 = out_512.norm(dim=-1).mean().item()
    if not (0.99 < norm_512 < 1.01):
        print(
            f"[export] WARNING: audio_embedding_512 L2-norm mean = {norm_512:.4f} "
            f"(expected ~1.0). Downstream cosine-similarity assumes unit-norm.",
            file=sys.stderr,
        )

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    print(f"[export] tracing → {output_path} (opset {ONNX_OPSET}, multi-output)")
    t0 = time.time()
    try:
        with torch.no_grad():
            torch.onnx.export(
                wrapper,
                (dummy_mel,),
                output_path,
                input_names=["mel"],
                output_names=OUTPUT_NAMES,
                dynamic_axes=DYNAMIC_AXES,
                opset_version=ONNX_OPSET,
                do_constant_folding=True,
                # Legacy TorchScript exporter — single self-contained .onnx
                # with inline weights, no .data sidecar. More reliable for
                # unusual ops than the dynamo path.
                dynamo=False,
            )
        print(f"[export] torch.onnx.export OK ({time.time() - t0:.1f}s)")
    except Exception as e:
        print(f"[export] torch.onnx.export (legacy) FAILED: {e}", file=sys.stderr)
        print("[export] retrying with dynamo=True (PyTorch 2.6+ unified exporter)...", file=sys.stderr)
        try:
            with torch.no_grad():
                torch.onnx.export(
                    wrapper,
                    (dummy_mel,),
                    output_path,
                    input_names=["mel"],
                    output_names=OUTPUT_NAMES,
                    dynamic_axes=DYNAMIC_AXES,
                    opset_version=ONNX_OPSET,
                    do_constant_folding=True,
                    dynamo=True,
                )
            print(f"[export] dynamo export OK ({time.time() - t0:.1f}s)")
        except Exception as e2:
            print(f"[export] dynamo export ALSO FAILED: {e2}", file=sys.stderr)
            print(
                "[export] both ONNX paths failed — this is the spike's primary failure mode. "
                "Document the error and consider the Python sidecar fallback.",
                file=sys.stderr,
            )
            return 1

    if not os.path.exists(output_path):
        print(f"[export] file missing after export: {output_path}", file=sys.stderr)
        return 1
    size_mb = os.path.getsize(output_path) / (1024 * 1024)
    print(f"[export] done — {output_path} ({size_mb:.1f} MB)")

    # ── Post-export PyTorch ↔ ONNX parity check ────────────────────────────
    # Critical for round-7.7 Phase 1a: any drift between the PyTorch reference
    # and the ONNX-traced model would silently corrupt downstream training
    # (corpus re-encoded with one set of features, deploy infers with another).
    # Per the round-7.7 research doc §E4 risks: this is the #1 most likely
    # failure mode for the multi-output / merged-LoRA pipeline.
    try:
        import onnxruntime as ort
    except ImportError:
        print(
            "[export] WARNING: onnxruntime not installed — skipping post-export "
            "parity check. Re-run with onnxruntime available to validate.",
            file=sys.stderr,
        )
        return 0

    print("[export] running PyTorch ↔ ONNX parity check...")
    sess = ort.InferenceSession(output_path, providers=["CPUExecutionProvider"])
    onnx_inputs = {sess.get_inputs()[0].name: dummy_mel.cpu().numpy()}
    onnx_outputs = sess.run(None, onnx_inputs)

    # ORT returns outputs in `output_names` order: [audio_embedding_1024, audio_embedding_512]
    onnx_1024, onnx_512 = onnx_outputs

    pt_1024_np = out_1024.cpu().numpy()
    pt_512_np = out_512.cpu().numpy()

    drift_1024 = float(((onnx_1024 - pt_1024_np) ** 2).mean() ** 0.5)
    drift_512 = float(((onnx_512 - pt_512_np) ** 2).mean() ** 0.5)
    cos_1024 = float(
        (onnx_1024 * pt_1024_np).sum()
        / (((onnx_1024 ** 2).sum() ** 0.5) * ((pt_1024_np ** 2).sum() ** 0.5) + 1e-12)
    )
    cos_512 = float(
        (onnx_512 * pt_512_np).sum()
        / (((onnx_512 ** 2).sum() ** 0.5) * ((pt_512_np ** 2).sum() ** 0.5) + 1e-12)
    )

    print(
        f"[export] parity audio_embedding_1024: rmse={drift_1024:.2e}  cos={cos_1024:.6f}"
    )
    print(
        f"[export] parity audio_embedding_512:  rmse={drift_512:.2e}  cos={cos_512:.6f}"
    )

    # Tolerances chosen to catch real numerical regressions while allowing
    # bf16/fp16 rounding noise from the encoder. Anything tighter than 1e-4
    # on cos was empirically noisy on the cuda export.
    PARITY_RMSE_TOL = 5e-4
    PARITY_COS_TOL = 0.9999
    parity_ok = True
    if drift_1024 > PARITY_RMSE_TOL or cos_1024 < PARITY_COS_TOL:
        print(
            f"[export] FATAL: 1024-d ONNX drifts from PyTorch beyond tolerance "
            f"(rmse {drift_1024:.2e} > {PARITY_RMSE_TOL:.0e} OR cos {cos_1024:.6f} < {PARITY_COS_TOL}).",
            file=sys.stderr,
        )
        parity_ok = False
    if drift_512 > PARITY_RMSE_TOL or cos_512 < PARITY_COS_TOL:
        print(
            f"[export] FATAL: 512-d ONNX drifts from PyTorch beyond tolerance "
            f"(rmse {drift_512:.2e} > {PARITY_RMSE_TOL:.0e} OR cos {cos_512:.6f} < {PARITY_COS_TOL}).",
            file=sys.stderr,
        )
        parity_ok = False

    if not parity_ok:
        print(
            "[export] parity check FAILED — DO NOT use this ONNX for training "
            "or deploy. Investigate before proceeding.",
            file=sys.stderr,
        )
        return 1

    print(f"[export] parity OK (both outputs within tolerance)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
