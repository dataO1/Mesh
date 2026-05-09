"""Validate the exported ONNX matches the PyTorch reference within tolerance.

Cosine ≥ 0.9999 per output is the gate (per the spike plan in
documents/embedding-models-research.md).

**Multi-output (round-7.7 Phase 1a, 2026-05-09):** the ONNX now emits
TWO outputs from a single forward pass:
  - `audio_embedding_1024` — pre-projection Conformer hidden states,
    mean-pooled over time. Used by the V18 intensity probe.
  - `audio_embedding_512`  — post-projection L2-normalized joint-space
    embedding. Used for music-text similarity / clustering.

This script validates BOTH outputs against the PyTorch reference path
that produces them. The PyTorch reference for the 512-d output is the
existing `mulan(wavs=raw_waveform)` call. For the 1024-d output we
mirror the wrapper's path: `MuQModel.get_predictions(mel, is_features_only=True)`
then mean-pool the returned hidden states over time.

The exported ONNX takes a **normalized mel-spectrogram** as input (Rust
will compute mel + normalize externally). PyTorch's 512-d reference
takes a raw waveform and computes its own mel internally; the 1024-d
reference uses the same precomputed normalized mel that ONNX consumes,
so both paths see identical numerical inputs.

To keep the comparison apples-to-apples we feed the model exactly ONE
10-second clip (the model's `clip_secs`), so PyTorch's clip-splitting +
per-clip averaging is a no-op.

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
    """Run all cases through the PyTorch MuQMuLan reference (raw waveform in).
    Returns the 512-d L2-normalized joint-space embeddings."""
    results: dict[str, np.ndarray] = {}
    with torch.no_grad():
        for label, wav in cases:
            t = torch.from_numpy(wav).unsqueeze(0).to(device).float()
            t0 = time.time()
            emb = mulan(wavs=t).cpu().numpy()
            dt = time.time() - t0
            results[label] = emb[0]
            print(f"[validate]   pytorch {device.type:>4} {label:>32}: shape={emb.shape} dt={dt * 1000:.0f}ms (512-d)")
    return results


def run_pytorch_1024(mulan, device: torch.device, mels: dict[str, np.ndarray]) -> dict[str, np.ndarray]:
    """Compute the 1024-d intensity-probe substrate via PyTorch on the same
    pre-normalized mel that ONNX consumes. Mirrors the export wrapper path:
    `encoder(mel, is_features_only=True)` → hidden states (last layer if tuple)
    mean-pooled over time → (1024,).

    We call the encoder DIRECTLY (`muq_model.encoder`) rather than going
    through `get_predictions`, because the standard `get_predictions` runs
    its own STFT preprocessing on a raw-waveform input — the export wrapper
    monkey-patches that away, but validate.py runs against an unpatched
    model. The encoder itself takes mel directly."""
    muq_model = mulan.mulan.audio.model.model
    encoder = muq_model.encoder
    results: dict[str, np.ndarray] = {}
    with torch.no_grad():
        for label, mel in mels.items():
            mel_t = torch.from_numpy(mel).to(device).float()
            t0 = time.time()
            _logits, hidden, _new_mask = encoder(mel_t, is_features_only=True)
            # Same tuple/tensor handling as the export wrapper.
            if isinstance(hidden, (tuple, list)):
                hidden = hidden[-1]
            audio_1024 = hidden.mean(dim=1).cpu().numpy()
            dt = time.time() - t0
            results[label] = audio_1024[0]
            print(f"[validate]   pytorch {device.type:>4} {label:>32}: shape={audio_1024.shape} dt={dt * 1000:.0f}ms (1024-d hidden)")
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


def run_onnx(onnx_path: str, provider: str, mels: dict[str, np.ndarray]) -> dict[str, dict[str, np.ndarray]]:
    """Run all cases through the ONNX runtime with the given execution provider.

    `mels` is {label: precomputed normalized mel of shape (1, n_mels, T)}.
    Returns `{output_name: {label: array}}` so the caller can compare each
    named output against its corresponding PyTorch reference.
    """
    import onnxruntime as ort

    available = ort.get_available_providers()
    if provider not in available:
        print(f"[validate] provider {provider} not available (have: {available}); skipping")
        return {}

    print(f"[validate] loading ONNX on {provider}...")
    session = ort.InferenceSession(onnx_path, providers=[provider])
    input_name = session.get_inputs()[0].name
    output_meta = session.get_outputs()
    output_names = [o.name for o in output_meta]
    print(f"[validate]   ONNX outputs: {output_names}")

    # Initialize per-output result containers
    results: dict[str, dict[str, np.ndarray]] = {n: {} for n in output_names}
    for label, mel in mels.items():
        t0 = time.time()
        outs = session.run(output_names, {input_name: mel})
        dt = time.time() - t0
        shapes = ", ".join(f"{n}={tuple(o.shape)}" for n, o in zip(output_names, outs))
        print(f"[validate]   onnx    {provider:>26} {label:>32}: dt={dt * 1000:.0f}ms  [{shapes}]")
        for n, o in zip(output_names, outs):
            results[n][label] = o[0]
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

    # Pre-compute normalized mels once; PyTorch 1024-d ref + ONNX share them.
    print("[validate] computing normalized mels for ONNX input...")
    mels = {label: compute_normalized_mel(mulan, device, wav) for label, wav in cases}
    for label, mel in mels.items():
        print(f"[validate]   mel {label:>32}: shape={mel.shape}")

    # PyTorch references for both heads
    pt_512 = run_pytorch(mulan, device, cases)            # 512-d via raw-waveform
    pt_1024 = run_pytorch_1024(mulan, device, mels)       # 1024-d via shared mel

    # ONNX outputs (multi-output, per execution provider)
    cpu_outputs = run_onnx(onnx_path, "CPUExecutionProvider", mels)
    cpu_ok = True
    if cpu_outputs:
        if "audio_embedding_512" in cpu_outputs:
            cpu_ok &= compare(pt_512, cpu_outputs["audio_embedding_512"], "ONNX-CPU 512")
        if "audio_embedding_1024" in cpu_outputs:
            cpu_ok &= compare(pt_1024, cpu_outputs["audio_embedding_1024"], "ONNX-CPU 1024")
        # Back-compat: if old single-output ONNX, the only output may be unnamed
        if "audio_embedding_512" not in cpu_outputs and "audio_embedding_1024" not in cpu_outputs:
            # Legacy single-output ONNX path
            only_out = next(iter(cpu_outputs.values()))
            cpu_ok &= compare(pt_512, only_out, "ONNX-CPU (legacy single-output)")

    cuda_ok = True
    if device.type == "cuda":
        cuda_outputs = run_onnx(onnx_path, "CUDAExecutionProvider", mels)
        if cuda_outputs:
            if "audio_embedding_512" in cuda_outputs:
                cuda_ok &= compare(pt_512, cuda_outputs["audio_embedding_512"], "ONNX-CUDA 512")
            if "audio_embedding_1024" in cuda_outputs:
                cuda_ok &= compare(pt_1024, cuda_outputs["audio_embedding_1024"], "ONNX-CUDA 1024")
            if "audio_embedding_512" not in cuda_outputs and "audio_embedding_1024" not in cuda_outputs:
                only_out = next(iter(cuda_outputs.values()))
                cuda_ok &= compare(pt_512, only_out, "ONNX-CUDA (legacy single-output)")
        else:
            print("[validate] (onnxruntime-gpu not installed — skipping CUDA EP check)")

    overall = cpu_ok and cuda_ok
    print(f"[validate] OVERALL: {'PASS' if overall else 'FAIL'}")
    return 0 if overall else 1


if __name__ == "__main__":
    sys.exit(main())
