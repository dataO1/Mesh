"""Music Flamingo listening pass: 30 s clips per track, five questions each.
Usage: listen.py [--url URL] [--workers N] SPEC...
SPEC is SET:N:CLIPS (first N tracks of a set from the ref JSONs, CLIPS one of
drop2|drop3) or FILE:CLIPS for a single audio file with a ref JSON.
Clips: drop = first 30 s of the main drop, late = 64 bars later (or the last
drop section), pre = the 8 bars before the main drop. Writes
$REF/listening/<set>/<track>.json and prints progress.
"""
import base64
import concurrent.futures as cf
import glob
import io
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
import requests
import soundfile as sf

import _common as C

URL = "http://localhost:8001/v1/chat/completions"
MODEL = "nvidia/music-flamingo-2601-hf"
SR = 16000
CLIP_S = 30.0
PROMPT = """You are a drum and bass producer reviewing this 30 second excerpt. Answer each point in 1-3 sentences, concretely, about what you hear:
1. LEAD: describe the main melodic/lead sound(s): timbre, synthesis character, register, layering (single line, stacked, chords, pads).
2. MELODY: describe the melody or riff: how long is the repeating phrase, does it repeat or vary, is there call and response between bass and lead or between instruments?
3. ARRANGEMENT: which elements play, which enter or drop out during the excerpt, and what changes over time (fills, risers, filter sweeps, drops, cuts).
4. MIX: balance of kick, snare, bass, lead; brightness, width, loudness, compression character.
5. ENERGY: what makes this excerpt exciting or boring, and what is the single most important production technique you hear?"""


def ffdecode(path, start, dur):
    cmd = ["ffmpeg", "-v", "quiet", "-ss", f"{max(start, 0):.2f}", "-t", f"{dur:.2f}", "-i", path,
           "-ac", "1", "-ar", str(SR), "-f", "f32le", "-"]
    env = {k: v for k, v in os.environ.items() if k != "LD_LIBRARY_PATH"}
    raw = subprocess.run(cmd, capture_output=True, check=True, env=env).stdout
    return np.frombuffer(raw, dtype=np.float32)


def b64wav(a):
    buf = io.BytesIO()
    sf.write(buf, a, SR, format="WAV", subtype="PCM_16")
    return base64.b64encode(buf.getvalue()).decode()


def ask(audio, url):
    payload = {
        "model": MODEL,
        "messages": [{"role": "user", "content": [
            {"type": "input_audio", "input_audio": {"data": b64wav(audio), "format": "wav"}},
            {"type": "text", "text": PROMPT}]}],
        "max_tokens": 700, "temperature": 0.7, "top_p": 0.9,
    }
    r = requests.post(url, json=payload, timeout=600)
    r.raise_for_status()
    return r.json()["choices"][0]["message"]["content"]


def clips_for(ref, kind):
    bar = 240.0 / ref["metrics"]["tempo"]
    phase = ref.get("beat_phase_s", 0.0) or 0.0
    runtime = ref["metrics"]["duration_s"]
    drop = ref.get("main_drop_start_s")
    if drop is None or drop < 1:
        drop = runtime * 0.1
    out = {"drop": drop, "pre": drop - 8 * bar}
    late = drop + 64 * bar
    drops = [s for s in ref.get("structure", []) if s["label"] == "drop"]
    if late + CLIP_S > runtime and drops:
        late = phase + drops[-1]["start_bar"] * bar
    out["late"] = min(late, runtime - CLIP_S)
    names = ["drop", "late"] if kind == "drop2" else ["pre", "drop", "late"]
    return {n: out[n] for n in names if out[n] >= 0}


def resolve(spec):
    path, kind = spec.rsplit(":", 1)
    if os.path.isfile(path):
        s = C.set_of(path) if hasattr(C, "set_of") else os.path.basename(os.path.dirname(path))
        s = "mesh" if s == "mesh-recordings" else s
        base = os.path.splitext(os.path.basename(path))[0]
        return [(s, base, path, os.path.join(C.REF_ROOT, "analysis", s, base + ".json"), kind)]
    s, n = path.split(":") if ":" in path else (path, "999")
    refs = sorted(p for p in glob.glob(os.path.join(C.REF_ROOT, "analysis", s, "*.json"))
                  if p.count(".") == 1 or not any(t in os.path.basename(p) for t in (".develop.", ".melody")))
    refs = [p for p in refs if not os.path.basename(p)[:-5].endswith((".develop", ".melody-bass", ".melody-other"))]
    out = []
    for p in refs[: int(n)]:
        ref = json.load(open(p))
        out.append((s, os.path.basename(p)[:-5], ref["file"], p, kind))
    return out


def main():
    args, url, workers = sys.argv[1:], URL, 8
    while args and args[0].startswith("--"):
        if args[0] == "--url": url, args = args[1], args[2:]
        elif args[0] == "--workers": workers, args = int(args[1]), args[2:]
    jobs = []
    for spec in args:
        for s, base, audio, refp, kind in resolve(spec):
            out = os.path.join(C.REF_ROOT, "listening", s, base + ".json")
            if os.path.exists(out):
                continue
            ref = json.load(open(refp))
            for name, start in clips_for(ref, kind).items():
                jobs.append((s, base, audio, out, name, start))
    print(f"{len(jobs)} clips", file=sys.stderr)
    results = {}

    def run(j):
        s, base, audio, out, name, start = j
        return j, ask(ffdecode(audio, start, CLIP_S), url)

    with cf.ThreadPoolExecutor(workers) as ex:
        for i, fut in enumerate(cf.as_completed([ex.submit(run, j) for j in jobs])):
            try:
                (s, base, audio, out, name, start), text = fut.result()
            except Exception as e:
                print(f"fail: {e}", file=sys.stderr); continue
            r = results.setdefault(out, {"set": s, "track": base, "file": audio, "clips": {}})
            r["clips"][name] = {"start_s": round(start, 2), "answer": text}
            print(f"[{i + 1}/{len(jobs)}] {s}/{base} {name}", file=sys.stderr)
    for out, r in results.items():
        os.makedirs(os.path.dirname(out), exist_ok=True)
        json.dump(r, open(out, "w"), indent=1)


if __name__ == "__main__":
    main()
