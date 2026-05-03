"""Grade every track in the mesh DB on intensity using NVIDIA Audio Flamingo 3.

Per track:
  1. Read drop_marker + path + sample_rate from mesh DB
  2. Decode 30 s of audio centered around the drop (drop_marker - 5 s),
     fallback: 33% mark of total length if no drop_marker stored
  3. Resample to 16 kHz mono
  4. Run AF3 with a JSON-mode prompt asking for intensity (0-10),
     sub-axis scores, genre guess, structural position, justification
  5. Persist raw response as <out_dir>/<track_id>.json (resumable)

After all tracks processed:
  - Aggregate into <out_dir>/llm-grading-raw.jsonl (one row per track)
  - Derive <out_dir>/llm-priors.csv with columns track_id|name|prior
    (compatible with scripts/compare-variants.py)

Run:
  python grade.py [--limit N] [--resume] [--out-dir DIR] [--db PATH]
                  [--collection PATH] [--reanalyze]

Defaults:
  --collection ~/Music/mesh-collection
  --out-dir    /tmp/track-grading

Resumability: skips tracks whose <out_dir>/<track_id>.json already exists.
Use --reanalyze to force re-running everything.
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import re
import sqlite3
import sys
import time
from pathlib import Path

import torch
import numpy as np
import soundfile as sf
import librosa


CLIP_SECS = 30
TARGET_SR = 16_000
MODEL_NAME = "nvidia/audio-flamingo-3-hf"


PROMPT = """\
Listen to this 30-second clip from a DJ's track library and rate its overall intensity on a 0-10 scale.

Intensity scale:
  0-1 = ambient drone, almost no rhythm or energy
  2-3 = downtempo, deep house, liquid drum-and-bass — calm, melodic, low energy
  4-5 = mid-energy melodic techno, warm grooves, dancefloor minimal
  6-7 = peak-time but still musical, energetic dancefloor DnB
  8 = hard / aggressive — peak-time hard techno, neurofunk DnB, distorted heavy
  9 = brutal — peak neurofunk, gabber, harsh industrial
  10 = noise wall, extreme power electronics

Judge based on what you actually hear: kick weight, distortion, layering density, harshness vs warmth, melody vs no melody. Different tracks should get different scores — distribute across the 0-10 range.

Respond with EXACTLY this single-line format and nothing else:

INTENSITY: <integer 0-10> | GENRE: <few words> | NOTES: <one short sentence describing what you actually hear>
"""


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--collection", type=Path,
                   default=Path.home() / "Music" / "mesh-collection")
    p.add_argument("--out-dir", type=Path, default=Path("/tmp/track-grading"))
    p.add_argument("--limit", type=int, default=None)
    p.add_argument("--resume", action="store_true",
                   help="(default behavior — skip tracks already graded)")
    p.add_argument("--reanalyze", action="store_true",
                   help="Force re-grading even if <track_id>.json exists")
    p.add_argument("--track-ids", type=Path, default=None,
                   help="Optional: file with one track_id per line (or first column of CSV)")
    return p.parse_args()


def load_track_list(args) -> list[tuple[int, str, int | None, int | None]]:
    """Read (track_id, path, drop_marker, frame_count) from mesh DB."""
    db_path = args.collection / "mesh.db"
    if not db_path.exists():
        sys.exit(f"[grade] DB not found: {db_path}")

    only_ids: set[int] | None = None
    if args.track_ids:
        only_ids = set()
        for line in args.track_ids.read_text().splitlines():
            first = line.split(",")[0].strip()
            try:
                only_ids.add(int(first))
            except ValueError:
                pass
        print(f"[grade] only-ids filter: {len(only_ids)} tracks")

    # Cozo embedded SQLite has no Python driver per se — but mesh DBs are
    # actually rocksdb / sqlite-fronted. We use the mesh-cue cargo binaries
    # to extract instead. Use the existing get_all_tracks via a Rust helper
    # we'll call from the bash wrapper.
    # For this script we just shell out and read /tmp/grade-tracks-list.csv
    # produced by the Rust side (see grade-tracks.nix).
    list_csv = args.out_dir / "_track-list.csv"
    if not list_csv.exists():
        sys.exit(f"[grade] track list not found at {list_csv}; "
                 "the wrapper script should produce it via reanalyze_ml --dump")
    tracks = []
    with list_csv.open() as f:
        r = csv.DictReader(f)
        for row in r:
            tid = int(row["track_id"])
            if only_ids is not None and tid not in only_ids:
                continue
            tracks.append((
                tid,
                row["path"],
                int(row["drop_marker"]) if row["drop_marker"] else None,
                int(row["frame_count"]) if row.get("frame_count") else None,
            ))
    if args.limit:
        tracks = tracks[: args.limit]
    return tracks


def load_audio_window(path: str, drop_marker: int | None,
                      frame_count: int | None) -> np.ndarray | None:
    """Decode 30 s of audio at 16 kHz mono, centered ~drop_marker.

    drop_marker is sample-position in the original (likely 48 kHz) FLAC.
    We start the window 5 s before the drop so the captioner hears intro→drop.
    """
    try:
        info = sf.info(path)
        native_sr = info.samplerate
        total_frames = info.frames
    except Exception as e:
        print(f"[grade] sf.info failed for {path}: {e}", file=sys.stderr)
        return None

    if drop_marker is not None and drop_marker > 0:
        start_native = max(0, drop_marker - native_sr * 5)
    else:
        # fallback: 33 % into the track (matches existing drop estimator default)
        start_native = total_frames // 3

    duration_native = native_sr * CLIP_SECS
    start_native = min(start_native, max(0, total_frames - duration_native))

    try:
        audio, sr = sf.read(
            path, start=start_native, frames=duration_native,
            dtype="float32", always_2d=False,
        )
    except Exception as e:
        print(f"[grade] sf.read failed for {path}: {e}", file=sys.stderr)
        return None

    # Mono mix
    if audio.ndim > 1:
        # multi-channel (stem-split FLAC has 8 channels) → average to mono
        audio = audio.mean(axis=1)

    # Resample to 16 kHz if needed
    if sr != TARGET_SR:
        audio = librosa.resample(audio, orig_sr=sr, target_sr=TARGET_SR,
                                 res_type="soxr_hq")

    # Pad to exactly 30 s if the file was shorter
    target_len = TARGET_SR * CLIP_SECS
    if len(audio) < target_len:
        audio = np.pad(audio, (0, target_len - len(audio)))
    elif len(audio) > target_len:
        audio = audio[:target_len]

    return audio.astype(np.float32)


def extract_json(text: str) -> dict | None:
    """Parse AF3's flat-text 'INTENSITY: N | GENRE: ... | NOTES: ...' format.

    AF3 doesn't reliably emit nested JSON — the prior schema collapsed into
    a positional array with intensity always pinned at 0.5. The flat-text
    format is reliably differentiating across tracks (verified in smoke
    tests).
    """
    if not text:
        return None
    parsed: dict = {}
    # Intensity: number 0-10 (allow decimal for safety, snap to int)
    m = re.search(r'INTENSITY\s*[:=]\s*(\d+(?:\.\d+)?)', text, re.IGNORECASE)
    if m:
        val = float(m.group(1))
        # If model reports 0-1 scale, scale up to 0-10
        if val <= 1.5 and 'intensity' not in text[m.end():m.end()+200].lower():
            # heuristic: if it's <=1.5 and the rest of the text doesn't
            # mention "low intensity" we don't auto-rescale; leave the
            # judgment to the reader. Just floor at 0.
            pass
        parsed["intensity"] = int(round(min(10, max(0, val))))
    g = re.search(r'GENRE\s*[:=]\s*([^|\n]+?)(?:\s*\||\n|$)', text, re.IGNORECASE)
    if g:
        parsed["genre_guess"] = g.group(1).strip()
    n = re.search(r'NOTES?\s*[:=]\s*(.+?)(?:\n|$)', text, re.IGNORECASE | re.DOTALL)
    if n:
        notes = n.group(1).strip()
        # Trim if absurdly long
        if len(notes) > 400:
            notes = notes[:400] + "..."
        parsed["notes"] = notes
    if "intensity" in parsed:
        return parsed
    return None


def derive_aggregate(out_dir: Path):
    """After per-track JSONs are written, build llm-grading-raw.jsonl + llm-priors.csv."""
    rows = []
    for jp in sorted(out_dir.glob("*.json")):
        if jp.name.startswith("_"):
            continue
        try:
            data = json.loads(jp.read_text())
            rows.append(data)
        except Exception as e:
            print(f"[aggregate] failed {jp}: {e}", file=sys.stderr)

    raw_jsonl = out_dir / "llm-grading-raw.jsonl"
    with raw_jsonl.open("w") as f:
        for row in rows:
            f.write(json.dumps(row) + "\n")
    print(f"[aggregate] wrote {raw_jsonl} ({len(rows)} rows)")

    priors_csv = out_dir / "llm-priors.csv"
    # Format: track_id|name|prior   (compatible with compare-variants.py)
    n_written = 0
    with priors_csv.open("w") as f:
        for row in rows:
            llm = row.get("llm_response", {})
            intensity = llm.get("intensity")
            if intensity is None:
                continue
            tid = row["track_id"]
            title = row.get("title", "?")
            artist = row.get("artist", "")
            name = f"{title} - {artist}" if artist else title
            # Sanitize | from name
            name = name.replace("|", "/")
            f.write(f"{tid}|{name}|{intensity}\n")
            n_written += 1
    print(f"[aggregate] wrote {priors_csv} ({n_written} rows)")


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    tracks = load_track_list(args)
    print(f"[grade] {len(tracks)} tracks queued")

    # Filter out already-done unless --reanalyze
    pending = []
    for entry in tracks:
        tid = entry[0]
        if not args.reanalyze and (args.out_dir / f"{tid}.json").exists():
            continue
        pending.append(entry)
    print(f"[grade] {len(pending)} tracks pending after resume filter")

    if not pending:
        print("[grade] nothing to do; running aggregation only")
        derive_aggregate(args.out_dir)
        return 0

    # Lazy-load model so listing/aggregation paths don't pay this cost
    print(f"[grade] loading {MODEL_NAME} on {torch.cuda.get_device_name(0) if torch.cuda.is_available() else 'CPU'}...")
    t0 = time.time()
    from transformers import AutoProcessor
    # AF3 uses a custom multi-modal class, not AutoModelForCausalLM
    try:
        from transformers import AudioFlamingo3ForConditionalGeneration as AF3Model
    except ImportError:
        # Older transformers — try AutoModel which dispatches via the model's
        # registered config class
        from transformers import AutoModel as AF3Model
    processor = AutoProcessor.from_pretrained(MODEL_NAME, trust_remote_code=True)
    model = AF3Model.from_pretrained(
        MODEL_NAME, trust_remote_code=True,
        torch_dtype=torch.bfloat16,
        device_map="cuda",
    ).eval()
    print(f"[grade] model loaded in {time.time() - t0:.1f}s")

    n_done = 0
    n_failed = 0
    start = time.time()
    for tid, path, drop_marker, _frame_count in pending:
        out_path = args.out_dir / f"{tid}.json"
        track_t0 = time.time()
        audio = load_audio_window(path, drop_marker, None)
        if audio is None:
            print(f"[grade] {tid}: audio decode failed — skip", file=sys.stderr)
            n_failed += 1
            n_done += 1
            continue

        # AF3 chat format — audio goes INSIDE the content dict as
        # {"type": "audio", "audio": <numpy array at 16 kHz>}, not as a
        # separate `audios=` kwarg. (Earlier the audio was silently dropped
        # and the model produced identical responses for every track.)
        try:
            messages = [
                {"role": "user", "content": [
                    {"type": "text", "text": PROMPT},
                    {"type": "audio", "audio": audio},
                ]},
            ]
            inputs = processor.apply_chat_template(
                messages,
                add_generation_prompt=True, tokenize=True,
                return_tensors="pt", return_dict=True,
            ).to("cuda")
            # AF3's audio encoder expects the same dtype as the model weights;
            # the feature extractor returns float32 by default, so cast.
            if "input_features" in inputs:
                inputs["input_features"] = inputs["input_features"].to(torch.bfloat16)
            with torch.no_grad():
                out_ids = model.generate(
                    **inputs,
                    max_new_tokens=200,
                    do_sample=True,
                    temperature=0.3,
                    top_p=0.9,
                    pad_token_id=processor.tokenizer.eos_token_id,
                )
            # Strip the prompt portion
            generated = out_ids[0, inputs["input_ids"].shape[1]:]
            response_text = processor.tokenizer.decode(generated, skip_special_tokens=True)
        except Exception as e:
            print(f"[grade] {tid}: inference failed — {e}", file=sys.stderr)
            n_failed += 1
            n_done += 1
            continue

        parsed = extract_json(response_text)
        # Look up display name from the CSV cache (already loaded)
        track_csv = args.out_dir / "_track-list.csv"
        title = ""
        artist = ""
        if track_csv.exists():
            with track_csv.open() as f:
                for row in csv.DictReader(f):
                    if int(row["track_id"]) == tid:
                        title = row.get("title", "")
                        artist = row.get("artist", "")
                        break

        record = {
            "track_id": tid,
            "title": title,
            "artist": artist,
            "path": path,
            "drop_marker": drop_marker,
            "model": MODEL_NAME,
            "raw_response": response_text,
            "llm_response": parsed if parsed else {"_parse_error": True},
            "wall_time_s": round(time.time() - track_t0, 2),
            "ts": int(time.time()),
        }
        out_path.write_text(json.dumps(record, indent=2))
        n_done += 1

        if n_done % 5 == 0 or n_done == len(pending):
            elapsed = time.time() - start
            rate = n_done / max(elapsed, 0.001)
            eta = (len(pending) - n_done) / max(rate, 0.001)
            intensity_field = parsed.get("intensity") if parsed else "?"
            print(f"[grade] {n_done}/{len(pending)} done "
                  f"({rate:.2f}/s, eta {eta:.0f}s, failed {n_failed}, "
                  f"last={tid} intensity={intensity_field})")

    print(f"[grade] complete: {n_done - n_failed} succeeded, {n_failed} failed, "
          f"{time.time() - start:.0f}s total")

    derive_aggregate(args.out_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
