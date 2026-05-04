"""Scrape everynoise.com homepage → JSON of all genres.

Each genre cell in the everynoise HTML is a `<div class="genre scanme">` with
attributes encoding:
  - preview_url    — 30s Spotify preview MP3 (the audible sample)
  - style/color    — hex colour; encodes two audio dimensions (R≈organic→
                     mechanical, G≈dense→spiky, B≈calm→energetic per
                     everynoise's FAQ)
  - style/top      — Y coordinate; rough proxy for "downtempo→uptempo" axis
  - style/left     — X coordinate; rough proxy for "mellow→energetic" axis
  - style/fontSize — relative cluster size / popularity
  - onclick        — calls playx("playlist_id", "genre_name", this)
  - title          — example artist + track ("e.g. Artist - Title")
  - inner text     — the genre name
  - <a href>       — sometimes points to a per-genre engenremap-*.html subpage

Output: /tmp/track-grading/everynoise_genres.json — one entry per genre with
all of the above. Used as the seed list for round-7 corpus building.

Usage:
  ~/.cache/mesh-spike/vllm-env/bin/python spike/track-grading/scrape_everynoise.py
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import requests
from bs4 import BeautifulSoup


URL = "https://everynoise.com/"
OUT_PATH = Path("/tmp/track-grading/everynoise_genres.json")


def parse_style(style: str) -> dict:
    """Pull the relevant numeric fields out of an inline `style="..."` blob."""
    out = {}
    for piece in style.split(";"):
        if ":" not in piece: continue
        k, v = piece.split(":", 1)
        k, v = k.strip(), v.strip()
        if k == "color":
            out["color"] = v
        elif k == "top":
            m = re.match(r"(-?\d+)px", v)
            if m: out["top"] = int(m.group(1))
        elif k == "left":
            m = re.match(r"(-?\d+)px", v)
            if m: out["left"] = int(m.group(1))
        elif k == "font-size":
            m = re.match(r"(\d+)%", v)
            if m: out["font_size_pct"] = int(m.group(1))
    return out


def parse_onclick(onclick: str) -> tuple[str | None, str | None]:
    """playx("PLAYLIST_ID", "genre_name", this) → (playlist_id, genre)."""
    m = re.search(
        r'playx\(\s*&quot;([^&]+)&quot;\s*,\s*&quot;([^&]+)&quot;',
        onclick,
    )
    if m:
        return m.group(1), m.group(2)
    # Fallback for un-html-escaped quotes
    m = re.search(r'playx\(\s*"([^"]+)"\s*,\s*"([^"]+)"', onclick)
    if m:
        return m.group(1), m.group(2)
    return None, None


def main() -> int:
    print(f"[scrape] fetching {URL} ...")
    r = requests.get(URL, timeout=30, headers={"User-Agent": "mesh-research/1.0"})
    r.raise_for_status()
    html = r.text
    print(f"[scrape] {len(html)/1024:.1f} KB downloaded")

    soup = BeautifulSoup(html, "lxml")
    genres = soup.select("div.genre.scanme")
    print(f"[scrape] {len(genres)} genre cells found")

    rows: list[dict] = []
    for div in genres:
        style = parse_style(div.get("style", ""))
        playlist_id, genre_token = parse_onclick(div.get("onclick", ""))
        title = div.get("title", "")
        # Most genre cells contain "{name} » " (the subpage link follows).
        # Inner text without the trailing arrow is the canonical name.
        text = div.get_text(strip=True)
        name = text.split("»")[0].strip()
        # Subpage link if present (engenremap-{slug}.html relative URL).
        a = div.find("a")
        subpage = None
        if a and a.get("href", "").startswith("engenremap-"):
            subpage = a["href"]
        rows.append({
            "name": name,
            "playlist_id": playlist_id,
            "preview_url": div.get("preview_url"),
            "color": style.get("color"),
            "top": style.get("top"),
            "left": style.get("left"),
            "font_size_pct": style.get("font_size_pct"),
            "example_track": title,
            "subpage": subpage,
        })

    # Quick coverage stats
    n_with_playlist = sum(1 for r in rows if r["playlist_id"])
    n_with_preview  = sum(1 for r in rows if r["preview_url"])
    n_with_subpage  = sum(1 for r in rows if r["subpage"])
    print(f"[scrape] genres with playlist_id: {n_with_playlist}/{len(rows)}")
    print(f"[scrape] genres with preview_url: {n_with_preview}/{len(rows)}")
    print(f"[scrape] genres with subpage:     {n_with_subpage}/{len(rows)}")

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(json.dumps({
        "source": URL,
        "scraped_at": __import__("datetime").datetime.now(__import__("datetime").UTC).isoformat(),
        "n_genres": len(rows),
        "genres": rows,
    }, indent=2))
    print(f"[scrape] wrote {OUT_PATH}  ({OUT_PATH.stat().st_size/1024:.0f} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
