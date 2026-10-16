"""Aggregate analysis JSONs per set: median and IQR of every metric.
Usage: compare.py [--out FILE.tsv] ANALYSIS_DIR...
A directory with JSON files is one set; a directory with only subdirectories
contributes each subdirectory as a set. Melody metrics are prefixed
other./bass.  Writes a TSV and prints a markdown table.
"""
import glob
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np

import _common as C


def load_set(d):
    rows = {}
    for p in sorted(glob.glob(os.path.join(d, "*.json"))):
        name = os.path.basename(p)[:-5]
        kind = None
        if ".melody-" in name:
            name, kind = name.rsplit(".melody-", 1)
        elif name.endswith(".develop"):
            name, kind = name[:-8], "dev"
        elif name.endswith(".melody"):
            continue
        try:
            m = json.load(open(p))["metrics"]
        except Exception:
            continue
        r = rows.setdefault(name, {})
        for k, v in m.items():
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                r[(kind + "." if kind else "") + k] = v
    return rows


def main():
    args = sys.argv[1:]
    out = os.path.join(C.REF_ROOT, "analysis", "compare.tsv")
    if args and args[0] == "--out":
        out, args = args[1], args[2:]
    sets = {}
    for d in args:
        d = d.rstrip("/")
        if glob.glob(os.path.join(d, "*.json")):
            sets[os.path.basename(d)] = load_set(d)
        else:
            for sd in sorted(glob.glob(os.path.join(d, "*/"))):
                if glob.glob(os.path.join(sd, "*.json")):
                    sets[os.path.basename(sd.rstrip("/"))] = load_set(sd)
    names = list(sets)
    keys = []
    for s in sets.values():
        for r in s.values():
            for k in r:
                if k not in keys: keys.append(k)
    ordered = [k for k in keys if "." not in k] + [k for k in keys if k.startswith("other.")] + [k for k in keys if k.startswith("bass.")] + [k for k in keys if k.startswith("dev.")]
    lines = ["metric\t" + "\t".join(f"{n}:n\t{n}:median\t{n}:q1\t{n}:q3" for n in names)]
    md = ["| metric | " + " | ".join(f"{n} (n={len(sets[n])})" for n in names) + " |", "|---|" + "---|" * len(names)]
    for k in ordered:
        tsv, cells = [k], []
        for n in names:
            v = np.array([r[k] for r in sets[n].values() if k in r], dtype=float)
            if len(v):
                q1, med, q3 = np.percentile(v, [25, 50, 75])
                tsv += [str(len(v)), f"{med:.4g}", f"{q1:.4g}", f"{q3:.4g}"]
                cells.append(f"{med:.3g} [{q1:.3g}..{q3:.3g}]" + ("" if len(v) == len(sets[n]) else f" n={len(v)}"))
            else:
                tsv += ["0", "", "", ""]; cells.append("-")
        lines.append("\t".join(tsv)); md.append(f"| {k} | " + " | ".join(cells) + " |")
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    open(out, "w").write("\n".join(lines) + "\n")
    print("\n".join(md))
    print(f"\nTSV: {out}", file=sys.stderr)


main()
