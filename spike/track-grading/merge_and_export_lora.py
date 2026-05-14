#!/usr/bin/env python3
"""
E4.4 — Merge trained LoRA adapters into MuQ-MuLan and export to ONNX.

Reads:  /home/data01/Music/mesh-track-grading/round7_7_lora/  (trained LoRA)
Writes: /home/data01/Projects/Mesh/models/muq-mulan-audio-tower-lora.onnx
        (plus <onnx>.norm.json sidecar)

Usage: python spike/track-grading/merge_and_export_lora.py [--ckpt-dir PATH]
"""
from __future__ import annotations

import argparse
import os
import sys
import time
from pathlib import Path

import numpy as np
import torch

# Re-use the export infrastructure from the existing export script
sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent / "nix" / "apps" / "convert-muq-mulan"))

from export import (  # type: ignore[import-not-found]
    MuQMuLanMelWrapper,
    extract_norm_stats,
    SAMPLE_RATE, CLIP_SECS, N_MELS, HOP_LENGTH, MEL_FRAMES,
    ONNX_OPSET, DYNAMIC_AXES, OUTPUT_NAMES, HIDDEN_DIM, LATENT_DIM,
    pick_device,
)

_CKPT_DIR = Path("/home/data01/Music/mesh-track-grading/round7_7_lora")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="E4.4 Merge LoRA into MuQ-MuLan and export ONNX"
    )
    p.add_argument("--ckpt-dir", type=Path, default=_CKPT_DIR,
                   help="Path to LoRA checkpoint directory")
    p.add_argument("--output", type=str,
                   default="/home/data01/Projects/Mesh/models/muq-mulan-audio-tower-lora.onnx",
                   help="Output ONNX path")
    p.add_argument("--device", type=str, default=None,
                   help="'cuda' or 'cpu' (default: cuda if available)")
    return p.parse_args(argv)


def load_merged_model(ckpt_dir: Path, device: torch.device):
    """Load base model, apply trained LoRA, merge and unload.

    Returns the merged (standard) MuQMuLan ready for export.
    """
    from muq import MuQMuLan
    from peft import PeftModel

    print("[merge] Loading base MuQ-MuLan-large ...", flush=True)
    base_model = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    base_model = base_model.to(device)

    # Find the latest LoRA checkpoint
    latest_file = ckpt_dir / "latest_epoch.txt"
    if not latest_file.exists():
        raise SystemExit(f"[merge] No latest_epoch.txt found in {ckpt_dir} — training not complete?")

    epoch = int(latest_file.read_text().strip())
    lora_dir = ckpt_dir / f"epoch_{epoch:03d}_lora"
    if not lora_dir.exists():
        raise SystemExit(f"[merge] LoRA adapter dir {lora_dir} not found")

    print(f"[merge] Loading LoRA adapters from epoch {epoch} ...", flush=True)
    model = PeftModel.from_pretrained(base_model, lora_dir)
    model = model.to(device)

    # Load the scoring head to check best val rho
    state_path = ckpt_dir / f"epoch_{epoch:03d}_training_state.pt"
    if state_path.exists():
        state = torch.load(state_path, map_location="cpu")
        best_rho = state.get("best_val_rho", "unknown")
        print(f"[merge] Best val Spearman ρ = {best_rho}", flush=True)

    print("[merge] Merging LoRA into base weights ...", flush=True)
    merged = model.merge_and_unload()
    merged = merged.to(device).eval()
    print("[merge] Merge complete.", flush=True)

    return merged


def run_parity_check(merged_model, device: torch.device):
    """Verify merged model produces the same outputs as the ONNX will."""
    from peft import PeftModel  # noqa: F811

    print("[parity] Running pre-export parity check ...", flush=True)

    wrapper = MuQMuLanMelWrapper(merged_model).eval()

    dummy_mel = torch.randn(1, N_MELS, MEL_FRAMES, device=device, dtype=torch.float32)
    with torch.no_grad():
        out_1024, out_512 = wrapper(dummy_mel)

    print(f"[parity] Forward OK — 1024: {tuple(out_1024.shape)}, 512: {tuple(out_512.shape)}", flush=True)

    # Check 512-d is L2-normalized
    norm_512 = out_512.norm(dim=-1).mean().item()
    if not (0.99 < norm_512 < 1.01):
        print(f"[parity] WARNING: 512-d L2 norm = {norm_512:.4f} (expected ~1.0)", flush=True)

    return wrapper, dummy_mel, out_1024, out_512


def main() -> int:
    args = parse_args()

    device = pick_device(args.device)
    print(f"[merge] Device: {device}", flush=True)

    # 1. Load + merge
    merged_model = load_merged_model(args.ckpt_dir, device)

    # 2. Write normalization stats
    norm_stats = extract_norm_stats(merged_model)
    norm_path = args.output + ".norm.json"
    Path(norm_path).parent.mkdir(parents=True, exist_ok=True)
    import json
    with open(norm_path, "w") as f:
        json.dump(norm_stats, f, indent=2)
    print(f"[merge] Wrote norm stats → {norm_path}", flush=True)

    # 3. Pre-export parity check
    wrapper, dummy_mel, pt_1024, pt_512 = run_parity_check(merged_model, device)

    # 4. Export ONNX
    print(f"[merge] Exporting ONNX → {args.output} ...", flush=True)
    t0 = time.time()
    Path(args.output).parent.mkdir(parents=True, exist_ok=True)

    try:
        with torch.no_grad():
            torch.onnx.export(
                wrapper,
                (dummy_mel,),
                args.output,
                input_names=["mel"],
                output_names=OUTPUT_NAMES,
                dynamic_axes=DYNAMIC_AXES,
                opset_version=ONNX_OPSET,
                do_constant_folding=True,
                dynamo=False,
            )
        print(f"[merge] Legacy export OK ({time.time() - t0:.1f}s)", flush=True)
    except Exception as e:
        print(f"[merge] Legacy export failed: {e}", flush=True)
        print("[merge] Retrying with dynamo=True ...", flush=True)
        try:
            with torch.no_grad():
                torch.onnx.export(
                    wrapper,
                    (dummy_mel,),
                    args.output,
                    input_names=["mel"],
                    output_names=OUTPUT_NAMES,
                    dynamic_axes=DYNAMIC_AXES,
                    opset_version=ONNX_OPSET,
                    do_constant_folding=True,
                    dynamo=True,
                )
            print(f"[merge] Dynamo export OK ({time.time() - t0:.1f}s)", flush=True)
        except Exception as e2:
            print(f"[merge] Both ONNX export paths FAILED: {e2}", file=sys.stderr)
            return 1

    if not os.path.exists(args.output):
        print(f"[merge] File missing after export: {args.output}", file=sys.stderr)
        return 1

    size_mb = os.path.getsize(args.output) / (1024 * 1024)
    print(f"[merge] ONNX written — {args.output} ({size_mb:.1f} MB)", flush=True)

    # 5. Post-export parity check (PyTorch vs ONNX)
    try:
        import onnxruntime as ort
    except ImportError:
        print("[merge] WARNING: onnxruntime not installed, skipping ONNX parity check", flush=True)
        return 0

    print("[merge] Running PyTorch ↔ ONNX parity check ...", flush=True)
    sess = ort.InferenceSession(args.output, providers=["CPUExecutionProvider"])
    onnx_inputs = {sess.get_inputs()[0].name: dummy_mel.cpu().numpy()}
    onnx_outputs = sess.run(None, onnx_inputs)
    onnx_1024, onnx_512 = onnx_outputs

    pt_1024_np = pt_1024.cpu().numpy()
    pt_512_np = pt_512.cpu().numpy()

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

    PARITY_RMSE_TOL = 5e-4
    PARITY_COS_TOL = 0.9999

    print(f"[merge] parity 1024 — rmse={drift_1024:.2e}  cos={cos_1024:.6f}", flush=True)
    print(f"[merge] parity 512  — rmse={drift_512:.2e}  cos={cos_512:.6f}", flush=True)

    parity_ok = True
    if drift_1024 > PARITY_RMSE_TOL or cos_1024 < PARITY_COS_TOL:
        print(f"[merge] FATAL: 1024-d ONNX/PyTorch drift exceeded tolerance", file=sys.stderr)
        parity_ok = False
    if drift_512 > PARITY_RMSE_TOL or cos_512 < PARITY_COS_TOL:
        print(f"[merge] FATAL: 512-d ONNX/PyTorch drift exceeded tolerance", file=sys.stderr)
        parity_ok = False

    if not parity_ok:
        return 1

    print("[merge] ✅ Parity check PASSED — ONNX matches PyTorch within tolerance", flush=True)
    print(f"[merge] Done. Merged ONNX at: {args.output}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
