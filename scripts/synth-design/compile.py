"""Compile a declarative sound spec (YAML) into a Surge XT patch and a style
lane block.

Usage: compile.py SPEC.yaml... --out DIR

Writes DIR/<name>.fxp (the patch) and DIR/<name>.lane.yaml (the option and the
lane settings to merge into a style). Needs surgepy (source
~/.cache/mesh-synth-design/env.sh; see scripts/synth-design/README.md).

Spec (see .claude/skills/synth-design/references/spec.md for the full format):

    name: pendulum-lead-a
    style: pendulum
    role: lead                     # lane role the sound is made for
    articulation: {kind: sustained, attack_ms: 4, register: [55, 79], length_16ths: [1, 4]}
    surge:
      base: Init Wavetable         # Templates/<base>.fxp, optional (default: init)
      set:                         # any Surge parameter by its full name
        A Osc 1 Type: wavetable    # enums by name (ot_*, pm_*, sm_*, fm_*, lt_*, fut_*, wst_*, fxt_*)
        A Osc 1 Unison Voices: 7
        A Filter 1 Cutoff: 2400 Hz # units: Hz, ms, s, dB, %, st (semitones), or raw numbers
      wavetables: {A1: Basic/Sawtooth Morph.wt}   # scene+osc -> factory or absolute path
      macros: [Morph, Dirt, Move, Size]           # names of M1.. (CLAP params 0..7)
      mod:                                         # source, destination, depth (-1..1 of the range)
        - [macro1, A Filter 1 Cutoff, 0.35]
        - [lfo1, A Osc 1 Morph, 0.2]
    lane:                           # copied into the lane block
      hpf: 0                        # role high-pass in Hz for this lane (0 = off)
      fx: []                        # lane FX chain (style FxSpec list)
      controls: []                  # exactly 4 ControlSpecs
"""
import math
import os
import re
import sys

import surgepy
import yaml
from surgepy import constants as C

SURGE_ID = "org.surge-synth-team.surge-xt"

# Enum tables surgepy does not export (sst-filters / sst-waveshapers order in
# Surge XT 1.3.4, see references/surge.md).
FILTER_TYPES = {}
WAVESHAPER_TYPES = {}
OSC_TYPES = {}
FX_TYPES = {}

SOURCES = {f"macro{i + 1}": getattr(C, f"ms_ctrl{i + 1}") for i in range(8)}
SOURCES.update({f"lfo{i + 1}": getattr(C, f"ms_lfo{i + 1}") for i in range(6)})
SOURCES.update({f"slfo{i + 1}": getattr(C, f"ms_slfo{i + 1}") for i in range(6)})
SOURCES.update(
    ampeg=C.ms_ampeg,
    filtereg=C.ms_filtereg,
    velocity=C.ms_velocity,
    keytrack=C.ms_keytrack,
    modwheel=C.ms_modwheel,
    pitchbend=C.ms_pitchbend,
    random=C.ms_random_bipolar,
    random_uni=C.ms_random_unipolar,
    alternate=C.ms_alternate_bipolar,
)


def load_enum_tables():
    """Filter and waveshaper names from the generated reference, if present."""
    ref = os.path.join(os.path.dirname(__file__), "..", "..", ".claude", "skills", "synth-design", "references", "surge-enums.yaml")
    if os.path.exists(ref):
        t = yaml.safe_load(open(ref)) or {}
        FILTER_TYPES.update(t.get("filter_types", {}))
        WAVESHAPER_TYPES.update(t.get("waveshaper_types", {}))
        OSC_TYPES.update(t.get("osc_types", {}))
        FX_TYPES.update(t.get("fx_types", {}))


def index_params(s):
    """Full parameter name -> parameter object, for the whole patch."""
    out = {}

    def walk(node):
        if hasattr(node, "getId"):
            name = s.getParameterName(node.getId()).strip()
            if name:
                # Some effects reuse a name (Distortion: pre and post EQ);
                # the second one becomes "<name> (2)".
                key, k = name, 2
                while key in out:
                    key, k = f"{name} ({k})", k + 1
                out[key] = node
        elif isinstance(node, dict):
            for v in node.values():
                walk(v)
        elif isinstance(node, (list, tuple)):
            for v in node:
                walk(v)

    walk(s.getPatch())
    return out


# Parameter name pattern -> enum prefix of the surgepy constants it takes.
ENUM_KINDS = [
    (r"Play Mode$", "pm_"),
    (r"^Scene Mode$", "sm_"),
    (r"FM Routing$", "fm_"),
    (r"LFO \d+ Type$", "lt_"),
]


def enum_value(name, value):
    """An enum given by name, resolved by the parameter's kind: an oscillator
    type takes ot_ names, an LFO type lt_ names, and so on."""
    v = str(value).strip().lower().replace(" ", "_").replace("-", "_")
    tables = [
        (r"Filter \d Type$", FILTER_TYPES),
        (r"Waveshaper Type$", WAVESHAPER_TYPES),
        (r"^FX [ABSG]\d Shape$", WAVESHAPER_TYPES),
        (r"Osc \d Type$", OSC_TYPES),
        (r"FX Type$", FX_TYPES),
    ]
    for pat, table in tables:
        if re.search(pat, name):
            if v in table:
                return table[v]
            raise ValueError(f"{name}: unknown value '{value}' (one of {sorted(table)})")
    consts = {k.lower(): getattr(C, k) for k in dir(C)}
    for pat, pre in ENUM_KINDS:
        if re.search(pat, name):
            for cand in (v, pre + v):
                if cand.startswith(pre) and cand in consts:
                    return consts[cand]
            raise ValueError(f"{name}: unknown value '{value}' (one of {sorted(k[len(pre):] for k in consts if k.startswith(pre))})")
    raise ValueError(f"{name}: '{value}' is not a number and the parameter takes no names")


UNIT = re.compile(r"^\s*([-+]?\d+(?:\.\d+)?)\s*(hz|khz|ms|s|db|%|st)\s*$", re.I)


def convert(s, name, par, value):
    """A spec value in display units -> the value setParamVal takes."""
    if isinstance(value, bool):
        return 1.0 if value else 0.0
    if isinstance(value, (int, float)):
        return float(value)
    m = UNIT.match(str(value))
    if not m:
        return float(enum_value(name, value))
    x, unit = float(m.group(1)), m.group(2).lower()
    lo, hi = s.getParamMin(par), s.getParamMax(par)
    if unit in ("hz", "khz"):
        hz = x * (1000.0 if unit == "khz" else 1.0)
        if "Rate" in name:
            return math.log2(hz)  # LFO rate: log2 Hz
        return 12.0 * math.log2(hz / 440.0)  # cutoffs: semitones from A440
    if unit in ("ms", "s"):
        sec = x / 1000.0 if unit == "ms" else x
        return math.log2(max(sec, 1e-4))  # envelope / portamento times: log2 s
    if unit == "db":
        if lo == 0.0 and hi == 1.0:
            return 2.0 ** (x / 18.0)  # amplitudes: dB = 18 log2(v)
        return x
    if unit == "%":
        return x / 100.0
    return x  # st: semitones as is


def compile_spec(spec, out_dir, factory):
    load_enum_tables()
    name = spec["name"]
    sg = spec.get("surge", {})
    s = surgepy.createSurge(48000)
    base = sg.get("base")
    if base:
        path = base if os.path.isabs(base) else os.path.join(factory, "patches_factory", "Templates", base + ".fxp")
        s.loadPatch(path)
    s.process()
    params = index_params(s)

    def set_all(items):
        for key, value in items:
            if key not in params:
                close = [n for n in params if key.lower() in n.lower()][:5]
                raise KeyError(f"{name}: no Surge parameter '{key}' (similar: {close})")
            s.setParamVal(params[key], convert(s, key, params[key], value))

    # Types first (they re-initialise their parameters on the next process).
    sets = list((sg.get("set") or {}).items()) + list((sg.get("set_after_types") or {}).items())
    types = [(k, v) for k, v in sets if k.endswith(" Type") or k.endswith("Scene Mode") or k == "Scene Mode"]
    set_all(types)
    s.process()
    s.process()
    params = index_params(s)
    for key, path in (sg.get("wavetables") or {}).items():
        scene, osc = "AB".index(key[0].upper()), int(key[1]) - 1
        full = path if os.path.isabs(path) else os.path.join(factory, "wavetables", path)
        if not os.path.isfile(full) or not s.loadWavetable(scene, osc, full):
            raise ValueError(f"{name}: wavetable {full} did not load")
    set_all([(k, v) for k, v in sets if (k, v) not in types])
    for i, label in enumerate(sg.get("macros") or []):
        s.setMacro(i, str(label), False)
    for src, dst, depth in sg.get("mod") or []:
        if src not in SOURCES:
            raise KeyError(f"{name}: unknown mod source '{src}' (one of {sorted(SOURCES)})")
        if dst not in params:
            raise KeyError(f"{name}: no Surge parameter '{dst}'")
        ms = s.getModSource(SOURCES[src])
        if not s.isValidModulation(params[dst], ms):
            raise ValueError(f"{name}: {src} cannot modulate '{dst}'")
        s.setModDepth01(params[dst], ms, float(depth))
    s.process()

    os.makedirs(out_dir, exist_ok=True)
    fxp = os.path.join(out_dir, f"{name}.fxp")
    s.savePatch(fxp)

    lane = spec.get("lane") or {}
    option = {"name": name, "clap": {"plugin_id": SURGE_ID, "preset": f"assets/designs/{name}.fxp", "own_envelope": True}}
    block = {
        "option": option,
        "role": spec.get("role", "lead"),
        "hpf": lane.get("hpf"),
        "fx": lane.get("fx") or [],
        "controls": lane.get("controls") or [],
        "generator": lane.get("generator") or {},
        "articulation": spec.get("articulation") or {},
    }
    with open(os.path.join(out_dir, f"{name}.lane.yaml"), "w") as f:
        yaml.safe_dump(block, f, sort_keys=False)
    return fxp


def main():
    args = sys.argv[1:]
    if "--out" not in args or len(args) < 3:
        sys.exit(__doc__)
    i = args.index("--out")
    out_dir, specs = args[i + 1], args[:i] + args[i + 2:]
    factory = os.environ.get("SURGE_DATA_HOME") or surgepy.createSurge(48000).getFactoryDataPath()
    for path in specs:
        spec = yaml.safe_load(open(path))
        print(compile_spec(spec, out_dir, factory))


if __name__ == "__main__":
    main()
