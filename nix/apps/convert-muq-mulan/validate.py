"""Validate the exported ONNX matches the PyTorch reference within tolerance.

Cosine ≥ 0.9999 per output is the gate (per the spike plan in
documents/embedding-models-research.md).

The exported ONNX takes a **normalized mel-spectrogram** as input (Rust
will compute mel + normalize externally). PyTorch's reference still
takes a raw waveform and computes its own mel internally. To keep the
comparison apples-to-apples we feed the model exactly ONE 10-second
clip (the model's `clip_secs`), so PyTorch's clip-splitting + per-clip
averaging is a no-op.

For each test case, we:
  1. Run PyTorch reference: `mulan(wavs=raw_waveform)` → 512-d.
  2. Compute the same MuQ-internal mel on the raw waveform via the
     model's own `preprocessor_melspec_2048` + `stat` dict.
  3. Run ONNX: `session.run(mel=normalized_mel)` → 512-d.
  4. Cosine-compare.

Test cases:
  1. A pure sine wave — deterministic, isolates numerical drift.
  2. White noise      — exercises wider spectral content.
  3. A real audio file — only if argv[3] is provided.

Also runs the ONNX through the CUDA execution provider (`onnxruntime-gpu`)
when available, to confirm the production GPU path won't surprise us.

Usage: python validate.py <onnx_path> [device] [optional_audio_file]
  onnx_path           — path to the exported .onnx file
  device              — "cuda"/"cpu" (default: cuda if available)
  optional_audio_file — wav/mp3/flac to validate against (skipped if missing)
"""
from __future__ import annotations

import os
import sys
import time

import numpy as np
import torch

SAMPLE_RATE = 24_000
CLIP_SECS = 10                          # match MuQ-MuLan's `clip_secs`
INPUT_SAMPLES = SAMPLE_RATE * CLIP_SECS # 240_000 → exactly one clip

COSINE_GATE = 0.9999  # per spike plan


def cosine(a: np.ndarray, b: np.ndarray) -> float:
    a = a.flatten().astype(np.float64)
    b = b.flatten().astype(np.float64)
    na = np.linalg.norm(a)
    nb = np.linalg.norm(b)
    if na < 1e-12 or nb < 1e-12:
        return 0.0
    return float(np.dot(a, b) / (na * nb))


def pick_device(requested: str | None) -> torch.device:
    if requested == "cpu":
        return torch.device("cpu")
    if requested == "cuda":
        if not torch.cuda.is_available():
            raise SystemExit("[validate] --cuda requested but cuda not available")
        return torch.device("cuda")
    return torch.device("cuda" if torch.cuda.is_available() else "cpu")


def make_test_inputs(audio_file: str | None):
    """Return a list of (label, np.ndarray of shape [INPUT_SAMPLES]) cases."""
    cases: list[tuple[str, np.ndarray]] = []

    # 1. Sine wave at 220 Hz — deterministic, low-spectral-content baseline.
    t = np.arange(INPUT_SAMPLES, dtype=np.float32) / SAMPLE_RATE
    cases.append(("sine_220hz", 0.5 * np.sin(2 * np.pi * 220.0 * t).astype(np.float32)))

    # 2. White noise — exercises wider spectral content.
    rng = np.random.default_rng(seed=42)
    cases.append(("white_noise", rng.normal(0.0, 0.3, INPUT_SAMPLES).astype(np.float32)))

    # 3. Real audio if provided.
    if audio_file and os.path.exists(audio_file):
        try:
            import librosa

            wav, _ = librosa.load(audio_file, sr=SAMPLE_RATE, mono=True, duration=CLIP_SECS)
            if len(wav) < INPUT_SAMPLES:
                wav = np.pad(wav, (0, INPUT_SAMPLES - len(wav)))
            else:
                wav = wav[:INPUT_SAMPLES]
            cases.append((f"real:{os.path.basename(audio_file)}", wav.astype(np.float32)))
        except Exception as e:
            print(f"[validate] WARNING: failed to load real audio {audio_file}: {e}", file=sys.stderr)

    return cases


def load_pytorch(device: torch.device):
    """Load MuQMuLan reference model. Used both for PyTorch inference and
    for computing the normalized mel that feeds the ONNX."""
    print(f"[validate] loading PyTorch MuQMuLan on {device}...")
    from muq import MuQMuLan

    return MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large").to(device).eval()


def run_pytorch(mulan, device: torch.device, cases) -> dict[str, np.ndarray]:
    """Run all cases through the PyTorch MuQMuLan reference (raw waveform in)."""
    results: dict[str, np.ndarray] = {}
    with torch.no_grad():
        for label, wav in cases:
            t = torch.from_numpy(wav).unsqueeze(0).to(device).float()
            t0 = time.time()
            emb = mulan(wavs=t).cpu().numpy()
            dt = time.time() - t0
            results[label] = emb[0]
            print(f"[validate]   pytorch {device.type:>4} {label:>32}: shape={emb.shape} dt={dt * 1000:.0f}ms")
    return results


def compute_normalized_mel(mulan, device: torch.device, wav: np.ndarray) -> np.ndarray:
    """Compute the same mel + normalization that MuQModel.get_predictions
    used to do internally — but on the host so we can feed ONNX directly."""
    muq_model = mulan.mulan.audio.model.model
    preproc = muq_model.preprocessor_melspec_2048
    preproc.to(device)
    stat = muq_model.stat
    mean = stat["melspec_2048_mean"]
    std = stat["melspec_2048_std"]

    with torch.no_grad():
        x = torch.from_numpy(wav).unsqueeze(0).to(device).float()
        # MelSTFT is callable; matches preprocessing()'s `layer(x.float())[..., :-1]`.
        mel = preproc(x)[..., :-1]
        mel = (mel - mean) / std
    return mel.cpu().numpy().astype(np.float32)


def run_onnx(onnx_path: str, provider: str, mels: dict[str, np.ndarray]) -> dict[str, np.ndarray]:
    """Run all cases through the ONNX runtime with the given execution provider.

    `mels` is {label: precomputed normalized mel of shape (1, n_mels, T)}.
    """
    import onnxruntime as ort

    available = ort.get_available_providers()
    if provider not in available:
        print(f"[validate] provider {provider} not available (have: {available}); skipping")
        return {}

    print(f"[validate] loading ONNX on {provider}...")
    session = ort.InferenceSession(onnx_path, providers=[provider])
    input_name = session.get_inputs()[0].name
    output_name = session.get_outputs()[0].name

    results: dict[str, np.ndarray] = {}
    for label, mel in mels.items():
        t0 = time.time()
        out = session.run([output_name], {input_name: mel})[0]
        dt = time.time() - t0
        results[label] = out[0]
        print(f"[validate]   onnx    {provider:>26} {label:>32}: shape={out.shape} dt={dt * 1000:.0f}ms")
    return results


def compare(pt_results: dict, onnx_results: dict, source_label: str) -> bool:
    """Compare PyTorch vs ONNX outputs; return True if all pass the cosine gate."""
    all_pass = True
    print(f"[validate] cosine vs PyTorch — gate ≥ {COSINE_GATE}:")
    for label in pt_results:
        if label not in onnx_results:
            continue
        cos = cosine(pt_results[label], onnx_results[label])
        passed = cos >= COSINE_GATE
        flag = "OK" if passed else "FAIL"
        print(f"[validate]   {source_label:>32} {label:>32}: cos={cos:.6f}  {flag}")
        if not passed:
            all_pass = False
    return all_pass


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    onnx_path = sys.argv[1]
    requested_device = sys.argv[2] if len(sys.argv) > 2 else None
    audio_file = sys.argv[3] if len(sys.argv) > 3 else None

    if not os.path.exists(onnx_path):
        print(f"[validate] ONNX missing: {onnx_path}", file=sys.stderr)
        return 1

    device = pick_device(requested_device)
    cases = make_test_inputs(audio_file)
    print(f"[validate] {len(cases)} test cases: {[c[0] for c in cases]}")

    mulan = load_pytorch(device)

    pt = run_pytorch(mulan, device, cases)

    # Pre-compute normalized mels once; both CPU and CUDA EPs share them.
    print("[validate] computing normalized mels for ONNX input...")
    mels = {label: compute_normalized_mel(mulan, device, wav) for label, wav in cases}
    for label, mel in mels.items():
        print(f"[validate]   mel {label:>32}: shape={mel.shape}")

    cpu_results = run_onnx(onnx_path, "CPUExecutionProvider", mels)
    cpu_ok = compare(pt, cpu_results, "ONNX-CPU")

    cuda_ok = True
    if device.type == "cuda":
        cuda_results = run_onnx(onnx_path, "CUDAExecutionProvider", mels)
        if cuda_results:
            cuda_ok = compare(pt, cuda_results, "ONNX-CUDA")
        else:
            print("[validate] (onnxruntime-gpu not installed — skipping CUDA EP check)")

    overall = cpu_ok and cuda_ok
    print(f"[validate] OVERALL: {'PASS' if overall else 'FAIL'}")
    return 0 if overall else 1


if __name__ == "__main__":
    sys.exit(main())
