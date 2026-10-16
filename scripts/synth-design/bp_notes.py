"""basic-pitch note events of an audio excerpt, as JSON on stdout:
[[onset_s, duration_s, midi, amplitude], ...] (onsets relative to START).
Usage: bp_notes.py WAV START_S DURATION_S   (runs in ~/.cache/mesh-transcribe/venv)
"""
import json, sys, tempfile, os
import numpy as np, soundfile as sf
from basic_pitch.inference import predict
from basic_pitch import ICASSP_2022_MODEL_PATH

wav, start, dur = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
info = sf.info(wav)
x, sr = sf.read(wav, start=int(start * info.samplerate), frames=int(dur * info.samplerate), always_2d=True)
with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as t:
    sf.write(t.name, x.mean(1), sr)
    _, _, events = predict(t.name, ICASSP_2022_MODEL_PATH, onset_threshold=0.5, frame_threshold=0.3,
                           minimum_note_length=40, minimum_frequency=40, maximum_frequency=1500)
    os.unlink(t.name)
print(json.dumps([[float(s), float(e - s), int(p), float(a)] for s, e, p, a, *_ in events]))
