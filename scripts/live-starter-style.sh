#!/usr/bin/env bash
# Assemble a playable breakbeat live style from sample banks in the Nix store.
#
# Usage: scripts/live-starter-style.sh OUT_DIR [--force] [--no-surge]
#
#   OUT_DIR     style folder to create, e.g. ~/Music/mesh-live-dev/styles/starter-breaks
#               Play it with: mesh-player --live OUT_DIR
#   --force     replace an existing style in OUT_DIR
#   --no-surge  do not set up Surge XT in ~/.clap (the bass lane's CLAP option
#               then shows as unavailable)
#
# Sample banks (copied, never symlinked, so OUT_DIR is self-contained):
#   github:switchangel/breaks       Unlicense, two-bar breaks
#   github:switchangel/pad          Unlicense, pads
#   github:tidalcycles/Dirt-Samples mixed sources: keep the style private
#
# Bank paths come from the Tidal project flake's lock ($TIDAL_FLAKE, default
# ~/Projects/tidal) when it exists, else from `nix flake prefetch` of the
# GitHub repos. The analysis and style writing is done by the
# `live-starter-style` bin (crates/mesh-core/src/bin/live_starter_style.rs).
#
# Surge XT: mesh discovers CLAP plugins only in ~/.clap, /usr/lib/clap and
# /usr/local/lib/clap (crates/mesh-core/src/clap/discovery.rs). Surge XT is
# built from the Mesh flake's nixpkgs (same glibc as the devshell; the
# system profile's build fails to dlopen there) with a GC root at
# ~/.clap/.surge-xt, and ~/.clap/"Surge XT.clap" links into it. Skipped when
# ~/.clap is itself a symlink (managed elsewhere, e.g. home-manager) or
# already holds a "Surge XT.clap" that is not this link.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out=""
force=()
surge=1
for arg in "$@"; do
  case "$arg" in
    --force) force=(--force) ;;
    --no-surge) surge=0 ;;
    -h|--help) sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option $arg" >&2; exit 2 ;;
    *) out="$arg" ;;
  esac
done
[[ -n "$out" ]] || { echo "usage: $0 OUT_DIR [--force] [--no-surge]" >&2; exit 2; }
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

breaks="$(bank swbreaks-samples switchangel/breaks)"
pads="$(bank pad-samples switchangel/pad)"
dirt="$(bank dirt-samples tidalcycles/Dirt-Samples)"
echo "breaks: $breaks"
echo "pads:   $pads"
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
"$repo/target/release/live-starter-style" --out "$out" --breaks "$breaks" --dirt "$dirt" --pads "$pads" "${force[@]}"

if [[ "$surge" == 1 ]]; then
  clap="$HOME/.clap"
  link="$clap/Surge XT.clap"
  if [[ -L "$clap" ]]; then
    echo "Surge XT: $clap is a symlink (managed elsewhere), not touched."
    echo "  Add a Surge XT CLAP bundle there yourself to enable the bass option 'surge-init'."
  elif [[ -e "$link" && ! -L "$link" ]] || { [[ -L "$link" ]] && [[ "$(readlink "$link")" != "$clap/.surge-xt/"* ]]; }; then
    echo "Surge XT: $link already exists, not touched."
  else
    mkdir -p "$clap"
    nix build --inputs-from "$repo" nixpkgs#surge-XT --out-link "$clap/.surge-xt"
    ln -sfn "$clap/.surge-xt/lib/clap/Surge XT.clap" "$link"
    echo "Surge XT: $link -> $(readlink -f "$link")"
  fi
fi
