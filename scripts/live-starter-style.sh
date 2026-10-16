#!/usr/bin/env bash
# Assemble a playable breakbeat live style from sample folders and banks in
# the Nix store, in steps that can run on their own.
#
# Usage: scripts/live-starter-style.sh [STEP] OUT_DIR [--force] [--no-surge]
#            [--kicks DIR --snares DIR] [--breaks DIR] [--library DIR]
#            [--lanes A,B]
#
#   STEP (default all):
#     build      library -> style folder: samples, motifs, style.yaml. Assets
#                unchanged since the last build are not copied again and keep
#                their stored loudness. Trims are dropped: calibrate after a
#                build. Needs --force when OUT_DIR already holds a style.
#     analyse    measure each sample's loudness once and store it in style.yaml
#                (`lufs` per sample), so loading the style does not measure
#                it again. Measures only samples without a value.
#     calibrate  input and output loudness trims per option (renders every
#                option; needs the CLAP bundles for the Surge options).
#     all        build, analyse, calibrate.
#
#   OUT_DIR       style folder, e.g. ~/Music/mesh-live-dev/styles/starter-breaks
#                 Play it with: mesh-player --live OUT_DIR
#   --force       build: replace an existing style in OUT_DIR
#   --lanes A,B   analyse and calibrate only these lanes (by name); the other
#                 lanes stay as they are
#   --no-surge    do not set up the Surge XT and Airwindows CLAP plugins (the
#                 reese/pad options and the drum-bus FX then show as unavailable)
#   --kicks DIR   kick one-shots; with --snares, kick n + snare n (sorted by
#   --snares DIR  name) is drum kit n. Without them: one Dirt-Samples kit.
#   --breaks DIR  break loops (1, 2 or 4 bars), one option each. Default:
#                 the library's Breaks/, else github:switchangel/breaks.
#   --library DIR drum library (Kicks/ Snares/ Breaks/ Cymbals/ Percussion/):
#                 kicks, snares and breaks unless given, and the hats, ride,
#                 shaker, percussion and top-loop pools. Correct its sorting
#                 with DIR/sample-overrides.yaml (see OUT_DIR/sample-classes.txt).
#
# Only build reads the library, breaks and kick/snare options. Without a STEP
# the old form `OUT_DIR [--force] ...` still works and runs all.
#
# Sample banks (copied, never symlinked, so OUT_DIR is self-contained):
#   github:switchangel/breaks       Unlicense, two-bar breaks
#   github:tidalcycles/Dirt-Samples mixed sources: keep the style private
#   Surge XT patches                reese basses and pads (`.fxp` presets)
#
# Bank paths come from the Tidal project flake's lock ($TIDAL_FLAKE, default
# ~/Projects/tidal) when it exists, else from `nix flake prefetch` of the
# GitHub repos. The style writing is done by the `live-starter-style` bin
# (crates/mesh-core/src/bin/live_starter_style.rs), analysis and trims by the
# `style-calibrate` bin (crates/mesh-core/src/bin/style_calibrate.rs).
#
# CLAP plugins (Surge XT for reeses and pads, Airwindows Consolidated for the
# drum bus) are linked into <collection>/effects/clap/<arch>/ when OUT_DIR is
# <collection>/styles/<name>, else into ~/.clap, by the build step. Live mode
# scans the style's collection, so a USB stick carries its own plugins; see
# link_clap below. The Surge patches are read from the same build either way.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out=""
force=()
surge=1
kicks=""
snares=""
user_breaks=""
library=""
lanes=""
step=all
if [[ "${1:-}" =~ ^(build|analyse|calibrate|all)$ ]]; then
  step="$1"
  shift
fi
while [[ $# -gt 0 ]]; do
  arg="$1"
  shift
  case "$arg" in
    --force) force=(--force) ;;
    --no-surge) surge=0 ;;
    --lanes) lanes="${1:?--lanes needs a comma separated list}"; shift ;;
    --kicks) kicks="$(cd "${1:?--kicks needs a folder}" && pwd)"; shift ;;
    --snares) snares="$(cd "${1:?--snares needs a folder}" && pwd)"; shift ;;
    --breaks) user_breaks="$(cd "${1:?--breaks needs a folder}" && pwd)"; shift ;;
    --library) library="$(cd "${1:?--library needs a folder}" && pwd)"; shift ;;
    -h|--help) sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option $arg" >&2; exit 2 ;;
    *) out="$arg" ;;
  esac
done
[[ -n "$out" ]] || { echo "usage: $0 [build|analyse|calibrate|all] OUT_DIR [--force] [--no-surge] [--kicks DIR --snares DIR] [--breaks DIR] [--library DIR] [--lanes A,B]" >&2; exit 2; }
if [[ -n "$kicks" && -z "$snares" ]] || [[ -z "$kicks" && -n "$snares" ]]; then
  echo "--kicks and --snares go together" >&2
  exit 2
fi
if [[ "$step" != build && "$step" != all && ! -f "$out/style.yaml" ]]; then
  echo "$out holds no style: run the build step first" >&2
  exit 1
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"

tidal="${TIDAL_FLAKE:-$HOME/Projects/tidal}"

# Run a command in the repo's devshell environment (the CLAP bundles need its
# library environment).
in_repo() {
  if command -v direnv >/dev/null; then
    direnv exec "$repo" "$@"
  else
    "$@"
  fi
}

# cargo is incremental: an up to date binary costs a no-op build.
build_bin() {
  local log
  if ! log="$(in_repo cargo build --release --manifest-path "$repo/Cargo.toml" -p mesh-core --bin "$1" 2>&1)"; then
    echo "$log" >&2
    exit 1
  fi
}

# Store path of a sample bank: the Tidal flake's locked input, else a prefetch.
bank() {
  local input="$1" github="$2" path=""
  if [[ -f "$tidal/flake.lock" ]]; then
    path="$(nix eval --raw --impure --expr \
      "(builtins.getFlake \"path:$tidal\").inputs.$input.outPath" 2>/dev/null || true)"
    if [[ -n "$path" && ! -e "$path" ]]; then
      nix flake archive "path:$tidal" >/dev/null 2>&1 || true
    fi
  fi
  if [[ -z "$path" || ! -e "$path" ]]; then
    path="$(nix flake prefetch --json "github:$github" | sed -n 's/.*"storePath":"\([^"]*\)".*/\1/p')"
  fi
  [[ -n "$path" && -d "$path" ]] || { echo "cannot resolve $github" >&2; exit 1; }
  echo "$path"
}

# CLAP plugins go into the collection the style lives in, under
# effects/clap/<arch>/, where live mode scans for them and from where USB
# export copies them (crates/mesh-core/src/clap/mod.rs
# `collection_plugin_dirs`). Styles outside a collection use ~/.clap. Built
# from the Mesh flake's nixpkgs (same glibc as the devshell), with GC roots in
# <collection>/.nix-gcroots/ (not exported).
# Link one CLAP bundle (nixpkgs attribute, bundle file name) into $plugin_dir.
link_clap() {
  local attr="$1" bundle="$2"
  local link="$plugin_dir/$bundle" root="$roots/.${attr,,}"
  if [[ -L "$plugin_dir" ]]; then
    echo "$bundle: $plugin_dir is a symlink (managed elsewhere), not touched."
  elif [[ -e "$link" && ! -L "$link" ]] || { [[ -L "$link" ]] && [[ "$(readlink "$link")" != "$root/"* ]]; }; then
    echo "$bundle: $link already exists, not touched."
  else
    mkdir -p "$plugin_dir" "$roots"
    nix build --inputs-from "$repo" "nixpkgs#$attr" --out-link "$root"
    ln -sfn "$root/lib/clap/$bundle" "$link"
    echo "$bundle: $link -> $(readlink -f "$link")"
  fi
}

step_build() {
  if [[ -z "$user_breaks" && -n "$library" && -d "$library/Breaks" ]]; then
    user_breaks="$library/Breaks"
  fi
  local breaks dirt
  breaks="${user_breaks:-$(bank swbreaks-samples switchangel/breaks)}"
  dirt="$(bank dirt-samples tidalcycles/Dirt-Samples)"
  echo "breaks: $breaks"
  echo "dirt:   $dirt"

  build_bin live-starter-style
  # Surge XT data (patches) from the Mesh flake's nixpkgs build.
  local surge_data pairs=() lib=()
  surge_data="$(nix build --no-link --print-out-paths --inputs-from "$repo" nixpkgs#surge-XT)/share/surge-xt"
  [[ -n "$kicks" ]] && pairs=(--kicks "$kicks" --snares "$snares")
  [[ -n "$library" ]] && lib=(--library "$library")
  "$repo/target/release/live-starter-style" --out "$out" --breaks "$breaks" --dirt "$dirt" --surge "$surge_data" "${pairs[@]}" "${lib[@]}" "${force[@]}"

  local style_parent
  style_parent="$(dirname "$out")"
  if [[ "$(basename "$style_parent")" == styles ]]; then
    local collection
    collection="$(dirname "$style_parent")"
    plugin_dir="$collection/effects/clap/$(uname -m)"
    roots="$collection/.nix-gcroots"
  else
    plugin_dir="$HOME/.clap"
    roots="$HOME/.clap"
  fi
  if [[ "$surge" == 1 ]]; then
    link_clap surge-XT "Surge XT.clap"
    link_clap airwin2rack "Airwindows Consolidated.clap"
  fi
}

lane_args=()
[[ -n "$lanes" ]] && lane_args=(--lanes "$lanes")

# Messages worth showing from style-calibrate (grep finding none is fine).
calibrate_msgs() { grep -E "^(skipped|warning|wrote|analysed|note|timing)" || true; }

step_analyse() {
  build_bin style-calibrate
  "$repo/target/release/style-calibrate" --style "$out" --analyse "${lane_args[@]}" | calibrate_msgs
}

# Input trims, then output trims after the processing (needs the CLAP bundles
# for the Surge options).
step_calibrate() {
  build_bin style-calibrate
  in_repo "$repo/target/release/style-calibrate" --style "$out" --clap "${lane_args[@]}" | calibrate_msgs
}

# Run a step and print its wall-clock time.
timed() {
  local start=$SECONDS
  "$1"
  echo "[$1: $((SECONDS - start)) s]"
}

case "$step" in
  build) timed step_build ;;
  analyse) timed step_analyse ;;
  calibrate) timed step_calibrate ;;
  all)
    timed step_build
    timed step_analyse
    timed step_calibrate
    ;;
esac
