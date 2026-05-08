"""Fetch tracks from Spotify playlists referenced in the everynoise scrape.

For each DJ-relevant genre in `everynoise_dj_genres.json`, calls Spotify's
`/playlists/{id}/tracks` and saves up to N tracks per genre that have a
30s preview URL we can train on. Output: a flat JSON list of track dicts
with (artist, title, isrc, duration_ms, preview_url, source_genre,
spotify_track_id, source_playlist_id) — ready for the downloader script.

Credentials resolution order (highest priority first):
  1. --client-id / --client-secret CLI args
  2. SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET environment variables
  3. ~/.config/mesh-research/spotify.env (KEY=VALUE lines)
  4. <repo>/.spotify.env (KEY=VALUE lines, last-resort)

The credentials file is per-user and never committed; the script never
logs the secret. Get a free token at https://developer.spotify.com/dashboard
(~5 min, no review process).

Resume-safe: per-genre results are cached individually under
`<out-dir>/per_genre/<playlist_id>.json`. Re-running picks up where the
previous run left off (use `--force` to refetch everything).

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/fetch_spotify_tracks.py
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/fetch_spotify_tracks.py --tracks-per-genre 10
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/fetch_spotify_tracks.py --client-id X --client-secret Y
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import spotipy
from spotipy.oauth2 import SpotifyClientCredentials


DEFAULT_CONFIG_PATHS = [
    Path.home() / ".config" / "mesh-research" / "spotify.env",
    Path(".spotify.env"),
]


def parse_env_file(p: Path) -> dict[str, str]:
    out = {}
    if not p.exists():
        return out
    for line in p.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        out[k.strip()] = v.strip().strip('"').strip("'")
    return out


def resolve_credentials(args) -> tuple[str, str]:
    if args.client_id and args.client_secret:
        return args.client_id, args.client_secret
    cid = os.environ.get("SPOTIFY_CLIENT_ID")
    sec = os.environ.get("SPOTIFY_CLIENT_SECRET")
    if cid and sec:
        return cid, sec
    for p in DEFAULT_CONFIG_PATHS:
        env = parse_env_file(p)
        if env.get("SPOTIFY_CLIENT_ID") and env.get("SPOTIFY_CLIENT_SECRET"):
            return env["SPOTIFY_CLIENT_ID"], env["SPOTIFY_CLIENT_SECRET"]
    sys.exit(
        "ERROR: no Spotify credentials found.\n"
        "Set them via one of:\n"
        "  --client-id / --client-secret CLI flags\n"
        "  SPOTIFY_CLIENT_ID + SPOTIFY_CLIENT_SECRET env vars\n"
        f"  {DEFAULT_CONFIG_PATHS[0]} (preferred)\n"
        f"  {DEFAULT_CONFIG_PATHS[1]} (repo-local fallback)\n"
        "\nGet credentials free at https://developer.spotify.com/dashboard "
        "(~5 min, no review)."
    )


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--genres", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/everynoise_dj_genres.json"),
                   help="output of categorize_genres.py")
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading/spotify"),
                   help="cache + final output directory")
    p.add_argument("--tracks-per-genre", type=int, default=10,
                   help="how many preview-having tracks to keep per genre")
    p.add_argument("--max-fetch-per-playlist", type=int, default=50,
                   help="how many tracks to scan per playlist looking for "
                        "the N with previews (some tracks lack previews)")
    p.add_argument("--client-id", type=str, default=None)
    p.add_argument("--client-secret", type=str, default=None)
    p.add_argument("--force", action="store_true",
                   help="ignore per-genre cache and refetch everything")
    p.add_argument("--limit-genres", type=int, default=None,
                   help="cap the genre count for testing")
    return p.parse_args()


def fetch_playlist_tracks(sp: spotipy.Spotify, playlist_id: str,
                          n_keep: int, n_scan: int) -> list[dict]:
    """Pull up to n_scan tracks; return the first n_keep that have a
    preview_url. Some Spotify playlists are user-curated and we filter
    out the (small) fraction without a preview."""
    out = []
    backoff = 1.0
    while True:
        try:
            r = sp.playlist_tracks(
                playlist_id,
                fields="items(track(id,name,artists,album.name,external_ids,"
                       "duration_ms,preview_url))",
                limit=n_scan,
            )
            break
        except spotipy.SpotifyException as e:
            if e.http_status == 429:
                ra = int(e.headers.get("Retry-After", str(int(backoff))))
                print(f"  rate-limited, sleeping {ra}s...", file=sys.stderr)
                time.sleep(ra)
                backoff *= 2
                if backoff > 60: raise
                continue
            if e.http_status in (404, 400):
                # Playlist gone or invalid id; treat as empty.
                return []
            raise
    for item in r.get("items", []):
        if len(out) >= n_keep:
            break
        t = item.get("track")
        if not t or not t.get("preview_url"):
            continue
        artists = t.get("artists") or []
        out.append({
            "spotify_track_id": t.get("id"),
            "artist": ", ".join(a.get("name", "") for a in artists),
            "title": t.get("name", ""),
            "album": (t.get("album") or {}).get("name", ""),
            "isrc": (t.get("external_ids") or {}).get("isrc"),
            "duration_ms": t.get("duration_ms"),
            "preview_url": t.get("preview_url"),
        })
    return out


def main() -> int:
    args = parse_args()
    if not args.genres.exists():
        sys.exit(f"missing {args.genres} — run categorize_genres.py first")

    cid, sec = resolve_credentials(args)
    print(f"[fetch] using Spotify client_id={cid[:8]}... (secret {len(sec)} chars)")

    auth = SpotifyClientCredentials(client_id=cid, client_secret=sec)
    sp = spotipy.Spotify(auth_manager=auth, requests_timeout=30, retries=3)

    data = json.loads(args.genres.read_text())
    genres = data["genres"]
    if args.limit_genres:
        genres = genres[: args.limit_genres]
    print(f"[fetch] {len(genres)} genres queued")

    cache_dir = args.out_dir / "per_genre"
    cache_dir.mkdir(parents=True, exist_ok=True)

    n_done = n_skipped = n_no_tracks = 0
    n_total_tracks = 0
    start = time.time()

    for i, g in enumerate(genres):
        pid = g.get("playlist_id")
        if not pid:
            n_no_tracks += 1
            continue
        cache = cache_dir / f"{pid}.json"
        if cache.exists() and not args.force:
            n_skipped += 1
            continue

        try:
            tracks = fetch_playlist_tracks(
                sp, pid,
                n_keep=args.tracks_per_genre,
                n_scan=args.max_fetch_per_playlist,
            )
        except Exception as e:
            print(f"  [skip] {g['name']!r} ({pid}): {e}", file=sys.stderr)
            tracks = []

        for t in tracks:
            t["source_genre"] = g["name"]
            t["source_playlist_id"] = pid

        cache.write_text(json.dumps(tracks, indent=2))
        n_done += 1
        n_total_tracks += len(tracks)

        if n_done % 25 == 0 or n_done == len(genres):
            rate = n_done / max(time.time() - start, 0.01)
            eta = (len(genres) - n_skipped - n_done) / max(rate, 0.01)
            print(f"  [{n_done + n_skipped}/{len(genres)}] "
                  f"+{n_total_tracks} tracks total "
                  f"({rate:.1f} genres/s, eta {eta/60:.1f} min, "
                  f"last: {g['name']!r})")

    # Aggregate everything into one corpus_tracks.json
    final_path = args.out_dir / "corpus_tracks.json"
    all_tracks: list[dict] = []
    for f in sorted(cache_dir.glob("*.json")):
        try:
            all_tracks.extend(json.loads(f.read_text()))
        except Exception:
            pass
    final_path.write_text(json.dumps(all_tracks, indent=2))

    print()
    print(f"[fetch] genres processed:    {n_done}")
    print(f"[fetch] genres cached/skipped: {n_skipped}")
    print(f"[fetch] genres without playlist id: {n_no_tracks}")
    print(f"[fetch] total tracks gathered: {n_total_tracks}")
    print(f"[fetch] aggregate file: {final_path} ({len(all_tracks)} tracks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
