"""Download the MuQ-MuLan-large checkpoint from HuggingFace.

Uses whatever cache dir `HF_HOME` / `HF_HUB_CACHE` point at — the wrapper
sets that to `~/.cache/mesh-spike/hf` so both this script and later
`MuQMuLan.from_pretrained(...)` calls in export.py / validate.py share
the same hub cache. Idempotent — re-runs only fetch missing files.
~2.65 GB total.
"""
import os
import sys

REPO_ID = "OpenMuQ/MuQ-MuLan-large"


def main() -> int:
    cache_root = os.environ.get("HF_HUB_CACHE") or os.environ.get("HF_HOME") or "(default ~/.cache/huggingface/hub/)"
    print(f"[download] cache (HF_HUB_CACHE): {cache_root}")
    print(f"[download] repo:                 {REPO_ID}")

    try:
        from huggingface_hub import snapshot_download
    except ImportError as e:
        print(f"[download] ERROR: huggingface_hub not installed: {e}", file=sys.stderr)
        return 2

    try:
        # No cache_dir kwarg → respects HF_HOME / HF_HUB_CACHE from the env.
        local_dir = snapshot_download(repo_id=REPO_ID)
    except Exception as e:
        print(f"[download] FAILED: {e}", file=sys.stderr)
        return 1

    print(f"[download] OK — snapshot at: {local_dir}")
    total = 0
    for root, _, files in os.walk(local_dir):
        for f in files:
            fp = os.path.join(root, f)
            try:
                total += os.path.getsize(fp)
            except OSError:
                pass
    print(f"[download] total snapshot size: {total / 1024**3:.2f} GiB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
