"""Wall-clock benchmark of the exported ONNX on CPU + GPU.

Measures per-window inference time so we have actual numbers to plug
into the integration plan (the prior "10s/window CPU" was an estimate
from comparable-param Conformer models, no published MuQ-MuLan benchmark
exists). Reports min/median/p95 over N runs.

Usage: python bench.py <onnx_path> [n_iters]
  onnx_path  — path to the exported .onnx file
  n_iters    — runs per provider (default: 20)
"""
from __future__ import annotations

import os
import sys
import time

import numpy as np

# The ONNX takes a normalized mel-spectrogram per 10 s clip — Rust does
# the mel + normalize externally. For timing we just need the right shape;
# content is irrelevant. 24 kHz / hop 240 / 10 s = 1000 frames, 128 mels.
SAMPLE_RATE = 24_000
CLIP_SECS = 10
N_MELS = 128
HOP_LENGTH = 240
MEL_FRAMES = SAMPLE_RATE * CLIP_SECS // HOP_LENGTH


def quantiles(times_ms: list[float]) -> tuple[float, float, float]:
    arr = np.asarray(times_ms)
    return float(arr.min()), float(np.median(arr)), float(np.percentile(arr, 95))


def bench_provider(onnx_path: str, provider: str, n: int) -> None:
    import onnxruntime as ort

    available = ort.get_available_providers()
    if provider not in available:
        print(f"[bench] provider {provider} not available (have: {available}); skipping")
        return

    print(f"[bench] {provider}: warming up...")
    session = ort.InferenceSession(onnx_path, providers=[provider])
    input_name = session.get_inputs()[0].name
    output_name = session.get_outputs()[0].name

    # Synthetic mel — content doesn't matter for timing.
    x = np.random.default_rng(0).standard_normal((1, N_MELS, MEL_FRAMES)).astype(np.float32)

    # 3 warmup runs to amortize first-call overhead (cuDNN autotune, etc.).
    for _ in range(3):
        session.run([output_name], {input_name: x})

    print(f"[bench] {provider}: {n} timed runs (single 10s clip, batch=1)")
    times: list[float] = []
    for i in range(n):
        t0 = time.perf_counter()
        session.run([output_name], {input_name: x})
        times.append((time.perf_counter() - t0) * 1000.0)

    lo, med, p95 = quantiles(times)
    print(f"[bench] {provider}: min={lo:.0f}ms  median={med:.0f}ms  p95={p95:.0f}ms")
    # MAEST uses 4 evenly-spaced 30s windows. MuQ-MuLan natively averages
    # consecutive 10s clips — open question whether we keep that or
    # subsample. Report a few candidate strides so the integration plan
    # can pick one with real numbers.
    for n_clips in (3, 6, 12):
        print(f"[bench] {provider}: estimated per-track ({n_clips}×10s clips): {med * n_clips:.0f}ms")


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    onnx_path = sys.argv[1]
    n_iters = int(sys.argv[2]) if len(sys.argv) > 2 else 20

    if not os.path.exists(onnx_path):
        print(f"[bench] ONNX missing: {onnx_path}", file=sys.stderr)
        return 1

    bench_provider(onnx_path, "CPUExecutionProvider", n_iters)
    bench_provider(onnx_path, "CUDAExecutionProvider", n_iters)
    return 0


if __name__ == "__main__":
    sys.exit(main())
