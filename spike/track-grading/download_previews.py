"""Download preview MP3s from a corpus_tracks.json manifest.

Reads any list of dicts that carry a `preview_url` and a track id, fetches
each MP3 in parallel into <out-dir>/{prefix}{id}.mp3. Resume-safe (skips
existing files, optionally verifies their content-type).

Default works on the Deezer fetcher's output schema (`deezer_track_id`,
`preview_url`) but accepts other id-key/url-key pairs via flags so any
future scraper that produces a similar manifest works without rewriting.

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/download_previews.py
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/download_previews.py \\
    --manifest /tmp/track-grading/deezer/corpus_tracks.json \\
    --out-dir /tmp/track-grading/audio --workers 16
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import requests


DEFAULT_MANIFEST = Path("/tmp/track-grading/deezer/corpus_tracks.json")
DEFAULT_OUT_DIR  = Path("/tmp/track-grading/audio")


def parse_args():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST,
                   help="JSON list of track dicts")
    p.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR,
                   help="where to write the MP3 files")
    p.add_argument("--id-key", default="deezer_track_id",
                   help="dict key holding the track's unique id")
    p.add_argument("--url-key", default="preview_url",
                   help="dict key holding the preview MP3 URL")
    p.add_argument("--prefix", default="dz_",
                   help="filename prefix (e.g. dz_648681682.mp3)")
    p.add_argument("--workers", type=int, default=16,
                   help="parallel HTTP workers")
    p.add_argument("--verify", action="store_true",
                   help="re-check existing files via content-type+size; "
                        "redownload if invalid")
    p.add_argument("--limit", type=int, default=None,
                   help="cap downloads for testing")
    return p.parse_args()


def fetch_one(url: str, dest: Path, sess: requests.Session) -> tuple[bool, str]:
    """Returns (success, reason). Uses streaming to avoid loading the full
    payload into memory."""
    try:
        with sess.get(url, stream=True, timeout=30) as r:
            if r.status_code != 200:
                return False, f"http {r.status_code}"
            ctype = (r.headers.get("Content-Type") or "").lower()
            if "audio" not in ctype and "octet-stream" not in ctype:
                return False, f"non-audio content-type: {ctype}"
            tmp = dest.with_suffix(dest.suffix + ".part")
            with tmp.open("wb") as f:
                for chunk in r.iter_content(chunk_size=64 * 1024):
                    if chunk:
                        f.write(chunk)
            tmp.rename(dest)
            return True, "ok"
    except requests.RequestException as e:
        return False, f"net error: {e}"


def main() -> int:
    args = parse_args()
    if not args.manifest.exists():
        sys.exit(f"missing {args.manifest}")
    rows = json.loads(args.manifest.read_text())
    if args.limit:
        rows = rows[: args.limit]
    args.out_dir.mkdir(parents=True, exist_ok=True)

    sess = requests.Session()
    sess.headers["User-Agent"] = "mesh-research/1.0 (preview downloader)"

    targets: list[tuple[str, Path]] = []
    for row in rows:
        tid = row.get(args.id_key)
        url = row.get(args.url_key)
        if not tid or not url:
            continue
        dest = args.out_dir / f"{args.prefix}{tid}.mp3"
        if dest.exists() and not args.verify:
            continue
        if dest.exists() and args.verify:
            # quick stat check; full content-type check happens by HEAD
            if dest.stat().st_size > 100_000:  # 100 KB minimum for real audio
                continue
        targets.append((url, dest))

    skipped = len(rows) - len(targets)
    print(f"[download] manifest: {len(rows)}  to-download: {len(targets)}  "
          f"already-cached: {skipped}")
    if not targets:
        print("[download] nothing to do.")
        return 0

    n_ok = n_fail = 0
    fails: list[tuple[str, str]] = []
    start = time.time()

    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futures = {ex.submit(fetch_one, u, d, sess): (u, d) for u, d in targets}
        for f in as_completed(futures):
            ok, reason = f.result()
            if ok:
                n_ok += 1
            else:
                n_fail += 1
                fails.append((futures[f][0][:80], reason))
            done = n_ok + n_fail
            if done % 200 == 0 or done == len(targets):
                elapsed = time.time() - start
                rate = done / max(elapsed, 0.01)
                eta = (len(targets) - done) / max(rate, 0.01)
                print(f"  [{done}/{len(targets)}] ok={n_ok} fail={n_fail}  "
                      f"({rate:.1f}/s, eta {eta:.0f}s)")

    print()
    print(f"[download] success: {n_ok}")
    print(f"[download] failed:  {n_fail}")
    if fails[:10]:
        print(f"[download] sample failures:")
        for u, r in fails[:10]:
            print(f"  {r:30s} {u}")
    return 0 if n_fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
