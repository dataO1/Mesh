"""Dump 512-dim MuQ-MuLan embeddings for the round-5 BT prior tracks.

Reads `ml_embeddings` relation from the mesh CozoDB (SQLite-backed),
filters to the 909 tracks present in `llm-pair-priors-r5.txt`, writes
to /tmp/track-grading/embeddings.npz with two arrays:
  - track_ids: int64 [N]
  - embeddings: float32 [N, 512]

Usage:
  LD_LIBRARY_PATH=/nix/store/c2qsgf2832zi4n29gfkqgkjpvmbmxam6-zlib-1.3.1/lib \\
    ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/dump_embeddings.py
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from pycozo.client import Client


DB_PATH = "/home/data01/Music/mesh-collection/mesh.db"
PRIORS_PATH = Path("documents/axis-eval-results/llm-pair-priors-r5.txt")
OUT_PATH = Path("/tmp/track-grading/embeddings.npz")


def main() -> int:
    if not Path(DB_PATH).exists():
        sys.exit(f"missing {DB_PATH}")
    if not PRIORS_PATH.exists():
        sys.exit(f"missing {PRIORS_PATH}")

    # Track IDs we care about (those with BT priors).
    wanted: set[int] = set()
    with PRIORS_PATH.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"): continue
            parts = line.split("|", 2)
            if len(parts) == 3:
                wanted.add(int(parts[0]))
    print(f"[dump] {len(wanted)} target track IDs")

    db = Client("sqlite", DB_PATH, {"dataframe": False})
    r = db.run("?[track_id, vec] := *ml_embeddings{track_id, vec}")
    rows = r["rows"]
    print(f"[dump] {len(rows)} embeddings in DB")

    track_ids: list[int] = []
    embs: list[list[float]] = []
    for tid, vec in rows:
        if tid in wanted:
            track_ids.append(int(tid))
            embs.append(vec)

    arr_ids = np.array(track_ids, dtype=np.int64)
    arr_emb = np.array(embs, dtype=np.float32)
    print(f"[dump] kept {len(track_ids)} rows, embedding dim={arr_emb.shape[1]}")

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    np.savez(OUT_PATH, track_ids=arr_ids, embeddings=arr_emb)
    print(f"[dump] wrote {OUT_PATH}  ({arr_emb.nbytes / 1024 / 1024:.1f} MB)")

    missing = wanted - set(track_ids)
    if missing:
        print(f"[dump] WARNING: {len(missing)} tracks have BT priors but no "
              f"embedding in DB", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
