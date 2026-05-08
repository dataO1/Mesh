"""Embed Music Flamingo captions with a sentence-transformer.

Default encoder: BAAI/bge-base-en-v1.5 (768d, ~440 MB, ~2k sent/sec on CPU,
top-tier on MTEB retrieval). Fallback: all-mpnet-base-v2 (768d, similar
speed).

Output:
    NPZ with `track_ids: int64[N]`, `caption_emb: float32[N, 768]`,
    `model_name: str`, `caption_lengths: int32[N]` (word count).

Usage:
    bash spike/track-grading/run_r7_step.sh embed_captions.py \
         --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \
         --out /home/data01/Music/mesh-track-grading/round7_6_caption_emb.npz
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--captions-root", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--model", default="BAAI/bge-base-en-v1.5",
                   help="sentence-transformer model id")
    p.add_argument("--batch-size", type=int, default=64)
    p.add_argument("--device", default="auto",
                   help="cuda | cpu | auto. CPU is plenty fast for ~15k captions.")
    return p.parse_args()


def main(args) -> int:
    files = sorted(args.captions_root.glob("*.json"))
    if not files:
        print(f"no caption files found under {args.captions_root}", file=sys.stderr)
        return 1
    print(f"[embed] reading {len(files)} captions from {args.captions_root}")

    track_ids = []
    captions = []
    word_counts = []
    for f in files:
        rec = json.loads(f.read_text())
        track_ids.append(int(rec["track_id"]))
        cap = rec.get("caption", "").strip()
        captions.append(cap)
        word_counts.append(len(cap.split()))

    print(f"[embed] caption stats: avg_words={np.mean(word_counts):.0f}  "
          f"min={min(word_counts)}  max={max(word_counts)}")

    from sentence_transformers import SentenceTransformer

    if args.device == "auto":
        import torch
        args.device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"[embed] loading {args.model} on {args.device} ...")
    model = SentenceTransformer(args.model, device=args.device)
    print(f"[embed] dim={model.get_sentence_embedding_dimension()}")

    emb = model.encode(
        captions,
        batch_size=args.batch_size,
        show_progress_bar=True,
        convert_to_numpy=True,
        normalize_embeddings=True,  # cosine-friendly
    ).astype(np.float32)
    print(f"[embed] embeddings shape: {emb.shape}")

    track_ids = np.array(track_ids, dtype=np.int64)
    word_counts = np.array(word_counts, dtype=np.int32)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(
        args.out,
        track_ids=track_ids,
        caption_emb=emb,
        caption_lengths=word_counts,
        model_name=args.model,
    )
    print(f"[embed] wrote {args.out} ({args.out.stat().st_size/1e6:.1f} MB)")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
