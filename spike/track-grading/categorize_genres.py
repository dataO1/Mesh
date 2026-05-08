"""Categorize the everynoise genres as DJ-relevant or not.

Permissive: include anything that plausibly plays in a DJ set OR is a
production-lineage neighbour of DJ music. Strict-out only the clearly-not
genres (jazz, blues, soul, classical, folk/country, gospel, etc.).

Rule:
  is_dj = (any INCLUDE_TERM in name) AND (no BLOCK_TERM in name)
The INCLUDE_TERMS list is the broad net; BLOCK_TERMS overrides ONLY for
genres that match an INCLUDE_TERM via a too-loose substring (e.g. "soul
house" still matches via "house" + would never match BLOCK_TERMS since
"soul" isn't on the override list — a "soul" hit only blocks if the genre
ALSO didn't match the include set, which it can't if it contains
"house"). In short: INCLUDE wins, BLOCK only applies to genres that
weren't going to be included anyway. Documented for clarity.

Outputs:
  /home/data01/Music/mesh-track-grading/everynoise_dj_genres.json     — the included subset
  /home/data01/Music/mesh-track-grading/everynoise_excluded_sample.json  — sample of excluded
                                                       genres for spot-check
"""
from __future__ import annotations

import json
import sys
from pathlib import Path


IN_PATH = Path("/home/data01/Music/mesh-track-grading/everynoise_genres.json")
OUT_INCLUDED = Path("/home/data01/Music/mesh-track-grading/everynoise_dj_genres.json")
OUT_EXCLUDED_SAMPLE = Path("/home/data01/Music/mesh-track-grading/everynoise_excluded_sample.json")


# Core DJ-relevant terms (broad net). Any genre whose name contains any of
# these is INCLUDED regardless of other substrings, because these terms are
# specific enough that the genre is fundamentally DJ-adjacent.
INCLUDE_TERMS = {
    # House family
    "house",
    # Techno family
    "techno", "schranz",
    # Trance family
    "trance", "psytrance", "goa", "uplifting", "psy-",
    # DnB / jungle / breaks
    "dnb", "drum and bass", "drumstep", "jungle", "breakbeat", "breakcore",
    "jump-up", "neuro", "liquid funk", "atmospheric", "darkstep", "techstep",
    # Dubstep / bass music
    "dubstep", "brostep", "riddim", "color bass", "future bass", "trapstep",
    "ohmstep", "deathstep", "wonky",
    # Garage / 2-step
    "garage", "2-step", "2step", "speed garage", "uk garage", "bassline",
    # Hardcore / hardstyle / gabber / industrial
    "hardstyle", "gabber", "hardcore techno", "hardcore breaks", "happy hardcore",
    "frenchcore", "tekno", "tek", "rawstyle", "uptempo", "millennium hardcore",
    "industrial", "ebm", "darkwave", "coldwave", "witch house", "synth-industrial",
    # Electro / EDM / pop crossover
    "electro", "edm", "electronic", "electronica", "electroclash",
    # Idm / glitch / experimental
    "idm", "glitch", "ambient", "downtempo", "trip hop", "trip-hop", "big beat",
    "drone", "noise", "experimental electronic", "intelligent dance",
    # Phonk / hyperpop / digicore / modern hyper
    "phonk", "hyperpop", "digicore", "glitchcore", "hexd", "dariacore",
    # Synthwave / vaporwave / futurefunk / hauntology
    "synthwave", "vaporwave", "futurefunk", "future funk", "hauntology",
    "chillwave", "outrun", "darksynth", "retrowave",
    # World / latin / afro dance
    "reggaeton", "dembow", "baile", "moombahton", "amapiano", "gqom", "kuduro",
    "kwaito", "afro house", "afrohouse", "dancehall", "ragga", "kompa",
    "shatta", "afrobeats",
    # Disco / italo / hi-nrg / club heritage
    "disco", "italo", "hi-nrg", "high-nrg", "eurodance", "eurobeat",
    # Footwork / juke / jersey club / ballroom
    "footwork", "juke", "jersey club", "ballroom", "bmore",
    # General "dance" + "club" tags (broad)
    "dance pop", "edm pop", "club", "festival", "future pop",
    # Trap (the EDM kind — also catches hip-hop trap which is DJ-played)
    "trap",
    # Modern aggressive pop / drift / phonk
    "drift",
    # Wave / wonky / weird
    "wave",
    # Hardcore (general — most "hardcore" tags are DJ-relevant; punk hardcore
    # gets caught by the BLOCK list below if needed)
    "hardcore",
    # New rave / new beat
    "new rave", "new beat", "rave",
    # Modern bass-music umbrella terms
    "bass music",
    # Hip-hop / rap / drill family — broadly DJ-played, especially regional
    # variants (tennessee hip hop, dirty south rap, slovak hip hop, etc.)
    "hip hop", "hip-hop", "hiphop", "rap", "drill", "trap soul",
    # Dub heritage (ragga, dub, french dub, electro dub)
    "dub",
    # Afrobeats / alte / amapiano-adjacent African DJ music
    "afrobeats", "afrobeat", "afrofusion", "alte",
    # K-pop / j-pop / mandopop only when explicitly tagged for dance
    "k-pop edm", "j-pop edm",
    # Jersey / Baltimore club + uk drill + grime + bmore
    "grime", "bmore",
    # Old-school hip-hop subgenres
    "boom bap", "g-funk", "g funk", "trip-hop", "triphop",
    # R&B (modern is club-played; "neo soul" stays blocked via SOFT_BLOCK)
    "r&b", "rnb", "r and b", "modern r&b",
    # Reggae heritage (dub originated here; ragga/dancehall already in)
    "reggae",
    # Turntablism / DJ technique
    "scratch", "turntab",
    # Latin / afro dance: kizomba, semba — DJ-played in latin/afro venues
    "kizomba", "semba", "kompa", "zouk", "soca",
    # Specific DJ-adjacent micro-genres I missed
    "happy hardcore", "uk hardcore", "bouncy", "donk",
    # Egyptian / Middle Eastern dance
    "mahraganat", "shaabi",
    # Aphex-style IDM lineage
    "braindance",
    # Psy- variants without the hyphen (psytrance/psybient already covered
    # via "psytrance" and "goa")
    "psybient", "psychill", "psybass", "psydub",
    # Modern aggressive electronic + bass micro-genres
    "wonky", "post-club",
    # Punk + metal + adjacent — broad include (per user "don't exclude
    # punk/metal per-se"). Loose net: their audio characteristics
    # (distortion, density, aggression) overlap with DJ-relevant axes
    # even if the genre itself isn't typically DJ-played.
    "punk", "metal", "emo", "screamo", "metalcore", "deathcore",
    "post-punk", "goth", "gothic",
    "doom", "djent", "grindcore", "powerviolence", "hardcoresynth",
    "thrash", "death metal", "black metal", "speedmetal", "speed metal",
    "nu-metal", "nu metal",
    # Regional bass-music dialects
    "miami bass", "atlanta bass", "memphis bass", "ghetto bass", "uk bass",
    # Synth-pop family
    "synthpop", "synth pop", "synth-pop", "synth funk", "synth-funk",
    # Modern hip-hop subgenres
    "plugg", "rage rap", "drill rap", "horrorcore", "scenecore",
    # Vaporwave / vapor-anything
    "vapor",
}


# HARD_BLOCK: specific phrases that override an INCLUDE match. These are
# names where a DJ-relevant word appears as a *modifier* of a non-DJ noun
# (e.g. "garage rock" = a rock-band genre, not the UK garage scene).
# Order of precedence: HARD_BLOCK > INCLUDE > SOFT_BLOCK > NEUTRAL.
HARD_BLOCK_TERMS = {
    "blues rock",
    "rockabilly",
    "surf rock",
    "americana",
    "country rock",
    "country pop",
    "soft rock",
    "classic rock",
    "prog rock",
    "art rock",
    "folk rock",
    "indie rock",
    "alternative rock",
    "pop rock",
    "garage rock",        # rock-band garage, not UK garage dance
    "rock and roll",
    "post-rock",
    "math rock",
    "barbershop",
    "a cappella",
    "gregorian",
    "spoken word",
    "audiobook",
    "comedy",
    "musical theater",
    "broadway",
    "show tune",
    "marching band",
    "marching",
    "anthem",
    "national anthem",
    "lullaby",
    "nursery",
    "children",
    "kids",
    "religious",
    "worship",
    "christian",
    "gospel",
    "hymn",
    "liturgical",
    "sermon",
    "prayer",
    "contemporary classical",
    "post-classical",
    "neoclassical",
    "neo-classical",
    "baroque",
    "chamber music",
    "string quartet",
    "opera",
    "operetta",
    "choral",
    "choir",
    "early music",
}


# SOFT_BLOCK: broad terms checked only when no INCLUDE matched. Lets
# "jazz house", "soulful deep house" stay INCLUDE via the "house" hit
# while "neo soul", "british soul", bare "country" / "folk" / "blues"
# get blocked.
SOFT_BLOCK_TERMS = {
    "jazz",
    "blues",
    "soul",
    "country",
    "folk",
    "gospel",
    "bluegrass",
    "classical",
    "polka",
    "mariachi",
    "bossa nova",
    "samba",                 # samba-house is INCLUDE via house; bare samba is not
    "tango",
    "fado",
    "flamenco",
    "schlager",
    "ranchera",
    "norteno",
    "cumbia",                # latin folkloric variant; reggaeton/latin trap kept via include
    "vallenato",
    "merengue",
    "bachata",
    "calypso",
    # Rock / indie — listening genres. Punk / metal / emo / screamo /
    # post-punk / goth REMOVED from this list per "don't exclude punk
    # and metal per-se" — they're now in INCLUDE.
    "rock",
    "indie",
    "shoegaze",
    "ska",
    # Singer-songwriter, acoustic, traditional listening
    "singer-songwriter",
    "songwriter",
    "acoustic",
    "ballad",
    "lullaby",
    "traditional",
    # Soundtrack / classical adjacent
    "soundtrack",
    "score",
    "film score",
    "orchestral",            # blocks "orchestral" if not paired with electronic
    "symphony",
    "concerto",
    "sonata",
    "violin",                # solo classical instrument
    "piano sonata",
    "harpsichord",
    "renaissance",
    "medieval",
    # Jazz-adjacent terms not covered by bare "jazz"
    "bop",                   # bebop, post-bop, hard bop, neo-bop
    "swing",                 # mostly jazz/big-band swing
    "fusion jazz",
    # Christian / new age listening
    "ccm",
    "new age",
    # Generic listening descriptors
    "easy listening",
    "smooth",                # smooth jazz, smooth saxophone
    "cocktail",
    # Genre-modifier non-DJ
    "noteworthy",
    "vocal harmony",
}


# Genres that match NEITHER include nor block: NEUTRAL bucket. Sample these
# for review since they're the borderline cases.


def classify(name: str) -> str:
    """Three-tier rule, precedence top-down:
      1. HARD_BLOCK — specific compound phrases (e.g. "garage rock") win
         over INCLUDE matches; these are non-DJ genres that happen to
         share a word with a DJ term.
      2. INCLUDE — broad net of DJ-relevant terms; matches imply include.
      3. SOFT_BLOCK — broad listening genres (jazz, soul, country, folk,
         classical, etc.) blocked only when no INCLUDE match was found.
      4. otherwise NEUTRAL (default-excluded from the corpus).
    """
    n = name.lower()
    if any(term in n for term in HARD_BLOCK_TERMS):
        return "BLOCK"
    if any(term in n for term in INCLUDE_TERMS):
        return "INCLUDE"
    if any(term in n for term in SOFT_BLOCK_TERMS):
        return "BLOCK"
    return "NEUTRAL"


def main() -> int:
    if not IN_PATH.exists():
        sys.exit(f"missing {IN_PATH} — run scrape_everynoise.py first")
    data = json.loads(IN_PATH.read_text())
    genres = data["genres"]

    bucketed: dict[str, list[dict]] = {"INCLUDE": [], "BLOCK": [], "NEUTRAL": []}
    for g in genres:
        bucket = classify(g["name"])
        bucketed[bucket].append(g)

    print(f"total genres: {len(genres)}")
    print(f"  INCLUDE (DJ-relevant)        : {len(bucketed['INCLUDE']):>5}")
    print(f"  BLOCK   (clearly not DJ)     : {len(bucketed['BLOCK']):>5}")
    print(f"  NEUTRAL (review borderline)  : {len(bucketed['NEUTRAL']):>5}")

    print()
    print("=== INCLUDE sample (first 30, every 10th from full list) ===")
    inc_sample = bucketed["INCLUDE"][::max(1, len(bucketed["INCLUDE"]) // 30)][:30]
    for g in inc_sample:
        print(f"  {g['name']}")

    print()
    print("=== NEUTRAL — sampling for review (every 30th) ===")
    neu_sample = bucketed["NEUTRAL"][::max(1, len(bucketed["NEUTRAL"]) // 60)][:60]
    for g in neu_sample:
        print(f"  {g['name']}")

    print()
    print("=== BLOCK sample (first 20) ===")
    for g in bucketed["BLOCK"][:20]:
        print(f"  {g['name']}")

    OUT_INCLUDED.write_text(json.dumps({
        "source": IN_PATH.name,
        "n_included": len(bucketed["INCLUDE"]),
        "rule": "HARD_BLOCK > INCLUDE > SOFT_BLOCK > NEUTRAL (see classify())",
        "include_terms_count": len(INCLUDE_TERMS),
        "hard_block_terms_count": len(HARD_BLOCK_TERMS),
        "soft_block_terms_count": len(SOFT_BLOCK_TERMS),
        "genres": bucketed["INCLUDE"],
    }, indent=2))
    print(f"\n[ok] wrote {OUT_INCLUDED}  ({OUT_INCLUDED.stat().st_size/1024:.0f} KB)")

    OUT_EXCLUDED_SAMPLE.write_text(json.dumps({
        "blocked": [g["name"] for g in bucketed["BLOCK"]][:200],
        "neutral": [g["name"] for g in bucketed["NEUTRAL"]][:300],
    }, indent=2))
    print(f"[ok] wrote {OUT_EXCLUDED_SAMPLE}  ({OUT_EXCLUDED_SAMPLE.stat().st_size/1024:.0f} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
