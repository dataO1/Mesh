"""Stage S3 — Caption structured-tag extraction.

Per-track multi-hot tags derived from MF caption text via case-insensitive
substring matching against a static keyword dictionary. Plus per-tag mention
counts. Output schema per the spec § 9.

The extracted tags (~50d) are a teacher-side feature only; never used as
labels and never read at student inference.

Usage:
    bash spike/track-grading/run_r7_step.sh extract_caption_tags.py \\
         --captions-root /home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo \\
         --out /home/data01/Music/mesh-track-grading/round7_6_caption_struct.npz
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import numpy as np


# ── Keyword dictionary (~50 tags). Extend cautiously: every new tag must
# either help the teacher or stay quiet on > 5 % of tracks. ────────────
TAGS: dict[str, list[str]] = {
    # Drums / percussion
    "instr_kick": ["kick drum", "kick", "four-on-the-floor", "four on the floor"],
    "instr_808": ["808"],
    "instr_snare": ["snare"],
    "instr_hihat": ["hi-hat", "hihat", "hi hat"],
    "instr_clap": ["clap"],
    "instr_breakbeat": ["breakbeat", "amen break", "broken beat"],
    "instr_drum_machine": ["drum machine", "drum-machine"],
    "instr_acoustic_drums": ["acoustic drum", "drum kit"],
    # Bass
    "instr_synth_bass": ["synth bass", "bass synth", "sub bass", "sub-bass", "subbass"],
    "instr_reese_bass": ["reese", "reese bass", "neuro bass", "neurobass"],
    "instr_acoustic_bass": ["acoustic bass", "double bass"],
    "instr_electric_bass": ["electric bass", "bass guitar"],
    # Strings / acoustic
    "instr_acoustic_guitar": ["acoustic guitar", "nylon-string", "fingerstyle"],
    "instr_electric_guitar": ["electric guitar", "power chord", "power-chord"],
    "instr_distorted_guitar": ["distorted guitar", "fuzz guitar"],
    "instr_piano": ["piano"],
    "instr_strings": ["string section", "violin", "cello", "orchestral string"],
    # Synths
    "instr_pad": ["synth pad", "pad", "atmospheric pad"],
    "instr_lead_synth": ["lead synth", "synth lead"],
    "instr_arp": ["arpeggi", "arp pattern"],
    "instr_drone": ["drone", "drones", "sustained drone"],
    # World / brass
    "instr_brass": ["brass section", "trumpet", "trombone", "saxophone"],
    "instr_world_perc": ["conga", "timbale", "bongo", "tabla", "djembe"],
    # Vocals
    "vocal_none": ["no vocals", "instrumental", "without vocals"],
    "vocal_clean_male": ["male vocal", "male singer", "male baritone", "male tenor"],
    "vocal_clean_female": ["female vocal", "female singer", "female mezzo", "female soprano"],
    "vocal_aggressive": ["shouted", "screamed", "growled", "harsh vocal", "screaming"],
    "vocal_rapping": ["rap vocals", "rap delivery", "rapper", "rapping"],
    "vocal_choir": ["choir", "choral"],
    "vocal_processed": ["auto-tune", "autotune", "vocoder", "heavily processed vocal"],
    # Mood
    "mood_dark": ["dark", "ominous", "menacing", "brooding", "sinister"],
    "mood_bright": ["bright", "uplifting", "joyful", "sparkling", "cheerful"],
    "mood_aggressive": ["aggressive", "abrasive", "raw and gritty", "in-your-face"],
    "mood_melancholic": ["melancholic", "wistful", "introspective", "contemplative"],
    "mood_euphoric": ["euphoric", "anthemic", "ecstatic"],
    # Production
    "mix_polished": ["polished", "high-fidelity", "hi-fi", "clean mix", "transparent mix"],
    "mix_lofi": ["lo-fi", "lofi", "raw", "dusty", "tape-warm", "unpolished"],
    "mix_compressed": ["heavily compressed", "brick-walled", "loudness war"],
    "mix_wide_stereo": ["wide stereo"],
    "mix_reverb": ["extensive reverb", "cavernous", "spacious reverb"],
    # Structural
    "struct_buildup": ["buildup", "build-up", "rising tension", "anticipation"],
    "struct_drop": ["drop", "the drop", "main drop", "peak section"],
    "struct_breakdown": ["breakdown", "extended breakdown"],
    "struct_intro": ["introduction", "atmospheric introduction", "intro section"],
    "struct_outro": ["outro", "fade-out", "closing", "ending"],
    "struct_continuous": ["continuous", "evolving", "no clear drop"],
    # Tempo / energy descriptors
    "tempo_fast": ["fast tempo", "high tempo", "uptempo", "up-tempo", "frantic", "relentless"],
    "tempo_slow": ["slow tempo", "downtempo", "down-tempo", "slow-paced", "languid"],
    "energy_high": ["high energy", "high-energy", "driving", "club-ready", "peak time"],
    "energy_low": ["low energy", "low-energy", "minimalist", "sparse", "ambient"],
    # Distortion / texture
    "fx_distortion": ["distortion", "distorted", "fuzz", "heavily saturated", "clipped"],
    "fx_noise": ["white noise", "noise FX", "noise sweep", "riser", "metallic noise"],
}


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--captions-root", type=Path, required=True,
                   help="dir containing <track_id>.json caption files")
    p.add_argument("--out", type=Path, required=True)
    return p.parse_args()


def main(args) -> int:
    files = sorted(args.captions_root.glob("*.json"))
    if not files:
        print(f"no caption files at {args.captions_root}", file=sys.stderr)
        return 1
    print(f"[struct] reading {len(files)} captions")

    tag_names = list(TAGS.keys())
    T = len(tag_names)
    print(f"[struct] {T} tags")

    # Pre-compile keyword regexes (case-insensitive whole-word-ish; allow
    # punctuation around the keyword by using \b plus the literal escape).
    compiled: dict[str, list[re.Pattern]] = {}
    for tag, kws in TAGS.items():
        compiled[tag] = [re.compile(rf"\b{re.escape(k)}\b", re.IGNORECASE) for k in kws]

    track_ids: list[int] = []
    present_rows: list[np.ndarray] = []
    count_rows: list[np.ndarray] = []

    for f in files:
        try:
            rec = json.loads(f.read_text())
        except Exception:
            continue
        tid = int(rec["track_id"])
        cap = rec.get("caption", "") or ""
        present = np.zeros(T, dtype=bool)
        counts = np.zeros(T, dtype=np.int32)
        for j, tag in enumerate(tag_names):
            n = sum(len(rx.findall(cap)) for rx in compiled[tag])
            counts[j] = n
            present[j] = n > 0
        track_ids.append(tid)
        present_rows.append(present)
        count_rows.append(counts)

    track_ids_arr = np.array(track_ids, dtype=np.int64)
    tag_present = np.stack(present_rows, axis=0)
    tag_count = np.stack(count_rows, axis=0)

    # Per-tag prevalence sanity
    rates = tag_present.mean(axis=0)
    print(f"[struct] tag prevalence:")
    for tag, r in zip(tag_names, rates):
        flag = ""
        if r < 0.005:
            flag = "  ⚠ <0.5% (consider removing or rewording)"
        elif r > 0.95:
            flag = "  ⚠ >95% (near-constant; tag is uninformative)"
        print(f"   {tag:25s}  {100*r:5.2f}%{flag}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(
        args.out,
        track_ids=track_ids_arr,
        tag_names=np.array(tag_names, dtype=object),
        tag_present=tag_present,
        tag_count=tag_count,
    )
    print(f"[struct] wrote {args.out} ({args.out.stat().st_size/1e6:.2f} MB)")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
