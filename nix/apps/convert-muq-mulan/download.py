"""Download the MuQ-MuLan-large checkpoint from HuggingFace.

Caches into `~/.cache/mesh-spike/muq-mulan/`. Idempotent — re-runs only
download missing files. ~2.65 GB total.
"""
import os
import sys
from pathlib import Path

CACHE_ROOT = Path.home() / ".cache" / "mesh-spike" / "muq-mulan"
REPO_ID = "OpenMuQ/MuQ-MuLan-large"


def main() -> int:
    CACHE_ROOT.mkdir(parents=True, exist_ok=True)
    print(f"[download] target cache: {CACHE_ROOT}")
    print(f"[download] repo:         {REPO_ID}")

    try:
        from huggingface_hub import snapshot_download
    except ImportError as e:
        print(f"[download] ERROR: huggingface_hub not installed: {e}", file=sys.stderr)
        return 2

    try:
        local_dir = snapshot_download(
            repo_id=REPO_ID,
            cache_dir=str(CACHE_ROOT),
            # Skip optional artifacts; the muq lib loads from the cache via
            # standard HF resolution on first .from_pretrained() call anyway.
        )
    except Exception as e:
        print(f"[download] FAILED: {e}", file=sys.stderr)
        return 1

    print(f"[download] OK — snapshot at: {local_dir}")
    # Show file sizes so the user can sanity-check the ~2.65 GB target.
    total = 0
    for root, _, files in os.walk(local_dir):
        for f in files:
            fp = os.path.join(root, f)
            try:
                size = os.path.getsize(fp)
                total += size
            except OSError:
                pass
    print(f"[download] total snapshot size: {total / 1024**3:.2f} GiB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
