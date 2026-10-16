"""Print a compiled patch's non-default parameters and modulation routings.
Usage: inspect_patch.py PATCH.fxp [FILTER_REGEX]
"""
import re
import sys

import surgepy

from compile import index_params

s = surgepy.createSurge(48000)
s.loadPatch(sys.argv[1])
s.process()
pat = re.compile(sys.argv[2]) if len(sys.argv) > 2 else None
for name, par in index_params(s).items():
    if pat and not pat.search(name):
        continue
    if s.getParamVal(par) != s.getParamDef(par):
        print(f"{name} = {s.getParamDisplay(par)}")
r = s.getAllModRoutings()
for sc in r["scene"]:
    for kind in ("scene", "voice"):
        for m in sc[kind]:
            print("mod", m)
for m in r["global"]:
    print("mod", m)
