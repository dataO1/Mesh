"""Export the MuQ-MuLan audio tower to ONNX.

The model is a CLIP-style dual encoder; we export only the audio side
(`mulan(wavs=...)` → 512-d joint-space embedding). The text tower is a
follow-up, gated on this spike succeeding.

Strategy:
  1. Load `MuQMuLan` on the best device (CUDA if available, else CPU).
  2. Wrap it in a thin nn.Module so torch.onnx.export sees positional args.
  3. Trace at fp32 with a 30-second waveform input @ 24 kHz (720_000 samples).
  4. Try torch.onnx.export first (TorchScript-based; produces self-contained
     ONNX with inline weights). Fall back to torch.onnx.dynamo_export if it
     trips on the Conformer or Mel-RVQ ops.

The ONNX file is device-agnostic — exporting on GPU just makes the trace
faster, the resulting `.onnx` runs on CPU or GPU at inference time via
`ort` execution providers.

Output: a single .onnx at the path passed as argv[2].

Usage: python export.py [device] [output_path]
  device       — "cuda" or "cpu" (default: cuda if available)
  output_path  — where to write the ONNX (default: ./muq-mulan-audio-tower.onnx)
"""
from __future__ import annotations

import os
import sys
import time
from pathlib import Path

import torch
from torch import nn

# ─── tunables ───────────────────────────────────────────────────────────────
SAMPLE_RATE = 24_000
DURATION_S = 30
INPUT_SAMPLES = SAMPLE_RATE * DURATION_S  # 720_000
ONNX_OPSET = 17
# Dynamic batch + dynamic length so a single ONNX can cover any clip length.
DYNAMIC_AXES = {
    "wavs": {0: "batch", 1: "samples"},
    "audio_embedding": {0: "batch"},
}


class MuQMuLanAudioWrapper(nn.Module):
    """Wraps MuQMuLan to expose `forward(wavs)` → 512-d audio embedding.

    The upstream API is `mulan(wavs=..., texts=...)` with kwargs; ONNX
    export needs a positional-arg forward. This wrapper also pins the call
    to the audio path so the text tower is excluded from the trace.
    """

    def __init__(self, mulan):
        super().__init__()
        self.mulan = mulan

    def forward(self, wavs: torch.Tensor) -> torch.Tensor:
        # `mulan(wavs=...)` returns the joint-space audio embedding.
        return self.mulan(wavs=wavs)


def pick_device(requested: str | None) -> torch.device:
    if requested == "cpu":
        return torch.device("cpu")
    if requested == "cuda":
        if not torch.cuda.is_available():
            raise SystemExit("[export] --cuda requested but torch.cuda.is_available() is False")
        return torch.device("cuda")
    # Auto-detect.
    if torch.cuda.is_available():
        return torch.device("cuda")
    return torch.device("cpu")


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

    wrapper = MuQMuLanAudioWrapper(mulan).eval()

    # Smoke-test the forward pass before tracing — fail fast on shape errors.
    print(f"[export] forward smoke test (batch=1, samples={INPUT_SAMPLES})")
    dummy = torch.randn(1, INPUT_SAMPLES, device=device, dtype=torch.float32)
    with torch.no_grad():
        out = wrapper(dummy)
    print(f"[export] forward OK — output shape: {tuple(out.shape)}, dtype: {out.dtype}")

    # Sanity-check the embedding dim matches our expectation (512 per the
    # MuQ-MuLan model card / config.json).
    if out.dim() != 2 or out.shape[1] != 512:
        print(
            f"[export] WARNING: expected (batch, 512) audio embedding, got {tuple(out.shape)}. "
            "Continuing — downstream consumers will see whatever dim ORT reports.",
            file=sys.stderr,
        )

    # Ensure parent dir exists.
    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    print(f"[export] tracing → {output_path} (opset {ONNX_OPSET})")
    t0 = time.time()
    try:
        with torch.no_grad():
            torch.onnx.export(
                wrapper,
                (dummy,),
                output_path,
                input_names=["wavs"],
                output_names=["audio_embedding"],
                dynamic_axes=DYNAMIC_AXES,
                opset_version=ONNX_OPSET,
                do_constant_folding=True,
                # dynamo=False uses the legacy TorchScript exporter; it produces
                # a single self-contained .onnx with inline weights, no .data
                # sidecar. More reliable for unusual ops than the dynamo path.
                dynamo=False,
            )
        print(f"[export] torch.onnx.export OK ({time.time() - t0:.1f}s)")
    except Exception as e:
        print(f"[export] torch.onnx.export FAILED: {e}", file=sys.stderr)
        print("[export] retrying with torch.onnx.dynamo_export...", file=sys.stderr)
        try:
            with torch.no_grad():
                onnx_program = torch.onnx.dynamo_export(wrapper, dummy)
                onnx_program.save(output_path)
            print(f"[export] dynamo_export OK ({time.time() - t0:.1f}s)")
        except Exception as e2:
            print(f"[export] dynamo_export ALSO FAILED: {e2}", file=sys.stderr)
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
