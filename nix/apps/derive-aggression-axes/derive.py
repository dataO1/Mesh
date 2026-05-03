"""Derive intensity-axis variants from MuQ-MuLan's text tower.

Each variant defines:
  - one or more sub-axes (semantic polar prompts)
  - a combination formula that produces a single 512-d `intensity_axis_vec`

The intensity axis is what mesh-cue projects audio embeddings onto at runtime.
Sub-axes are kept in the artifact for diagnostics + the eval CLI.

We deliberately ship many variants so the eval pass can compare their rankings
on the user's library and pick the most useful direction. Variants are
designed to disagree — single-concept axes vs blends, different weightings
vs different sub-axis sets.

Run:
  python derive.py [output_dir]
    output_dir defaults to ./models/aggression-axes/

Reads MuQ-MuLan from the standard HF cache (already warm from the audio
exporter). Text tower is small — runs in seconds on CPU.
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import torch

# ─── Sub-axis library ───────────────────────────────────────────────────────
#
# Each entry = (axis_name, positive_prompts, negative_prompts).
# The polar axis vector for each is computed as:
#     unit( mean(text_tower(positive)) - mean(text_tower(negative)) )
#
# Positive direction = "more of the axis name's first half"
# (e.g. "aggression" axis: positive = aggressive, negative = calm).

SUB_AXES = {
    "aggression": (
        [
            "aggressive heavy distorted hard techno with pounding kicks",
            "harsh industrial noise with abrasive screeching textures",
            "fast intense gabber kicks with overdriven distortion",
            "hard-hitting drum and bass with menacing bass and ripping snares",
            "brutal dark techno with crushing reese bass and metallic stabs",
            "aggressive hardcore with relentless 4/4 kicks and screaming leads",
            "intense peak-time techno with driving hypnotic energy",
            "heavy distorted electronic music with violent dynamics",
        ],
        [
            "calm peaceful ambient drone with soft warm pads",
            "gentle introspective downtempo with delicate piano",
            "slow meditative chillout with airy ethereal textures",
            "soft minimal techno with sparse warm bass and clean kicks",
            "smooth deep house with mellow chords and relaxed groove",
            "relaxed lo-fi beats with dreamy atmospheres",
            "tranquil soundscape with floating tones and no percussion",
            "warm soothing electronic ambient with no harshness",
        ],
    ),
    "distortion": (
        [
            "heavily distorted overdriven music with clipping and saturation",
            "fuzzy gritty sound with aggressive harmonic distortion",
            "saturated screaming synths with broken speaker textures",
            "raw lo-fi production with deliberate digital clipping",
            "abrasive noise music with crushed and bit-reduced timbres",
            "industrial textures with metallic distorted percussion",
            "harsh wall of distortion with no clean elements",
            "extreme distortion with mangled blown-out frequencies",
        ],
        [
            "clean polished production with smooth round tones",
            "pristine high-fidelity electronic music with no distortion",
            "soft analog warmth with gentle filtered tones",
            "clean digital synthesis with crystal clear textures",
            "polished pop production with no harsh elements",
            "smooth jazz-inspired electronic music with warm clean sound",
            "clean acoustic ambient with natural untreated tones",
            "transparent mastering with full dynamic range and clarity",
        ],
    ),
    "density": (
        [
            "dense busy production with many layered elements",
            "wall of sound with packed full-spectrum textures",
            "maximalist arrangement with overlapping rhythms and melodies",
            "thick heavy mix with no empty space anywhere",
            "complex polyrhythmic layering with constant motion",
            "saturated full mix with everything happening at once",
            "dense psychedelic textures with countless overlapping voices",
            "packed orchestral electronic music with crowded arrangements",
        ],
        [
            "sparse minimal arrangement with lots of empty space",
            "minimal techno with single elements at a time",
            "stripped-down rhythmic music with breathing room",
            "ambient piece with one or two slow elements only",
            "deep dub techno with vast empty silences between sounds",
            "skeletal arrangement with single kick and one bassline",
            "minimal click music with isolated percussive events",
            "sparse drone with single sustained tones over time",
        ],
    ),
    "darkness": (
        [
            "dark menacing music with sinister atmospheric tension",
            "ominous brooding soundscape with shadowy textures",
            "sinister horror-inspired electronic music with unsettling tones",
            "pitch black industrial sound with gothic atmospheres",
            "evil dystopian electronic music with apocalyptic mood",
            "dark dungeon synth with haunting cavernous reverb",
            "menacing techno with deep ominous sub-bass and metallic clangs",
            "occult ritualistic darkness with dread-inducing drones",
        ],
        [
            "bright uplifting music with cheerful sparkling synths",
            "happy euphoric trance with major-key melodies",
            "joyful summer house with bright sunny atmospheres",
            "playful bubblegum electronic pop with sweet melodies",
            "luminous shimmering ambient with warm golden tones",
            "celebratory festival anthem with triumphant fanfares",
            "light optimistic music with floating positive emotions",
            "radiant melodic synthwave with feel-good harmonies",
        ],
    ),
    "noisiness": (
        [
            "noisy harsh experimental music with chaotic frequencies",
            "abrasive electronic noise with dissonant textures",
            "screeching feedback-driven music with no recognizable melody",
            "static-filled glitchy production with ear-piercing artifacts",
            "raw noise wall with overwhelming spectrum",
            "industrial noise with grinding metallic shrieks",
            "harsh power electronics with jarring atonal clusters",
            "tinnitus-inducing noise music with extreme high frequencies",
        ],
        [
            "melodic singing music with clear catchy melodies",
            "harmonic chord-driven music with consonant progressions",
            "vocal-led song with memorable hummable tunes",
            "lyrical instrumental music with expressive melodic phrasing",
            "tonal classical-influenced electronic music with melodic lines",
            "flowing arpeggiated melodies with smooth voice leading",
            "song-form music with verses choruses and recognizable hooks",
            "tuneful synthesizer music with clear singable themes",
        ],
    ),
    "atonality": (
        [
            "atonal experimental music with no key center",
            "dissonant percussive music focused on rhythm not melody",
            "abstract sound design with no harmonic content",
            "rhythm-only techno with mechanical hits and no chord progression",
            "industrial techno built from non-pitched percussion and impacts",
            "atonal noise-techno with metallic hits and no tonal melody",
            "drum-focused hard techno with kicks claps stabs and no melodic line",
            "percussive tribal techno with toms and shakers but no harmony",
        ],
        [
            "melodic music with clear chord progressions and key center",
            "tonal harmonic music with traditional voice leading",
            "song-based music with verse-chorus melodic structure",
            "trance with euphoric arpeggios and major-key buildups",
            "melodic house with warm pad chords and singable hooks",
            "synthwave with clear melodic leads over harmonic basslines",
            "deep house with jazzy chord stabs and tonal bassline",
            "ambient music with consonant chord drones and harmonic stillness",
        ],
    ),
}

# ─── Variant library ───────────────────────────────────────────────────────
#
# A variant is a list of (sub_axis_name, weight). Weights need not sum to 1 —
# they will be summed and the result re-normalised to unit length. Ordering
# matters only for human readability.
#
# Goal: span the design space so resulting intensity rankings disagree
# enough to be informative.

VARIANTS = {
    "V1_pure_aggression": {
        "name": "Pure aggression↔calm — single polar axis (baseline)",
        "components": [("aggression", 1.0)],
        "rationale": "Most direct interpretation. Treats the chill↔aggressive vocabulary as the only signal.",
    },
    "V2_pure_distortion": {
        "name": "Distortion-only — pure timbral roughness signal",
        "components": [("distortion", 1.0)],
        "rationale": "Tests whether 'aggression' as a perceptual quality is dominated by timbre alone, ignoring tempo/density.",
    },
    "V3_pure_density": {
        "name": "Density-only — wall-of-sound vs minimalism",
        "components": [("density", 1.0)],
        "rationale": "Tests density as the discriminator. A minimal banger and a dense ambient piece will rank inversely vs V1/V2.",
    },
    "V4_blend_equal_3": {
        "name": "Equal-weight blend of aggression + distortion + density",
        "components": [("aggression", 1.0), ("distortion", 1.0), ("density", 1.0)],
        "rationale": "Naive composite. Tests whether democracy across three core axes beats any single one.",
    },
    "V5_aggression_led": {
        "name": "Aggression-led blend (0.6/0.2/0.2)",
        "components": [("aggression", 0.6), ("distortion", 0.2), ("density", 0.2)],
        "rationale": "Aggression as headline, distortion+density as supporting. Tests whether weighting the semantic axis dominantly while letting timbre+density nudge produces a more musically intuitive ordering than the equal-weight V4.",
    },
    "V6_five_axis_weighted": {
        "name": "Five-axis composite (0.4/0.2/0.2/0.1/0.1)",
        "components": [
            ("aggression", 0.4),
            ("distortion", 0.2),
            ("density", 0.2),
            ("darkness", 0.1),
            ("noisiness", 0.1),
        ],
        "rationale": "All five sub-axes contribute, aggression weighted highest. Maximally inclusive composite — tests whether bringing dark/noisy axes in adds discrimination or muddles it.",
    },
    "V7_dark_noisy_emphasis": {
        "name": "Dark+noisy emphasis (0.2/0.2/0.1/0.25/0.25)",
        "components": [
            ("aggression", 0.2),
            ("distortion", 0.2),
            ("density", 0.1),
            ("darkness", 0.25),
            ("noisiness", 0.25),
        ],
        "rationale": "Inverts V6 emphasis to test whether dark+noisy carry more 'intensity' signal than the literal aggression vocabulary. Useful comparison: if V6 and V7 rank similarly, the underlying axes are correlated; if they diverge, the weighting matters.",
    },

    # ── V8-V13: V6/V7 lineage with explicit distortion + atonal axis ────────
    # First eval round picked V6/V7 as best — both blend multiple sub-axes
    # but neither weights distortion enough to dominate, and neither has
    # the atonal↔melodic distinction (which is the cleanest semantic split
    # between hard techno/neuro DnB and melodic techno/deep house).

    "V8_v7_with_distortion_bump": {
        "name": "V7 with distortion bumped 0.2→0.35",
        "components": [
            ("aggression", 0.15),
            ("distortion", 0.35),
            ("density", 0.05),
            ("darkness", 0.20),
            ("noisiness", 0.25),
        ],
        "rationale": "V7 already wins Spearman; user feels distortion is underweighted in top results. Bump distortion to be the largest single contributor while keeping V7's dark+noisy character. Tests whether more distortion sharpens the top-end ranking.",
    },

    "V9_v6_with_atonal": {
        "name": "V6 with atonal added (0.4/0.2/0.15/0.1/0.05/0.1)",
        "components": [
            ("aggression", 0.4),
            ("distortion", 0.2),
            ("density", 0.15),
            ("darkness", 0.10),
            ("noisiness", 0.05),
            ("atonality", 0.10),
        ],
        "rationale": "V6 is the most-balanced top performer. Add the atonal axis at 0.10 to give percussive/non-melodic tracks (industrial techno, neuro) a small additional boost. Aggression stays dominant.",
    },

    "V10_balanced_six_axis": {
        "name": "Balanced six-axis (0.25/0.20/0.15/0.10/0.10/0.20)",
        "components": [
            ("aggression", 0.25),
            ("distortion", 0.20),
            ("density", 0.15),
            ("darkness", 0.10),
            ("noisiness", 0.10),
            ("atonality", 0.20),
        ],
        "rationale": "All 6 sub-axes contribute meaningfully, with aggression + atonality the top two. Tests the hypothesis that the cleanest 'intensity' signal is 'aggressive AND non-melodic' (industrial techno gold standard).",
    },

    "V11_neuro_dnb_tuned": {
        "name": "Neuro/DnB-tuned (high aggr + dist + noisy + atonal)",
        "components": [
            ("aggression", 0.30),
            ("distortion", 0.25),
            ("density", 0.05),
            ("darkness", 0.10),
            ("noisiness", 0.20),
            ("atonality", 0.10),
        ],
        "rationale": "Tuned for neurofunk DnB / dark techstep specifically. Aggression + distortion + noisiness all prominent, density downweighted (neuro is rhythmically dense but tracks are also often quite spare in the breakdown).",
    },

    "V12_peak_techno_tuned": {
        "name": "Peak-time-techno-tuned (aggr + dark + atonal, low noisy)",
        "components": [
            ("aggression", 0.35),
            ("distortion", 0.15),
            ("density", 0.10),
            ("darkness", 0.20),
            ("noisiness", 0.05),
            ("atonality", 0.15),
        ],
        "rationale": "Tuned for peak-time / hard techno — driving + dark + atonal, low noisiness (peak techno isn't noisy, just LOUD and percussive). Tests whether downweighting noisiness recovers Charlotte De Witte / peak-time tracks that V7 underranked.",
    },

    "V13_distortion_atonal_dominant": {
        "name": "Distortion + atonal dominant (testing user hypothesis)",
        "components": [
            ("aggression", 0.10),
            ("distortion", 0.35),
            ("density", 0.05),
            ("darkness", 0.10),
            ("noisiness", 0.10),
            ("atonality", 0.30),
        ],
        "rationale": "Stress-test: if 'intensity' is best captured by 'sounds distorted AND sounds atonal/percussive', this should win. Aggression demoted to a tiebreaker. Likely overshoots and breaks peak-time techno (which is aggressive but not heavily distorted), but the comparison point is informative.",
    },
}


def l2_normalize(v: torch.Tensor) -> torch.Tensor:
    norm = v.norm()
    if norm < 1e-9:
        raise ValueError("zero-vector cannot be normalised")
    return v / norm


def derive_sub_axis(mulan, name: str, positive: list[str], negative: list[str]) -> dict:
    """Run the text tower, return the unit-norm polar axis + per-prompt vectors for provenance."""
    with torch.no_grad():
        pos_vecs = mulan(texts=positive)  # (N_pos, 512), already l2-normed
        neg_vecs = mulan(texts=negative)

    mean_pos = pos_vecs.mean(dim=0)
    mean_neg = neg_vecs.mean(dim=0)
    raw_axis = mean_pos - mean_neg
    axis = l2_normalize(raw_axis)

    return {
        "name": name,
        "axis_vec": axis.cpu().tolist(),
        "prompts_positive": positive,
        "prompts_negative": negative,
        "n_positive": len(positive),
        "n_negative": len(negative),
    }


def assemble_variant(variant_id: str, spec: dict, sub_axis_cache: dict) -> dict:
    """Combine sub-axes per the variant's component weights into a single intensity vector."""
    components = spec["components"]
    accumulator = torch.zeros(512)
    sub_axes_used = []

    for sub_name, weight in components:
        if sub_name not in sub_axis_cache:
            raise KeyError(f"sub-axis '{sub_name}' missing from cache")
        sub = sub_axis_cache[sub_name]
        sub_vec = torch.tensor(sub["axis_vec"])
        accumulator = accumulator + weight * sub_vec
        sub_with_weight = dict(sub)
        sub_with_weight["weight_in_intensity"] = weight
        sub_axes_used.append(sub_with_weight)

    intensity_axis = l2_normalize(accumulator)

    formula_parts = [f"{w:.2f} * sub[{n}]" for n, w in components]
    formula = " + ".join(formula_parts)
    formula = f"l2_normalize({formula})"

    return {
        "variant_id": variant_id,
        "name": spec["name"],
        "rationale": spec["rationale"],
        "model": "OpenMuQ/MuQ-MuLan-large",
        "embedding_dim": 512,
        "method": "polar-prompt-difference, weighted-sum, l2-normalised",
        "intensity_formula": formula,
        "intensity_axis_vec": intensity_axis.cpu().tolist(),
        "sub_axes": sub_axes_used,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def main() -> int:
    output_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("./models/aggression-axes")
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"[derive] output → {output_dir}")
    print("[derive] loading MuQMuLan from HF cache...")
    t0 = time.time()
    try:
        from muq import MuQMuLan
    except ImportError as e:
        print(f"[derive] ERROR: muq lib not installed: {e}", file=sys.stderr)
        return 2

    mulan = MuQMuLan.from_pretrained("OpenMuQ/MuQ-MuLan-large").eval()
    if torch.cuda.is_available():
        mulan = mulan.cuda()
        print(f"[derive] cuda: {torch.cuda.get_device_name()}")
    else:
        print("[derive] running on CPU (text tower is small)")
    print(f"[derive] loaded in {time.time() - t0:.1f}s")

    # Step 1: derive every sub-axis once. Each variant references these by name.
    print(f"\n[derive] computing {len(SUB_AXES)} sub-axes...")
    sub_axis_cache = {}
    for sub_name, (positive, negative) in SUB_AXES.items():
        t0 = time.time()
        sub = derive_sub_axis(mulan, sub_name, positive, negative)
        sub_axis_cache[sub_name] = sub
        # Sanity print: a few component magnitudes of the axis
        axis_t = torch.tensor(sub["axis_vec"])
        print(
            f"  [{sub_name:11s}] {len(positive)}+ / {len(negative)}-  "
            f"|axis| = {axis_t.norm().item():.3f}  "
            f"max|c| = {axis_t.abs().max().item():.4f}  "
            f"({time.time() - t0:.1f}s)"
        )

    # Step 2: assemble each variant by combining sub-axes.
    print(f"\n[derive] assembling {len(VARIANTS)} variants...")
    for variant_id, spec in VARIANTS.items():
        variant = assemble_variant(variant_id, spec, sub_axis_cache)
        out_path = output_dir / f"{variant_id}.json"
        with open(out_path, "w") as f:
            json.dump(variant, f, indent=2)
        intensity_t = torch.tensor(variant["intensity_axis_vec"])
        print(
            f"  [{variant_id:25s}] {variant['intensity_formula']:60s}  "
            f"|i| = {intensity_t.norm().item():.3f}  → {out_path}"
        )

    # Step 3: cross-variant similarity matrix — confirms variants disagree.
    print("\n[derive] cross-variant cosine similarity (low values = variants disagree, high = redundant):")
    variant_ids = list(VARIANTS.keys())
    variant_axes = {}
    for vid in variant_ids:
        with open(output_dir / f"{vid}.json") as f:
            data = json.load(f)
            variant_axes[vid] = torch.tensor(data["intensity_axis_vec"])

    header = " " * 28 + "  ".join(f"{vid[:8]:>8s}" for vid in variant_ids)
    print(header)
    for vid_a in variant_ids:
        row_vals = []
        for vid_b in variant_ids:
            sim = float(torch.dot(variant_axes[vid_a], variant_axes[vid_b]))
            row_vals.append(f"{sim:+.3f}")
        print(f"  {vid_a:25s} " + "    ".join(f"{v:>6s}" for v in row_vals))

    print(f"\n[derive] done — {len(VARIANTS)} variants written to {output_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
