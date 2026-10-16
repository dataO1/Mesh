"""Ask Music Flamingo to listen to 30 s clips with a critical producer prompt.
Usage: listen_compare.py FILE@START_S[:LABEL]... (server on :8001)
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "live-analysis"))
import listen as L  # noqa: E402  (reuses decode, request)

PROMPT = """You are a strict drum and bass producer and mix engineer giving honest feedback on this 30 second excerpt. Be concrete and critical; do not be polite.
1. LEAD SOUND: describe timbre, thickness/weight, register, distortion, width. Does it sound thin or full, cheap or professional?
2. MELODY: describe the notes and rhythm of the riff. Does it sound energetic and dark, or childish/playful? Name what makes it so.
3. GROOVE: how do lead and bass sit against the kick and snare?
4. VERDICT: would this work in a Pendulum-style drop? What are the three most important changes?"""

L.PROMPT = PROMPT
for arg in sys.argv[1:]:
    path, rest = arg.split("@")
    start, _, label = rest.partition(":")
    print(f"==== {label or os.path.basename(path)} @ {start}s")
    print(L.ask(L.ffdecode(path, float(start), 30.0), L.URL))
