#!/usr/bin/env bash
# Assemble a playable breakbeat live style from sample folders and banks in
# the Nix store.
#
# Usage: scripts/live-starter-style.sh OUT_DIR [--force] [--no-surge]
#            [--kicks DIR --snares DIR] [--breaks DIR]
#
#   OUT_DIR       style folder to create, e.g. ~/Music/mesh-live-dev/styles/starter-breaks
#                 Play it with: mesh-player --live OUT_DIR
#   --force       replace an existing style in OUT_DIR
#   --no-surge    do not set up the Surge XT and Airwindows CLAP plugins (the
#                 reese/pad options and the drum-bus FX then show as unavailable)
#   --kicks DIR   kick one-shots; with --snares, kick n + snare n (sorted by
#   --snares DIR  name) is drum kit n. Without them: one Dirt-Samples kit.
#   --breaks DIR  break loops (1, 2 or 4 bars), one option each. Default:
#                 github:switchangel/breaks.
#
# Sample banks (copied, never symlinked, so OUT_DIR is self-contained):
#   github:switchangel/breaks       Unlicense, two-bar breaks
#   github:tidalcycles/Dirt-Samples mixed sources: keep the style private
#   Surge XT patches                reese basses and pads (`.fxp` presets)
#
# Bank paths come from the Tidal project flake's lock ($TIDAL_FLAKE, default
# ~/Projects/tidal) when it exists, else from `nix flake prefetch` of the
# GitHub repos. The analysis and style writing is done by the
# `live-starter-style` bin (crates/mesh-core/src/bin/live_starter_style.rs).
#
# CLAP plugins (Surge XT for reeses and pads, Airwindows Consolidated for the
# drum bus) are linked into <collection>/effects/clap/<arch>/ when OUT_DIR is
# <collection>/styles/<name>, else into ~/.clap. Live mode scans the style's
# collection, so a USB stick carries its own plugins; see the end of this
# script. The Surge patches are read from the same build either way.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out=""
force=()
surge=1
kicks=""
snares=""
user_breaks=""
while [[ $# -gt 0 ]]; do
  arg="$1"
  shift
  case "$arg" in
    --force) force=(--force) ;;
    --no-surge) surge=0 ;;
    --kicks) kicks="$(cd "${1:?--kicks needs a folder}" && pwd)"; shift ;;
    --snares) snares="$(cd "${1:?--snares needs a folder}" && pwd)"; shift ;;
    --breaks) user_breaks="$(cd "${1:?--breaks needs a folder}" && pwd)"; shift ;;
    -h|--help) sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option $arg" >&2; exit 2 ;;
    *) out="$arg" ;;
  esac
done
[[ -n "$out" ]] || { echo "usage: $0 OUT_DIR [--force] [--no-surge] [--kicks DIR --snares DIR] [--breaks DIR]" >&2; exit 2; }
if [[ -n "$kicks" && -z "$snares" ]] || [[ -z "$kicks" && -n "$snares" ]]; then
  echo "--kicks and --snares go together" >&2
  exit 2
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"

tidal="${TIDAL_FLAKE:-$HOME/Projects/tidal}"

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

breaks="${user_breaks:-$(bank swbreaks-samples switchangel/breaks)}"
dirt="$(bank dirt-samples tidalcycles/Dirt-Samples)"
echo "breaks: $breaks"
echo "dirt:   $dirt"

cargo_in_repo() {
  if command -v direnv >/dev/null && [[ -z "${IN_NIX_SHELL:-}" ]]; then
    direnv exec "$repo" cargo "$@"
  else
    cargo "$@"
  fi
}
if ! log="$(cargo_in_repo build --release --manifest-path "$repo/Cargo.toml" -p mesh-core --bin live-starter-style 2>&1)"; then
  echo "$log" >&2
  exit 1
fi
# Surge XT data (patches) from the Mesh flake's nixpkgs build.
surge_data="$(nix build --no-link --print-out-paths --inputs-from "$repo" nixpkgs#surge-XT)/share/surge-xt"
pairs=()
[[ -n "$kicks" ]] && pairs=(--kicks "$kicks" --snares "$snares")
"$repo/target/release/live-starter-style" --out "$out" --breaks "$breaks" --dirt "$dirt" --surge "$surge_data" "${pairs[@]}" "${force[@]}"

# CLAP plugins go into the collection the style lives in, under
# effects/clap/<arch>/, where live mode scans for them and from where USB
# export copies them (crates/mesh-core/src/clap/mod.rs
# `collection_plugin_dirs`). Styles outside a collection use ~/.clap. Built
# from the Mesh flake's nixpkgs (same glibc as the devshell), with GC roots in
# <collection>/.nix-gcroots/ (not exported).
style_parent="$(dirname "$out")"
if [[ "$(basename "$style_parent")" == styles ]]; then
  collection="$(dirname "$style_parent")"
  plugin_dir="$collection/effects/clap/$(uname -m)"
  roots="$collection/.nix-gcroots"
else
  plugin_dir="$HOME/.clap"
  roots="$HOME/.clap"
fi

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

if [[ "$surge" == 1 ]]; then
  link_clap surge-XT "Surge XT.clap"
  link_clap airwin2rack "Airwindows Consolidated.clap"
fi

# Loudness trims per option (needs the CLAP bundles for the Surge options).
cargo_in_repo build --release --manifest-path "$repo/Cargo.toml" -p mesh-core --bin style-calibrate >/dev/null 2>&1
"$repo/target/release/style-calibrate" --style "$out" --clap | tail -n 1
