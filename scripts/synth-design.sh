#!/usr/bin/env bash
# Sound design tools for live mode: compile declarative sound specs into Surge
# XT patches, audition them through the engine, score them against the
# reference sets. Driven by the synth-design skill (.claude/skills/synth-design).
#
# Usage: scripts/synth-design.sh COMMAND ARGS...
#
#   setup                     build surgepy (Surge XT 1.3.4 Python bindings, with
#                             wavetable and macro additions) and the Python env
#   compile SPEC... --out DIR  spec YAML -> DIR/<name>.fxp + <name>.lane.yaml
#   audition DIR NAME... --style STYLE --out OUT [...]
#                             render designs in context, solo and at morph 0/0.5/1
#   score OUT --set SET [...] measure an audition against a reference set
#   dump OUT.md               regenerate the Surge parameter reference
#   inspect PATCH.fxp [RE]    non-default parameters and modulations of a patch
#   riffs AUDITION CLIPS --out OUT  each clip alone in context, numbered, for rating
#
# Cache: ${SYNTH_DESIGN_CACHE:-~/.cache/mesh-synth-design} (surge-src/, py).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
cache="${SYNTH_DESIGN_CACHE:-$HOME/.cache/mesh-synth-design}"
surge_src_store="$(nix eval --raw nixpkgs#surge-xt.src 2>/dev/null || true)"
surge_share="$(dirname "$(dirname "$(dirname "$(readlink -f "$HOME/.clap/Surge XT.clap" 2>/dev/null || echo /x/x/x/x)")")")/share/surge-xt"

setup() {
  mkdir -p "$cache"
  nix build --impure --expr 'with import <nixpkgs> {}; python3.withPackages (p: [ p.numpy p.scipy p.pyyaml p.soundfile ])' -o "$cache/py"
  if [[ ! -e "$cache/surge-src/build/src/surge-python" ]]; then
    rm -rf "$cache/surge-src"
    cp -r "$(nix build --no-link --print-out-paths nixpkgs#surge-xt.src)" "$cache/surge-src"
    chmod -R u+w "$cache/surge-src"
    patch -d "$cache/surge-src" -p1 < "$here/synth-design/surgepy-mesh.patch"
    nix shell nixpkgs#cmake nixpkgs#ninja nixpkgs#gcc nixpkgs#pkg-config nixpkgs#git -c bash -c "
      cd '$cache/surge-src' && cmake -B build -G Ninja -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
        -DSURGE_BUILD_PYTHON_BINDINGS=ON -DSURGE_SKIP_JUCE_FOR_RACK=ON -DSURGE_BUILD_TESTRUNNER=OFF \
        -DSURGE_BUILD_FX=OFF -DSURGE_BUILD_XT=OFF -DCMAKE_BUILD_TYPE=Release \
        -DPython3_EXECUTABLE='$cache/py/bin/python3' >/dev/null && cmake --build build --target surgepy -j\$(nproc) >/dev/null"
  fi
}

[[ $# -ge 1 ]] || { sed -n '2,/^set -euo/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 2; }
cmd="$1"; shift
[[ "$cmd" == setup ]] && { setup; exit 0; }
[[ -x "$cache/py/bin/python3" && -d "$cache/surge-src/build/src/surge-python" ]] || setup

export SURGE_DATA_HOME="${SURGE_DATA_HOME:-$surge_share}"
export PYTHONPATH="$cache/surge-src/build/src/surge-python${PYTHONPATH:+:$PYTHONPATH}"
py="$cache/py/bin/python3"
case "$cmd" in
  compile) exec "$py" "$here/synth-design/compile.py" "$@" ;;
  audition) cd "$root" && exec nix develop -c "$py" "$here/synth-design/audition.py" "$@" ;;
  score) exec "${LIVE_ANALYSE_CACHE:-$HOME/.cache/mesh-live-analyse}/py/bin/python3" "$here/synth-design/score.py" "$@" ;;
  dump) exec "$py" "$here/synth-design/surge_dump.py" "$@" ;;
  inspect) cd "$here/synth-design" && exec "$py" inspect_patch.py "$@" ;;
  riffs) cd "$root" && exec nix develop -c "$py" "$here/synth-design/riff_audition.py" "$@" ;;
  *) echo "unknown command $cmd" >&2; exit 2 ;;
esac
