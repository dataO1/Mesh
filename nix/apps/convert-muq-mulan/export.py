"""Export the MuQ-MuLan audio tower to ONNX.

The model is a CLIP-style dual encoder; we export only the audio side
(`mulan(wavs=...)` → 512-d joint-space embedding). The text tower is a
follow-up, gated on this spike succeeding.

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
DYNAMIC_AXES = {
    "mel": {0: "batch", 2: "frames"},
    "audio_embedding": {0: "batch"},
}


class MuQMuLanMelWrapper(nn.Module):
    """Wraps MuQMuLan to expose `forward(mel)` → 512-d audio embedding.

    `mel` shape: (batch, n_mels=128, time). Caller (Rust) is responsible
    for computing the mel with matching parameters and applying the
    mean/std normalization documented in the sidecar `.norm.json`.

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

    def forward(self, mel: torch.Tensor) -> torch.Tensor:
        # `get_audio_latents` runs: AudioSpectrogramTransformerPretrained
        # → MuQ.forward → MuQModel.get_predictions (our patched version)
        # → returned hidden_states[use_layer_idx] → proj → transformer
        # → mean → audio_to_latents → l2norm → 512-d.
        return self.mulan.mulan_module.get_audio_latents(mel)


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
        out = wrapper(dummy_mel)
    print(f"[export] forward OK — output shape: {tuple(out.shape)}, dtype: {out.dtype}")

    if out.dim() != 2 or out.shape[1] != 512:
        print(
            f"[export] WARNING: expected (batch, 512) audio embedding, got {tuple(out.shape)}. "
            "Continuing — downstream consumers will see whatever dim ORT reports.",
            file=sys.stderr,
        )

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    print(f"[export] tracing → {output_path} (opset {ONNX_OPSET})")
    t0 = time.time()
    try:
        with torch.no_grad():
            torch.onnx.export(
                wrapper,
                (dummy_mel,),
                output_path,
                input_names=["mel"],
                output_names=["audio_embedding"],
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
                    output_names=["audio_embedding"],
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
    return 0


if __name__ == "__main__":
    sys.exit(main())
