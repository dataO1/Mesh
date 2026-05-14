#!/usr/bin/env python3
"""Inspect MuQ-MuLan-large audio tower attention architecture using muq package."""
from __future__ import annotations

import sys
import torch
from muq import MuQMuLan


def main():
    print("Loading MuQ-MuLan-large via muq package ...")
    model = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large")
    model.eval()

    # Find the audio tower
    print(f"\nModel type: {type(model).__name__}")
    print(f"Top attrs: {[a for a in dir(model) if not a.startswith('_')][:30]}")

    # mulan_module is the key attribute
    mulan = model.mulan_module
    print(f"\nmulan_module type: {type(mulan).__name__}")

    # Inspect mulan children
    print("\n=== mulan_module children ===")
    for name, child in mulan.named_children():
        print(f"  {name}: {type(child).__name__}")

    # Look for audio tower
    if hasattr(mulan, 'audio'):
        print("\n=== mulan.audio ===")
        audio = mulan.audio
        print(f"  type: {type(audio).__name__}")
        for name, child in audio.named_children():
            print(f"  {name}: {type(child).__name__}")
        
        # Drill into audio.model
        if hasattr(audio, 'model'):
            print("\n=== mulan.audio.model ===")
            am = audio.model
            print(f"  type: {type(am).__name__}")
            for name, child in am.named_children():
                print(f"  {name}: {type(child).__name__}")
            
            # Drill into audio.model.model (the actual Conformer)
            if hasattr(am, 'model'):
                print("\n=== mulan.audio.model.model (Conformer?) ===")
                amm = am.model
                print(f"  type: {type(amm).__name__}")
                for name, child in amm.named_children():
                    print(f"  {name}: {type(child).__name__}")

    # Full linear layer inventory in audio tower
    print("\n=== ALL Linear layers in mulan.audio (fused QKV check) ===")
    if hasattr(mulan, 'audio'):
        for nm, mod in mulan.audio.named_modules():
            if isinstance(mod, torch.nn.Linear):
                fused = " ⚡ FUSED QKV" if mod.out_features == 3 * mod.in_features else ""
                print(f"  {nm}: Linear({mod.in_features} → {mod.out_features}){fused}")

    # Also check: what does extract_audio_latents do internally?
    print("\n=== extract_audio_latents source hint ===")
    import inspect
    try:
        src = inspect.getsource(model.extract_audio_latents)
        # Find key lines mentioning model or forward
        for line in src.split('\n'):
            if any(kw in line.lower() for kw in ['model(', 'forward', 'encoder', 'conformer', 'get_predict']):
                print(f"  {line.strip()}")
    except Exception:
        print("  (could not get source)")

    # Summary
    print("\n=== SUMMARY ===")
    print("Check the Linear layer list above for 'FUSED QKV' markers.")
    print("If present, the Conformer uses fused attention projections.")
    print("Viable PEFT targets: attention Linear layers (q/k/v/o_proj or equivalent).")

    return 0


if __name__ == "__main__":
    sys.exit(main())
