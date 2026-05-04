"""Refresh expired Deezer preview URLs in corpus_tracks.json.

Deezer's preview URLs (cdnt-preview.dzcdn.net/.../X.mp3?hdnea=exp=...)
are signed with an `exp=<unix_ts>` lifetime around 30-45 minutes. If the
download phase outlasts the URL TTL, later workers see HTTP 403.

This script:
  1. Reads corpus_tracks.json
  2. For every track whose dz_<id>.mp3 is NOT yet on disk, calls
     `/track/{id}` to get a fresh preview URL
  3. Writes the manifest back in place

After this, re-run download_previews.py — it skips existing MP3s and
fetches the rest with fresh URLs.

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/refresh_preview_urls.py
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import requests


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--manifest", type=Path,
                   default=Path("/tmp/track-grading/deezer/corpus_tracks.json"))
    p.add_argument("--audio-dir", type=Path,
                   default=Path("/tmp/track-grading/audio"))
    p.add_argument("--prefix", default="dz_")
    p.add_argument("--rate-rps", type=float, default=10.0,
                   help="Deezer API rate cap (anonymous limit ~10/s)")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    if not args.manifest.exists():
        sys.exit(f"missing {args.manifest}")
    manifest = json.loads(args.manifest.read_text())
    print(f"[refresh] manifest: {len(manifest)} tracks")

    need_refresh: list[dict] = []
    n_have = 0
    for row in manifest:
        tid = row.get("deezer_track_id")
        if tid is None: continue
        if (args.audio_dir / f"{args.prefix}{tid}.mp3").exists():
            n_have += 1
        else:
            need_refresh.append(row)
    print(f"[refresh] already downloaded: {n_have}")
    print(f"[refresh] need fresh URL:     {len(need_refresh)}")
    if not need_refresh:
        print("[refresh] nothing to do."); return 0

    sess = requests.Session()
    sess.headers["User-Agent"] = "mesh-research/1.0 (preview refresh)"
    min_gap = 1.0 / max(args.rate_rps, 0.1)

    n_ok = n_fail = 0
    last_call = 0.0
    start = time.time()

    for i, row in enumerate(need_refresh):
        # rate gate
        elapsed = time.time() - last_call
        if elapsed < min_gap:
            time.sleep(min_gap - elapsed)
        last_call = time.time()

        tid = row["deezer_track_id"]
        try:
            r = sess.get(f"https://api.deezer.com/track/{tid}", timeout=15).json()
        except Exception as e:
            n_fail += 1
            if i < 5: print(f"  fail {tid}: {e}", file=sys.stderr)
            continue
        new_preview = r.get("preview")
        if new_preview:
            row["preview_url"] = new_preview
            n_ok += 1
        else:
            n_fail += 1

        if (i + 1) % 200 == 0 or (i + 1) == len(need_refresh):
            rate = (i + 1) / max(time.time() - start, 0.01)
            eta = (len(need_refresh) - i - 1) / max(rate, 0.01)
            print(f"  [{i+1}/{len(need_refresh)}] ok={n_ok} fail={n_fail} "
                  f"({rate:.1f}/s, eta {eta/60:.1f} min)")

    # Atomic write back
    tmp = args.manifest.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(manifest, indent=2))
    tmp.rename(args.manifest)
    print(f"\n[refresh] manifest updated: {n_ok} URLs refreshed, {n_fail} failed")
    print(f"[refresh] now run download_previews.py to fetch the {n_ok} new URLs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
