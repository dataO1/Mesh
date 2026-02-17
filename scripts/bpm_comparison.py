#!/usr/bin/env python3
"""
BPM Comparison Tool — Compare detected BPMs against Beatport ground truth.

Reads the JSON export from `export-analysis` and looks up each track on Beatport
to find the real BPM. Produces a comparison report with accuracy statistics for
both the Beat This! (Advanced) and Essentia (Simple) backends.

Usage:
    nix run .#bpm-report                   # One-liner: export + scrape + report
    nix run .#bpm-report -- --limit 20     # Scrape only 20 new tracks

    # Or manually:
    cargo run -p mesh-core --bin export-analysis -- analysis-export.json
    python3 scripts/bpm_comparison.py analysis-export.json --scrape
    python3 scripts/bpm_comparison.py analysis-export.json -g ground-truth.json

Ground truth JSON format:
    {
        "Artist - Track Name": 174,
        "Another Artist - Track": 175
    }
"""

import json
import sys
import argparse
import re
import os
from concurrent.futures import ThreadPoolExecutor, as_completed
from urllib.request import urlopen, Request
from urllib.parse import quote_plus
from urllib.error import URLError, HTTPError


# ---------------------------------------------------------------------------
# Name cleaning
# ---------------------------------------------------------------------------

def strip_numeric_prefix(name: str) -> str:
    """Strip leading numeric prefix like '100_' from track/artist names."""
    return re.sub(r'^\d+_', '', name)


def parse_track_fields(raw_name: str, raw_artist: str | None) -> tuple[str, str | None]:
    """Parse the DB name/artist fields into clean display values.

    DB stores names as '{prefix}_{Artist} - {Title}' and artist as '{prefix}_{Artist}'.
    Returns (title, artist) with prefixes stripped and artist extracted.
    """
    clean_artist = strip_numeric_prefix(raw_artist).strip() if raw_artist else None

    clean_name = strip_numeric_prefix(raw_name).strip()
    # Remove trailing duplicate markers like " (2)"
    clean_name = re.sub(r'\s*\(\d+\)\s*$', '', clean_name).strip()

    # The name field often includes "Artist - Title". Strip the artist prefix.
    title = clean_name
    if clean_artist:
        prefix = f"{clean_artist} - "
        if clean_name.startswith(prefix):
            title = clean_name[len(prefix):]
        else:
            dash_pos = clean_name.find(' - ')
            if dash_pos > 0:
                title = clean_name[dash_pos + 3:]

    return title.strip(), clean_artist


# ---------------------------------------------------------------------------
# Beatport scraping via Next.js SSR JSON
# ---------------------------------------------------------------------------

def search_beatport_bpm(title: str, artist: str | None,
                        detected_bpm: float | None = None) -> dict | None:
    """Search Beatport for a track and extract BPM from the SSR JSON data."""
    query = title
    if artist:
        first_artist = re.split(r'[,&]', artist)[0].strip()
        query = f"{first_artist} {title}"

    search_query = re.sub(r'\s*\(.*?\)\s*', ' ', query).strip()
    url = f"https://www.beatport.com/search?q={quote_plus(search_query)}"
    headers = {
        "User-Agent": "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
        "Accept": "text/html,application/xhtml+xml",
    }

    try:
        req = Request(url, headers=headers)
        with urlopen(req, timeout=15) as resp:
            html = resp.read().decode("utf-8", errors="replace")

        result = extract_bpm_from_nextdata(html, title, artist, detected_bpm)
        if result:
            result["url"] = url
            return result

    except (URLError, HTTPError, TimeoutError, OSError) as e:
        return {"error": str(e)}

    return None


def extract_bpm_from_nextdata(html: str, title: str, artist: str | None,
                               detected_bpm: float | None) -> dict | None:
    """Extract BPM from Beatport's Next.js SSR JSON embedded in the page."""
    all_scripts = re.findall(r'<script[^>]*>(.*?)</script>', html, re.DOTALL)
    data_scripts = [s for s in all_scripts if s.startswith('{"props')]

    if not data_scripts:
        return None

    try:
        data = json.loads(data_scripts[0])
        queries = data['props']['pageProps']['dehydratedState']['queries']

        for query in queries:
            state_data = query.get('state', {}).get('data', {})
            tracks_wrapper = state_data.get('tracks', {})
            if not isinstance(tracks_wrapper, dict):
                continue
            tracks = tracks_wrapper.get('data', [])
            if not isinstance(tracks, list) or not tracks:
                continue

            match = find_best_track_match(tracks, title, artist)
            if match:
                raw_bpm = match.get('bpm')
                if raw_bpm and isinstance(raw_bpm, (int, float)) and raw_bpm > 0:
                    bpm = resolve_tempo_octave(raw_bpm, detected_bpm)
                    match_name = match.get('track_name', '?')
                    match_artists = [a.get('artist_name', '') for a in match.get('artists', []) if a]
                    return {
                        "bpm": bpm,
                        "raw_bpm": raw_bpm,
                        "source": "beatport",
                        "matched_track": match_name,
                        "matched_artists": match_artists,
                    }

    except (json.JSONDecodeError, KeyError, TypeError):
        pass

    return None


def find_best_track_match(tracks: list, title: str, artist: str | None) -> dict | None:
    """Find the best matching track from Beatport search results.

    Requires both title AND artist to match (at least partially) to avoid
    false positives from common track names by different artists.
    """
    norm_title = normalize_for_match(title)
    norm_artist = normalize_for_match(artist) if artist else None

    best = None
    best_score = -1

    for t in tracks:
        if not isinstance(t, dict):
            continue
        bp_name = t.get('track_name', '') or ''
        bp_mix = t.get('mix_name', '') or ''
        bp_artists = [a.get('artist_name', '') for a in t.get('artists', []) if a and a.get('artist_name')]

        title_score = 0
        artist_score = 0

        norm_bp_name = normalize_for_match(bp_name)
        norm_bp_full = normalize_for_match(f"{bp_name} {bp_mix}")
        if norm_title == norm_bp_name:
            title_score = 10
        elif norm_title in norm_bp_name or norm_bp_name in norm_title:
            title_score = 5
        elif norm_title in norm_bp_full:
            title_score = 3

        if norm_artist:
            for bp_art in bp_artists:
                norm_bp_art = normalize_for_match(bp_art)
                if norm_artist == norm_bp_art:
                    artist_score = 10
                    break
                elif norm_artist in norm_bp_art or norm_bp_art in norm_artist:
                    artist_score = 5
                    break

        if title_score == 0 or (norm_artist and artist_score == 0):
            continue

        combined = title_score + artist_score
        if combined > best_score:
            best_score = combined
            best = t

    return best if best_score >= 10 else None


def normalize_for_match(text: str) -> str:
    """Normalize text for fuzzy matching."""
    text = text.lower().strip()
    text = re.sub(r'\s*\(.*?\)\s*', ' ', text)
    text = re.sub(r'\s*\[.*?\]\s*', ' ', text)
    text = re.sub(r'[^a-z0-9\s]', '', text)
    text = re.sub(r'\s+', ' ', text).strip()
    return text


def resolve_tempo_octave(beatport_bpm: int | float, detected_bpm: float | None) -> int:
    """Resolve Beatport's half-tempo convention.

    Beatport often reports DnB at half-tempo (86 instead of 172).
    If the detected BPM is roughly double the Beatport BPM, use the doubled value.
    """
    bp = int(beatport_bpm)

    if detected_bpm is None:
        return bp

    doubled = bp * 2
    if abs(detected_bpm - doubled) < abs(detected_bpm - bp):
        return doubled

    return bp


# ---------------------------------------------------------------------------
# Ground truth management
# ---------------------------------------------------------------------------

def load_ground_truth(path: str) -> dict:
    """Load ground truth BPMs from a JSON file."""
    if not os.path.exists(path):
        return {}
    with open(path) as f:
        return json.load(f)


def normalize_name(name: str) -> str:
    """Normalize a track name for fuzzy matching against ground truth."""
    name = name.lower().strip()
    name = strip_numeric_prefix(name)
    name = re.sub(r'\s*\(.*?\)\s*', ' ', name)
    name = re.sub(r'\s*\[.*?\]\s*', ' ', name)
    name = re.sub(r'\s+', ' ', name).strip()
    return name


def find_ground_truth_bpm(title: str, artist: str | None, ground_truth: dict) -> float | None:
    """Find a track's ground truth BPM using fuzzy name matching."""
    candidates = [title]
    if artist:
        candidates.extend([
            f"{artist} - {title}",
            f"{title} - {artist}",
        ])

    for key, bpm in ground_truth.items():
        if key in candidates:
            return bpm

    norm_candidates = [normalize_name(c) for c in candidates]
    for key, bpm in ground_truth.items():
        norm_key = normalize_name(key)
        for nc in norm_candidates:
            if norm_key == nc:
                return bpm
            if nc in norm_key or norm_key in nc:
                return bpm

    return None


# ---------------------------------------------------------------------------
# Parallel scraping
# ---------------------------------------------------------------------------

def scrape_batch(tasks: list[dict], workers: int = 8) -> dict[str, dict]:
    """Scrape Beatport for a batch of tracks in parallel.

    tasks: list of {key, title, artist, detected_bpm}
    Returns: dict mapping key -> scrape result (or None).
    """
    results = {}

    def _fetch(task):
        result = search_beatport_bpm(task["title"], task["artist"], task["detected_bpm"])
        return task["key"], result

    done = 0
    total = len(tasks)
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(_fetch, t): t for t in tasks}
        for future in as_completed(futures):
            key, result = future.result()
            results[key] = result
            done += 1
            task = futures[future]
            label = f"{task['artist']} - {task['title']}" if task['artist'] else task['title']
            if result and "bpm" in result:
                raw = result.get("raw_bpm", result["bpm"])
                suffix = f" (raw={raw})" if raw != result["bpm"] else ""
                matched = result.get("matched_track", "")
                print(f"  [{done}/{total}] {label} → {result['bpm']} BPM{suffix} [{matched}]")
            elif result and "error" in result:
                print(f"  [{done}/{total}] {label} → error: {result['error']}", file=sys.stderr)
            else:
                print(f"  [{done}/{total}] {label} → not found")

    return results


# ---------------------------------------------------------------------------
# Statistics
# ---------------------------------------------------------------------------

def compute_statistics(comparisons: list, bpm_field: str) -> dict:
    """Compute accuracy statistics from comparison results for a given BPM field."""
    if not comparisons:
        return {"error": "no comparisons available"}

    errors = []
    for c in comparisons:
        gt = c.get("ground_truth_bpm")
        detected = c.get(bpm_field)
        if gt is not None and detected is not None:
            errors.append(detected - gt)

    abs_errors = [abs(e) for e in errors]

    if not abs_errors:
        return {"total_tracks": len(comparisons), "tracks_with_ground_truth": 0}

    n = len(abs_errors)
    mean_abs_error = sum(abs_errors) / n
    max_error = max(abs_errors)
    within_1 = sum(1 for e in abs_errors if e <= 1.0)
    within_2 = sum(1 for e in abs_errors if e <= 2.0)
    within_5 = sum(1 for e in abs_errors if e <= 5.0)
    octave_errors = sum(1 for e in abs_errors if e > 10.0)

    sorted_errors = sorted(abs_errors)
    if n % 2:
        median_error = sorted_errors[n // 2]
    else:
        median_error = (sorted_errors[n // 2 - 1] + sorted_errors[n // 2]) / 2

    return {
        "total_tracks": len(comparisons),
        "tracks_with_ground_truth": n,
        "mean_absolute_error_bpm": round(mean_abs_error, 2),
        "median_absolute_error_bpm": round(median_error, 2),
        "max_absolute_error_bpm": round(max_error, 2),
        "within_1_bpm": within_1,
        "within_1_bpm_pct": round(100 * within_1 / n, 1),
        "within_2_bpm": within_2,
        "within_2_bpm_pct": round(100 * within_2 / n, 1),
        "within_5_bpm": within_5,
        "within_5_bpm_pct": round(100 * within_5 / n, 1),
        "tempo_octave_errors": octave_errors,
    }


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def print_report(bt_stats: dict, es_stats: dict, comparisons: list, out=sys.stdout):
    """Print formatted comparison report."""
    p = lambda *a, **kw: print(*a, **kw, file=out)

    p(f"\n{'='*70}")
    p(f"  BPM Comparison — Beat This! (Advanced) vs Essentia (Simple)")
    p(f"{'='*70}")
    p(f"  Total tracks:           {bt_stats.get('total_tracks', 0)}")
    p(f"  With ground truth:      {bt_stats.get('tracks_with_ground_truth', 0)}")

    if bt_stats.get("tracks_with_ground_truth", 0) > 0:
        n = bt_stats["tracks_with_ground_truth"]
        p(f"\n  {'Metric':<28s} {'Beat This!':>12s} {'Essentia':>12s}")
        p(f"  {'-'*52}")
        for label, key in [
            ("Mean absolute error", "mean_absolute_error_bpm"),
            ("Median absolute error", "median_absolute_error_bpm"),
            ("Max absolute error", "max_absolute_error_bpm"),
        ]:
            p(f"  {label:<28s} {bt_stats[key]:>10.2f}   {es_stats[key]:>10.2f}")

        for label, key, pct_key in [
            ("Within 1 BPM", "within_1_bpm", "within_1_bpm_pct"),
            ("Within 2 BPM", "within_2_bpm", "within_2_bpm_pct"),
            ("Within 5 BPM", "within_5_bpm", "within_5_bpm_pct"),
        ]:
            bt_v, es_v = bt_stats[key], es_stats[key]
            bt_p, es_p = bt_stats[pct_key], es_stats[pct_key]
            p(f"  {label:<28s} {bt_v:>4d}/{n} ({bt_p:>5.1f}%)  {es_v:>4d}/{n} ({es_p:>5.1f}%)")

        p(f"  {'Tempo octave errors (>10)':<28s} {bt_stats['tempo_octave_errors']:>10d}   {es_stats['tempo_octave_errors']:>10d}")

    # Worst offenders
    bt_worst = [c for c in comparisons
                if c["beat_this_error"] is not None and abs(c["beat_this_error"]) > 2.0]
    if bt_worst:
        p(f"\n  Beat This! — Tracks with >2 BPM error:")
        for c in bt_worst[:15]:
            es_err = f"{c['essentia_error']:+.1f}" if c['essentia_error'] is not None else "n/a"
            label = f"{c['artist']} - {c['title']}" if c['artist'] else c['title']
            p(f"    {label}")
            p(f"      BT={c['beat_this_bpm']:.1f}  ES={c['essentia_bpm']}  GT={c['ground_truth_bpm']}"
              f"  BT_err={c['beat_this_error']:+.1f}  ES_err={es_err}")

    # Where each is worse
    es_worse = [c for c in comparisons
                if c["beat_this_error"] is not None and c["essentia_error"] is not None
                and abs(c["essentia_error"]) > abs(c["beat_this_error"]) + 1.0
                and abs(c["essentia_error"]) > 2.0]
    if es_worse:
        p(f"\n  Tracks where Essentia is worse than Beat This!:")
        for c in es_worse[:10]:
            label = f"{c['artist']} - {c['title']}" if c['artist'] else c['title']
            p(f"    {label}: BT_err={c['beat_this_error']:+.1f}, ES_err={c['essentia_error']:+.1f}")

    bt_worse = [c for c in comparisons
                if c["beat_this_error"] is not None and c["essentia_error"] is not None
                and abs(c["beat_this_error"]) > abs(c["essentia_error"]) + 1.0
                and abs(c["beat_this_error"]) > 2.0]
    if bt_worse:
        p(f"\n  Tracks where Beat This! is worse than Essentia:")
        for c in bt_worse[:10]:
            label = f"{c['artist']} - {c['title']}" if c['artist'] else c['title']
            p(f"    {label}: BT_err={c['beat_this_error']:+.1f}, ES_err={c['essentia_error']:+.1f}")

    p(f"{'='*70}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Compare detected BPMs against Beatport ground truth")
    parser.add_argument("export_json",
                        help="Path to analysis-export.json from export-analysis")
    parser.add_argument("--ground-truth", "-g",
                        help="Path to ground truth JSON")
    parser.add_argument("--output", "-o", default="bpm-comparison.json",
                        help="Output comparison JSON (default: bpm-comparison.json)")
    parser.add_argument("--scrape", action="store_true",
                        help="Scrape Beatport for missing ground truth BPMs")
    parser.add_argument("--workers", type=int, default=8,
                        help="Parallel scrape workers (default: 8)")
    parser.add_argument("--limit", type=int, default=0,
                        help="Limit number of tracks to scrape (0 = all)")
    args = parser.parse_args()

    # Load export data
    with open(args.export_json) as f:
        export_data = json.load(f)

    tracks = export_data.get("tracks", [])
    print(f"Loaded {len(tracks)} tracks from {args.export_json}")

    # Load or build ground truth
    gt_path = args.ground_truth or "ground-truth.json"
    ground_truth = load_ground_truth(gt_path)
    if ground_truth:
        print(f"Loaded {len(ground_truth)} ground truth entries from {gt_path}")

    # Parse all tracks
    parsed = []
    for track in tracks:
        title, artist = parse_track_fields(track["name"], track.get("artist"))
        parsed.append({
            "title": title,
            "artist": artist,
            "beat_this_bpm": track.get("bpm"),
            "essentia_bpm": track.get("original_bpm"),
            "key": track.get("key"),
            "duration_seconds": track.get("duration_seconds"),
            "lufs": track.get("lufs"),
            "first_beat_sample": track.get("first_beat_sample"),
        })

    # Identify tracks needing scraping
    if args.scrape:
        scrape_tasks = []
        for p in parsed:
            if find_ground_truth_bpm(p["title"], p["artist"], ground_truth) is None:
                key = f"{p['artist']} - {p['title']}" if p['artist'] else p['title']
                scrape_tasks.append({
                    "key": key,
                    "title": p["title"],
                    "artist": p["artist"],
                    "detected_bpm": p["beat_this_bpm"],
                })

        if args.limit:
            scrape_tasks = scrape_tasks[:args.limit]

        if scrape_tasks:
            print(f"\nScraping Beatport for {len(scrape_tasks)} tracks ({args.workers} workers)...")
            results = scrape_batch(scrape_tasks, workers=args.workers)

            found = 0
            for key, result in results.items():
                if result and "bpm" in result:
                    ground_truth[key] = result["bpm"]
                    found += 1
            print(f"Found {found}/{len(scrape_tasks)} BPMs on Beatport")

    # Build comparison results
    comparisons = []
    for p in parsed:
        gt_bpm = find_ground_truth_bpm(p["title"], p["artist"], ground_truth)

        bt_error = None
        es_error = None
        if gt_bpm is not None:
            if p["beat_this_bpm"] is not None:
                bt_error = round(p["beat_this_bpm"] - gt_bpm, 2)
            if p["essentia_bpm"] is not None:
                es_error = round(p["essentia_bpm"] - gt_bpm, 2)

        comparisons.append({
            "title": p["title"],
            "artist": p["artist"],
            "beat_this_bpm": p["beat_this_bpm"],
            "essentia_bpm": p["essentia_bpm"],
            "ground_truth_bpm": gt_bpm,
            "beat_this_error": bt_error,
            "essentia_error": es_error,
            "key": p["key"],
            "duration_seconds": p["duration_seconds"],
            "lufs": p["lufs"],
            "first_beat_sample": p["first_beat_sample"],
        })

    # Statistics
    bt_stats = compute_statistics(comparisons, "beat_this_bpm")
    es_stats = compute_statistics(comparisons, "essentia_bpm")

    # Sort: ground truth first, worst error first
    comparisons.sort(key=lambda c: (
        c["beat_this_error"] is None,
        -abs(c["beat_this_error"]) if c["beat_this_error"] is not None else 0
    ))

    # Write JSON output
    output = {
        "beat_this_statistics": bt_stats,
        "essentia_statistics": es_stats,
        "comparisons": comparisons,
    }

    with open(args.output, "w") as f:
        json.dump(output, f, indent=2)
    print(f"Wrote comparison to {args.output}")

    # Save updated ground truth
    if args.scrape and ground_truth:
        with open(gt_path, "w") as f:
            json.dump(ground_truth, f, indent=2, sort_keys=True)
        print(f"Saved {len(ground_truth)} ground truth entries to {gt_path}")

    # Print report
    print_report(bt_stats, es_stats, comparisons)


if __name__ == "__main__":
    main()
