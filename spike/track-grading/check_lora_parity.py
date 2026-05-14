#!/usr/bin/env python3
"""
E4.2: Pre-train parity check — Apply identity LoRA to MuQ-MuLan and verify
that merge_and_unload() reproduces bit-identical weights in the conformer's
attention linear layers (linear_q, linear_k, linear_v, linear_out).

We load MuQ-MuLan-large, wrap it with PEFT LoRA (r=16, alpha=32) targeting
only the conformer attention projection layers, run a dummy forward pass to
activate the LoRA adapters, merge-and-unload, then compare every attention
weight against the original base state dict.
"""

import copy
import sys
from pathlib import Path

import torch
import torch.nn as nn

from muq import MuQMuLan
from peft import LoraConfig, get_peft_model


def main():
    device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"Device: {device}", flush=True)

    # ------------------------------------------------------------------
    # 1. Load the base model
    # ------------------------------------------------------------------
    print("Loading MuQ-MuLan-large ...", flush=True)
    model: MuQMuLan = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    model = model.to(device)
    model.eval()
    print("Model loaded.", flush=True)

    # Snapshot the *original* parameters BEFORE any LoRA wrapping.
    # We'll compare against these after merge_and_unload.
    target_keys = {"linear_q", "linear_k", "linear_v", "linear_out"}
    orig_state = {
        name: param.detach().cpu().clone()
        for name, param in model.named_parameters()
        # The parameter name looks like:
        #   mulan.audio.model.model.conformer.layers.N.self_attn.linear_q.weight
        # We match if the component *before* the last dot is one of our keys.
        if any(name.rpartition(".")[0].endswith(k) for k in target_keys)
    }
    print(f"Captured {len(orig_state)} original attention projection "
          f"weight tensors.", flush=True)
    if not orig_state:
        # Fallback: dump all parameter names to debug
        print("  WARNING: no attention weights captured — dumping all param names:",
              flush=True)
        for n, _ in model.named_parameters():
            print(f"    {n}", flush=True)

    # Also snapshot the full state dict for sanity
    orig_full = copy.deepcopy(model.state_dict())

    # ------------------------------------------------------------------
    # 2. Apply PEFT LoRA with identity initialisation
    # ------------------------------------------------------------------
    lora_config = LoraConfig(
        r=16,
        lora_alpha=32,
        target_modules=["linear_q", "linear_k", "linear_v", "linear_out"],
        init_lora_weights=True,       # B = 0, A = random → identity product
        lora_dropout=0.0,
        bias="none",
        task_type="FEATURE_EXTRACTION",
    )
    model = get_peft_model(model, lora_config)
    model.eval()
    print("LoRA adapters injected.", flush=True)

    # List the injected LoRA parameters
    lora_params = [n for n, _ in model.named_parameters() if "lora" in n]
    print(f"  LoRA parameters ({len(lora_params)}):")
    for n in lora_params[:8]:
        print(f"    {n}")
    if len(lora_params) > 8:
        print(f"    ... and {len(lora_params) - 8} more")

    # Verify that B matrices are zero (identity init)
    all_b_zero = True
    for name, param in model.named_parameters():
        if "lora_B" in name:
            if param.abs().max().item() > 1e-12:
                print(f"  WARNING: {name} is not zero (max={param.abs().max().item()})")
                all_b_zero = False
    if all_b_zero:
        print("  All lora_B matrices are zero (identity init confirmed).", flush=True)

    # ------------------------------------------------------------------
    # 3. Dummy forward pass to "activate" the LoRA layers
    # ------------------------------------------------------------------
    print("Running dummy forward pass ...", flush=True)
    with torch.no_grad():
        # The MuQ-MuLan audio tower expects raw waveforms at 24 kHz.
        # The audio transformer converts them to spectrograms internally.
        # Generate a 3-second dummy waveform (batch_size=1).
        batch_size = 1
        # Access the underlying base model's sr attribute
        base_model = model.base_model if hasattr(model, "base_model") else model
        sr = getattr(base_model, "sr", 24000)
        duration_secs = 3
        num_samples = sr * duration_secs
        dummy_wav = torch.randn(batch_size, num_samples, device=device)

        # Forward through the *base* model (bypass PEFT wrapper which doesn't
        # know about MuQ-MuLan's wavs/texts interface).
        _ = base_model(wavs=dummy_wav)

    print("Forward pass completed.", flush=True)

    # ------------------------------------------------------------------
    # 4. Merge LoRA and unload
    # ------------------------------------------------------------------
    print("Merging LoRA and unloading ...", flush=True)
    model = model.merge_and_unload()
    model.eval()
    print("Merge & unload done.", flush=True)

    # ------------------------------------------------------------------
    # 5. Compare merged weights vs original
    # ------------------------------------------------------------------
    # Collect the merged attention projection weights
    merged_state = {
        name: param.detach().cpu().clone()
        for name, param in model.named_parameters()
        if any(name.rpartition(".")[0].endswith(k) for k in target_keys)
    }
    print(f"Merged attention projections: {len(merged_state)}", flush=True)

    if set(orig_state.keys()) != set(merged_state.keys()):
        print("\n⚠  KEY MISMATCH between original and merged state dicts!")
        print("  Missing in merged:", set(orig_state.keys()) - set(merged_state.keys()))
        print("  Extra in merged:", set(merged_state.keys()) - set(orig_state.keys()))
        sys.exit(1)

    # Compare each weight tensor
    max_abs_diff = 0.0
    max_rel_diff = 0.0
    mismatches = []
    for key in sorted(orig_state.keys()):
        w_orig = orig_state[key]
        w_merged = merged_state[key]
        abs_diff = (w_orig - w_merged).abs().max().item()
        denom = w_orig.abs().max().item()
        rel_diff = abs_diff / (denom + 1e-12) if denom > 1e-12 else 0.0
        max_abs_diff = max(max_abs_diff, abs_diff)
        max_rel_diff = max(max_rel_diff, rel_diff)
        if abs_diff > 1e-7:
            mismatches.append((key, abs_diff, rel_diff))

    # ------------------------------------------------------------------
    # 6. Report
    # ------------------------------------------------------------------
    print("\n" + "=" * 70)
    print("PARITY CHECK RESULTS")
    print("=" * 70)
    print(f"  Max absolute difference : {max_abs_diff:.3e}")
    print(f"  Max relative difference : {max_rel_diff:.3e}")

    if mismatches:
        print(f"\n  ⚠  {len(mismatches)} weight(s) exceed 1e-7 tolerance:")
        for key, ad, rd in mismatches:
            print(f"      {key:70s}  abs={ad:.3e}  rel={rd:.3e}")
    else:
        print(f"\n  ✓  All {len(orig_state)} attention projection weights are "
              f"identical within float32 precision (≤ 1e-7).")

    # ------------------------------------------------------------------
    # 7. Check for residual LoraLayer containers
    # ------------------------------------------------------------------
    print("\n" + "-" * 70)
    print("RESIDUAL LoraLayer CHECK")
    print("-" * 70)
    residual_lora = []
    for name, module in model.named_modules():
        # PEFT injects LoraLayer subclasses; after merge_and_unload these
        # should be gone and replaced by the original Linear layers.
        mod_cls_name = type(module).__name__
        if "Lora" in mod_cls_name or "lora" in mod_cls_name.lower():
            residual_lora.append((name, mod_cls_name))
    if residual_lora:
        print(f"  ⚠  Found {len(residual_lora)} residual LoRA module(s):")
        for name, cls in residual_lora:
            print(f"      {name:70s} -> {cls}")
    else:
        print("  ✓  No residual LoraLayer / LoRA containers found.")

    # Also check that the original Linear layers are back in place
    # (i.e., no LoRALinear or similar wrappers)
    for name, module in model.named_modules():
        if isinstance(module, nn.Linear):
            for suffix in target_keys:
                if name.endswith(suffix):
                    print(f"  ✓  {name} is a plain nn.Linear (not wrapped).")
                    break

    print("\nDone.")
    return 0 if not mismatches else 1


if __name__ == "__main__":
    sys.exit(main())
