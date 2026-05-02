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

SAMPLE_RATE = 24_000
DURATION_S = 30
INPUT_SAMPLES = SAMPLE_RATE * DURATION_S


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

    # Synthetic input — content doesn't matter for timing.
    x = np.random.default_rng(0).standard_normal((1, INPUT_SAMPLES)).astype(np.float32)

    # 3 warmup runs to amortize first-call overhead (cuDNN autotune, etc.).
    for _ in range(3):
        session.run([output_name], {input_name: x})

    print(f"[bench] {provider}: {n} timed runs (single-window, batch=1)")
    times: list[float] = []
    for i in range(n):
        t0 = time.perf_counter()
        session.run([output_name], {input_name: x})
        times.append((time.perf_counter() - t0) * 1000.0)

    lo, med, p95 = quantiles(times)
    print(f"[bench] {provider}: min={lo:.0f}ms  median={med:.0f}ms  p95={p95:.0f}ms")
    # 4 windows per track per the existing MAEST cap → estimated per-track time.
    per_track = med * 4
    print(f"[bench] {provider}: estimated per-track (4 windows): {per_track:.0f}ms")


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
