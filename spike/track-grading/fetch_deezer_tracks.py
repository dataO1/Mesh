"""Build a corpus of music tracks from any seed list via Deezer's public API.

Deezer's metadata + preview-MP3 endpoints are unauthenticated and free —
no dev account, no OAuth, no Premium. Per seed (genre exemplar, artist
recommendation, anything you can describe with artist+title) the script:

  1. searches `/search?q=artist:"X" track:"Y"` to get a Deezer track id
  2. fetches `/track/{id}/radio?limit=N` for N-1 similar tracks
  3. records seed + N-1 radio tracks (1 + 9 = 10 by default)
  4. caches per-seed JSON so re-runs resume cleanly

INPUT (`--input <path>`, auto-detected):

  everynoise format (default if file has top-level `genres` key):
    {"genres": [
      {"name": "neurofunk",
       "example_track": "e.g. Hyper \\"FCKD\\""}, ...]}

  flat-seed format (any list of dicts at top level):
    [{"category": "neurofunk", "artist": "Hyper", "title": "FCKD"}, ...]
    {"category", "artist", "title"} required keys; additional keys
    pass through. Single-field variant `{"seed_query": "Hyper - FCKD"}`
    also supported (split on " - ").

OUTPUT (`<out-dir>/corpus_tracks.json` aggregate, plus per-seed cache):

  [{"deezer_track_id": 648681682,
    "artist": "Hyper", "title": "FCKD (feat. Mark Arn)",
    "isrc": "CA5KR1917366", "duration_s": 207,
    "preview_url": "https://cdnt-preview.dzcdn.net/...mp3",
    "source_category": "neurofunk",
    "source_seed": "Hyper - FCKD",
    "match_kind": "seed" | "radio"}, ...]

Rate limits: Deezer allows ~50 req / 5 s anonymous. Default 10 req/s
keeps us comfortably under (configurable via `--rate-rps`).

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/fetch_deezer_tracks.py
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/fetch_deezer_tracks.py \\
    --input my_seeds.json --tracks-per-seed 10 --rate-rps 8
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path

import requests


DEFAULT_INPUT = Path("/tmp/track-grading/everynoise_dj_genres.json")
DEFAULT_OUT_DIR = Path("/tmp/track-grading/deezer")


def parse_args():
    p = argparse.ArgumentParser(formatter_class=argparse.RawDescriptionHelpFormatter,
                                description=__doc__)
    p.add_argument("--input", type=Path, default=DEFAULT_INPUT,
                   help="seed file (everynoise format or flat list)")
    p.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR,
                   help="cache + final corpus_tracks.json directory")
    p.add_argument("--tracks-per-seed", type=int, default=10,
                   help="seed + (N-1) radio recommendations per seed")
    p.add_argument("--rate-rps", type=float, default=10.0,
                   help="max API requests per second (anonymous Deezer "
                        "limit is ~10/s; 8-10 is safe)")
    p.add_argument("--limit-seeds", type=int, default=None,
                   help="cap seed count for testing")
    p.add_argument("--force", action="store_true",
                   help="ignore per-seed cache and refetch")
    return p.parse_args()


# ──────────────────────────────────────────────────────────────────────
# Input adapters
# ──────────────────────────────────────────────────────────────────────

EXAMPLE_RX = re.compile(r'^\s*e\.g\.\s+(.+?)\s+["“]([^"”]+)["”]\s*$')

def parse_everynoise_seed(row: dict) -> tuple[str, str, str] | None:
    """`{"name": ..., "example_track": "e.g. ARTIST \"TITLE\""}` → (cat,a,t)."""
    cat = row.get("name") or ""
    raw = row.get("example_track") or ""
    m = EXAMPLE_RX.match(raw)
    if m:
        return cat, m.group(1).strip(), m.group(2).strip()
    return None


def parse_flat_seed(row: dict) -> tuple[str, str, str] | None:
    """Accept several flat schemas. Returns (category, artist, title) or None."""
    cat = row.get("category") or row.get("genre") or row.get("name") or "uncategorized"
    if row.get("artist") and row.get("title"):
        return cat, row["artist"].strip(), row["title"].strip()
    seed = row.get("seed_query") or row.get("query")
    if seed and " - " in seed:
        a, _, t = seed.partition(" - ")
        return cat, a.strip(), t.strip()
    return None


def load_seeds(path: Path) -> list[tuple[str, str, str]]:
    """Returns [(category, artist, title), ...]; auto-detects format."""
    raw = json.loads(path.read_text())
    seeds: list[tuple[str, str, str]] = []
    if isinstance(raw, dict) and "genres" in raw:
        rows = raw["genres"]
        for r in rows:
            s = parse_everynoise_seed(r)
            if s: seeds.append(s)
    elif isinstance(raw, list):
        for r in raw:
            s = parse_flat_seed(r)
            if s: seeds.append(s)
    else:
        sys.exit(f"unrecognised input format in {path}")
    return seeds


# ──────────────────────────────────────────────────────────────────────
# Deezer API
# ──────────────────────────────────────────────────────────────────────

class DeezerClient:
    BASE = "https://api.deezer.com"
    UA = "mesh-research/1.0 (round-7 corpus build)"

    def __init__(self, rate_rps: float):
        self._min_gap = 1.0 / max(rate_rps, 0.1)
        self._last = 0.0
        self._sess = requests.Session()
        self._sess.headers["User-Agent"] = self.UA

    def _gate(self):
        elapsed = time.time() - self._last
        if elapsed < self._min_gap:
            time.sleep(self._min_gap - elapsed)
        self._last = time.time()

    def _get(self, path: str, **params) -> dict:
        self._gate()
        for attempt in range(4):
            try:
                r = self._sess.get(f"{self.BASE}{path}", params=params, timeout=20)
            except requests.RequestException as e:
                if attempt == 3: raise
                time.sleep(2 ** attempt); continue
            if r.status_code == 429:
                # Quota Limit Exceeded — back off and retry.
                wait = float(r.headers.get("Retry-After", str(2 ** attempt)))
                time.sleep(wait + 0.5); continue
            try:
                data = r.json()
            except ValueError:
                if attempt == 3: raise RuntimeError(f"bad JSON: {r.text[:200]}")
                time.sleep(2 ** attempt); continue
            if isinstance(data, dict) and "error" in data:
                code = data["error"].get("code")
                if code == 4:  # rate limit error in payload
                    time.sleep(2 ** attempt); continue
                # other errors: return as-is for caller to handle
            return data
        raise RuntimeError(f"deezer GET {path} failed after 4 attempts")

    def search_track(self, artist: str, title: str) -> dict | None:
        """Strict artist+title search; falls back to title-only on empty."""
        for q in (f'artist:"{artist}" track:"{title}"', title):
            data = self._get("/search", q=q, limit=5)
            items = data.get("data") or []
            if items:
                return items[0]
        return None

    def artist_radio(self, artist_id: int, limit: int) -> list[dict]:
        """Deezer returns ~25 similar-to-artist tracks. The `/track/{id}/radio`
        endpoint doesn't exist — radios are keyed by artist or playlist."""
        data = self._get(f"/artist/{artist_id}/radio", limit=limit)
        return data.get("data") or []

    def artist_top(self, artist_id: int, limit: int) -> list[dict]:
        """Top tracks for an artist — used as fallback when radio is empty
        (rare niche artists with no similar-artist graph)."""
        data = self._get(f"/artist/{artist_id}/top", limit=limit)
        return data.get("data") or []


# ──────────────────────────────────────────────────────────────────────
# Per-track normaliser
# ──────────────────────────────────────────────────────────────────────

def deezer_to_record(t: dict, category: str, seed_query: str,
                     match_kind: str) -> dict | None:
    """Map a Deezer track dict to the corpus record schema. Skip tracks
    with no preview URL — they're useless for embedding extraction."""
    if not t.get("preview"):
        return None
    artist = t.get("artist") or {}
    return {
        "deezer_track_id": t.get("id"),
        "artist": (artist.get("name") if isinstance(artist, dict) else str(artist)),
        "title": t.get("title", ""),
        "isrc": t.get("isrc"),
        "duration_s": t.get("duration"),
        "preview_url": t.get("preview"),
        "source_category": category,
        "source_seed": seed_query,
        "match_kind": match_kind,
    }


# ──────────────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────────────

def cache_path(out_dir: Path, category: str, artist: str, title: str) -> Path:
    safe = re.sub(r"[^a-zA-Z0-9_-]+", "_", f"{category}__{artist}__{title}")[:120]
    p = out_dir / "per_seed" / f"{safe}.json"
    p.parent.mkdir(parents=True, exist_ok=True)
    return p


def main() -> int:
    args = parse_args()
    if not args.input.exists():
        sys.exit(f"missing {args.input}")

    seeds = load_seeds(args.input)
    if args.limit_seeds:
        seeds = seeds[: args.limit_seeds]
    print(f"[fetch-deezer] {len(seeds)} seeds parsed from {args.input.name}")
    print(f"[fetch-deezer] target: {args.tracks_per_seed} tracks/seed → "
          f"~{len(seeds) * args.tracks_per_seed} total")

    client = DeezerClient(rate_rps=args.rate_rps)
    args.out_dir.mkdir(parents=True, exist_ok=True)

    n_done = n_skipped_cache = n_no_match = 0
    n_records = 0
    start = time.time()

    for i, (cat, artist, title) in enumerate(seeds):
        seed_query = f"{artist} - {title}"
        cache = cache_path(args.out_dir, cat, artist, title)
        if cache.exists() and not args.force:
            n_skipped_cache += 1
            continue

        try:
            seed = client.search_track(artist, title)
        except Exception as e:
            print(f"  [{i+1}] search FAILED for {seed_query!r}: {e}",
                  file=sys.stderr)
            cache.write_text(json.dumps({"error": str(e), "seed": seed_query,
                                          "tracks": []}))
            n_done += 1; continue

        if not seed:
            cache.write_text(json.dumps({"no_match": True, "seed": seed_query,
                                         "tracks": []}))
            n_no_match += 1; n_done += 1; continue

        seed_id = seed.get("id")
        records: list[dict] = []
        seed_rec = deezer_to_record(seed, cat, seed_query, "seed")
        if seed_rec:
            records.append(seed_rec)

        # Pull similar tracks via the seed artist's radio endpoint, then
        # fall back to the artist's top tracks if radio is sparse (very
        # niche artists sometimes have no similar-artist graph).
        seed_artist_id = (seed.get("artist") or {}).get("id")
        need = args.tracks_per_seed - len(records)
        seen_ids = {seed_id}
        if need > 0 and seed_artist_id:
            try:
                radio = client.artist_radio(int(seed_artist_id), limit=need + 10)
                for r in radio:
                    if len(records) >= args.tracks_per_seed: break
                    rid = r.get("id")
                    if rid in seen_ids: continue
                    rec = deezer_to_record(r, cat, seed_query, "radio")
                    if rec:
                        records.append(rec); seen_ids.add(rid)
            except Exception as e:
                print(f"  [{i+1}] radio FAILED for {seed_query!r}: {e}",
                      file=sys.stderr)
            need = args.tracks_per_seed - len(records)
            if need > 0:
                try:
                    top = client.artist_top(int(seed_artist_id), limit=need + 5)
                    for r in top:
                        if len(records) >= args.tracks_per_seed: break
                        rid = r.get("id")
                        if rid in seen_ids: continue
                        rec = deezer_to_record(r, cat, seed_query, "artist_top")
                        if rec:
                            records.append(rec); seen_ids.add(rid)
                except Exception as e:
                    print(f"  [{i+1}] artist_top FAILED for {seed_query!r}: {e}",
                          file=sys.stderr)

        cache.write_text(json.dumps({
            "seed": seed_query, "category": cat,
            "n_returned": len(records), "tracks": records,
        }, indent=2))
        n_records += len(records)
        n_done += 1

        if n_done % 25 == 0 or (i + 1) == len(seeds):
            elapsed = time.time() - start
            rate = n_done / max(elapsed, 0.01)
            eta_s = (len(seeds) - n_skipped_cache - n_done) / max(rate, 0.01)
            print(f"  [{n_done + n_skipped_cache}/{len(seeds)}] "
                  f"records+{n_records}  "
                  f"({rate:.1f} seeds/s  eta {eta_s/60:.1f} min  "
                  f"no_match={n_no_match}  cached={n_skipped_cache})")

    # Aggregate
    print()
    print(f"[fetch-deezer] aggregating per-seed caches...")
    seen_ids: set[int] = set()
    all_records: list[dict] = []
    for f in sorted((args.out_dir / "per_seed").glob("*.json")):
        try:
            blob = json.loads(f.read_text())
        except Exception:
            continue
        for rec in blob.get("tracks", []):
            tid = rec.get("deezer_track_id")
            if tid is None or tid in seen_ids:
                continue
            seen_ids.add(tid)
            all_records.append(rec)
    out = args.out_dir / "corpus_tracks.json"
    out.write_text(json.dumps(all_records, indent=2))

    print(f"[fetch-deezer] seeds processed:        {n_done}")
    print(f"[fetch-deezer] seeds cached/skipped:   {n_skipped_cache}")
    print(f"[fetch-deezer] seeds with no match:    {n_no_match}")
    print(f"[fetch-deezer] unique tracks gathered: {len(all_records)}")
    print(f"[fetch-deezer] aggregate file:         {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
