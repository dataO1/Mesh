"""Symbolic statistics of MIDI riff clips (16th grid), checked against the
riff rules of references/music-theory.md (section 7.2) and the carnival
detectors (section 2.4).
Usage: riff_stats.py CLIP.mid...   (prints a table per clip and the median)
"""
import struct, sys, os
import numpy as np

NAMES = ["1", "b2", "2", "b3", "3", "4", "b5", "5", "b6", "6", "b7", "7"]
LHL_W = [0 if s == 0 else -1 if s == 8 else -2 if s in (4, 12) else -3 if s % 2 == 0 else -4 for s in range(16)]


def read_mid(path):
    b = open(path, "rb").read()
    tpq = struct.unpack(">H", b[12:14])[0]
    i, notes, t, on = 14, [], 0, {}
    while i < len(b):
        if b[i:i + 4] != b"MTrk":
            break
        ln = struct.unpack(">I", b[i + 4:i + 8])[0]
        j, end, t, status = i + 8, i + 8 + ln, 0, 0
        while j < end:
            d = 0
            while True:
                c = b[j]; j += 1; d = (d << 7) | (c & 0x7F)
                if c < 0x80:
                    break
            t += d
            c = b[j]
            if c == 0xFF:
                l = b[j + 2]; j += 3 + l; continue
            if c >= 0x80:
                status = c; j += 1
            kind = status & 0xF0
            if kind in (0x80, 0x90):
                k, v = b[j], b[j + 1]; j += 2
                if kind == 0x90 and v > 0:
                    on[k] = t
                elif k in on:
                    notes.append((on.pop(k), t, k))
            else:
                j += 1 if kind in (0xC0, 0xD0) else 2
        i = end
    step = tpq / 4
    out = sorted((round(s / step), max(1, round((e - s) / step)), k) for s, e, k in notes)
    return out


def tonic_of(notes):
    """Lowest-weighted pitch class by duration on strong steps: the pedal/root."""
    w = np.zeros(12)
    for s, l, k in notes:
        w[k % 12] += l * (2 if s % 4 == 0 else 1)
    return int(np.argmax(w))


def stats(notes):
    bars = max(1, (max(s + l for s, l, k in notes) + 15) // 16)
    steps = [s % 16 for s, l, k in notes]
    ivs = [abs(b[2] - a[2]) for a, b in zip(notes, notes[1:])]
    tonic = tonic_of(notes)
    pcs = [NAMES[(k - tonic) % 12] for s, l, k in notes]
    # rests: longest silence per bar in 16ths
    occ = np.zeros(bars * 16, bool)
    for s, l, k in notes:
        occ[s:s + l] = True
    rest_bars = 0
    for b in range(bars):
        run = best = 0
        for v in occ[b * 16:(b + 1) * 16]:
            run = 0 if v else run + 1; best = max(best, run)
        rest_bars += best >= 2
    lhl = 0
    for i, (s, l, k) in enumerate(notes):
        nxt = notes[i + 1][0] if i + 1 < len(notes) else bars * 16
        for r in range(s + 1, nxt):
            if not occ[r] or r >= s + l:
                if LHL_W[r % 16] > LHL_W[s % 16]:
                    lhl += LHL_W[r % 16] - LHL_W[s % 16]
                break
    reps = {1: 0.0, 2: 0.0}
    grid = {}
    for s, l, k in notes:
        grid[s] = k
    for lag in (1, 2):
        same = sum(1 for s, k in grid.items() if grid.get(s + 16 * lag) == k)
        reps[lag] = same / max(1, sum(1 for s in grid if s + 16 * lag < bars * 16))
    n = len(ivs) or 1
    return {
        "bars": bars,
        "notes/bar": len(notes) / bars,
        "offbeat": np.mean([s % 4 != 0 for s in steps]),
        "downbeat": np.mean([s == 0 for s in steps]),
        "on 4/12": np.mean([s in (4, 12) for s in steps]),
        "pickup 14/15/7/3": np.mean([s in (3, 7, 14, 15) for s in steps]),
        "len>=4": np.mean([l >= 4 for s, l, k in notes]),
        "len med": float(np.median([l for s, l, k in notes])),
        "repeat": sum(i == 0 for i in ivs) / n,
        "step1-2": sum(1 <= i <= 2 for i in ivs) / n,
        "4th/5th": sum(5 <= i <= 7 for i in ivs) / n,
        "oct+": sum(i >= 12 for i in ivs) / n,
        "range": max(k for *_, k in notes) - min(k for *_, k in notes),
        "rest-bars": rest_bars / bars,
        "LHL/bar": lhl / bars,
        "rep1": reps[1], "rep2": reps[2],
        "colour b2/b5/b6/7": np.mean([p in ("b2", "b5", "b6", "7") for p in pcs]),
        "major 3/6": np.mean([p in ("3", "6") for p in pcs]),
        "degrees": " ".join(sorted(set(pcs), key=NAMES.index)),
    }


def main():
    rows = []
    for p in sys.argv[1:]:
        notes = read_mid(p)
        if notes:
            rows.append((os.path.basename(p)[:28], stats(notes)))
    keys = [k for k in rows[0][1] if k != "degrees"]
    print("clip".ljust(30) + "".join(k[:9].rjust(10) for k in keys) + "  degrees")
    for name, r in rows:
        print(name.ljust(30) + "".join(f"{r[k]:10.2f}" for k in keys) + "  " + r["degrees"])
    print("MEDIAN".ljust(30) + "".join(f"{np.median([r[k] for _, r in rows]):10.2f}" for k in keys))


if __name__ == "__main__":
    main()
