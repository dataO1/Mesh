"""Lead notes of an audio excerpt by predominant-melody extraction (Melodia,
essentia), quantized to 16th steps. JSON on stdout:
[[onset_s, duration_s, midi, 1.0], ...] (onsets relative to START).
Usage: mel_notes.py WAV START_S DURATION_S STEP_S  (runs in ~/.cache/mesh-transcribe/venv)
A note starts where the step pitch changes or where the stem re-attacks
(spectral-flux onset on the step) on the same pitch; unvoiced steps rest.
"""
import json, sys
import numpy as np
import essentia.standard as es

wav, start, dur, step = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4])
sr = 44100
audio = es.MonoLoader(filename=wav, sampleRate=sr)()
x = audio[int(start * sr):int((start + dur) * sr)]
x = es.EqualLoudness()(x)
hop = 128
f0, conf = es.PredominantPitchMelodia(frameSize=2048, hopSize=hop, minFrequency=110, maxFrequency=1760,
                                      voicingTolerance=0.2, guessUnvoiced=False)(x)
t = np.arange(len(f0)) * hop / sr
# Onset strength (spectral flux) per frame, for re-attacks on the same pitch.
w, spec, od = es.Windowing(type="hann"), es.Spectrum(), es.OnsetDetection(method="flux")
flux = np.array([od(spec(w(fr)), np.zeros(1025)) for fr in es.FrameGenerator(x, frameSize=2048, hopSize=hop)])
flux = flux[:len(f0)] if len(flux) >= len(f0) else np.pad(flux, (0, len(f0) - len(flux)))
thr = np.percentile(flux, 85)
n = int(round(dur / step))
steps = []
for k in range(n):
    sel = (t >= k * step) & (t < (k + 1) * step)
    v = f0[sel]
    v = v[v > 0]
    if len(v) < 0.4 * sel.sum():
        steps.append(None)
        continue
    midi = 69 + 12 * np.log2(np.median(v) / 440.0)
    attack = flux[sel][: max(1, sel.sum() // 3)].max() > thr
    steps.append((int(round(midi)), attack))
notes = []
for k, s in enumerate(steps):
    if s is None:
        continue
    midi, attack = s
    prev = steps[k - 1] if k > 0 else None
    if notes and prev is not None and prev[0] == midi and not attack:
        notes[-1][1] += step
    else:
        notes.append([k * step, step, midi, 1.0])
print(json.dumps(notes))
