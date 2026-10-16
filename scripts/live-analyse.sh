#!/usr/bin/env bash
# Audio analysis of live-mode renders and reference tracks, in one persistent
# Python environment (numpy, scipy, librosa, soundfile). The environment is
# built once with `nix build` and reused from a cached result link, so a call
# does not evaluate a nix shell again.
#
# Usage: scripts/live-analyse.sh COMMAND ARGS...
#
#   ref [--out DIR] FILE...      tempo, loudness, spectrum, width, crest and 8-bar
#                                structure per track -> JSON (ref.py)
#   stems FILE...                htdemucs_ft separation into
#                                $REF/stems/<set>/, 2 parallel workers, skips done
#   melody STEM_WAV... [BPM]     pyin note statistics of an Other/Bass stem (melody.py)
#   develop [--no-images] FILE... interplay, per-8-bar development metrics and PNGs per track
#                                (needs stems, ref and melody JSON first) (develop.py)
#   compare [--out TSV] DIR...   median and IQR per set -> TSV + markdown (compare.py)
#   listen SPEC...               Music Flamingo answers on 30 s clips (server on :8001),
#                                SPEC = SET:N:drop2|drop3 or FILE:drop2|drop3 (listen.py)
#   mix FILE [BPM]               spectrum, crest, side/mid, transients, repetition per
#                                section of a render (mix.py)
#   stemspec PREFIX...           spectrum and crest of PREFIX_(*)_htdemucs_ft.wav (stemspec.py)
#   peaks FILE...                peak, RMS and crest (peaks.py)
#   pct FILE...                  RMS and peak percentiles (pct.py)
#   bandpk FILE                  peak, RMS and crest per frequency band (bandpk.py)
#   spikes FILE [BPM]            loudest 2-6 kHz peaks with bar position (spikes.py)
#   rebuild                      rebuild the Python environment
#
# <set> is the parent directory name of the file (mesh-recordings -> mesh).
# REF = ${LIVE_REF_ROOT:-~/Music/mesh-live-dev/references}; JSON goes to
# $REF/analysis/<set>/. Cache: ${LIVE_ANALYSE_CACHE:-~/.cache/mesh-live-analyse}/py.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cache="${LIVE_ANALYSE_CACHE:-$HOME/.cache/mesh-live-analyse}"
link="$cache/py"
export LIVE_REF_ROOT="${LIVE_REF_ROOT:-$HOME/Music/mesh-live-dev/references}"
# The devshell LD_LIBRARY_PATH breaks ffmpeg (glibc drift): run without it.
unset LD_LIBRARY_PATH
if ! command -v ffmpeg >/dev/null 2>&1 && [[ -e "$HOME/.local/state/nix/gcroots/audio-separator" ]]; then
  ff="$(nix-store -qR "$(readlink -f "$HOME/.local/state/nix/gcroots/audio-separator")" 2>/dev/null | grep -E "ffmpeg-[0-9.]+-bin$" | head -1)"
  [[ -n "$ff" ]] && export PATH="$ff/bin:$PATH"
fi

usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | grep '^#' | sed 's/^# \{0,1\}//'; }

build_env() {
  mkdir -p "$cache"
  nix build --impure --expr 'with import <nixpkgs> {}; python3.withPackages (p: [ p.numpy p.scipy p.librosa p.soundfile p.matplotlib ])' -o "$link"
}

set_of() { local d; d="$(basename "$(dirname "$(readlink -f "$1")")")"; [[ "$d" == mesh-recordings ]] && d=mesh; echo "$d"; }

separate_one() {
  local f="$1" set base out
  set="$(set_of "$f")"; base="$(basename "${f%.*}")"
  out="$LIVE_REF_ROOT/stems/$set"; mkdir -p "$out"
  if [[ -e "$out/${base}_(Bass)_htdemucs_ft.wav" && -e "$out/${base}_(Drums)_htdemucs_ft.wav" \
     && -e "$out/${base}_(Other)_htdemucs_ft.wav" && -e "$out/${base}_(Vocals)_htdemucs_ft.wav" ]]; then
    echo "skip $base"; return 0
  fi
  local t0=$SECONDS
  "$SEPARATOR" "$f" -m htdemucs_ft.yaml --output_format=WAV --model_file_dir "$HOME/.cache/audio-separator-models" \
    --output_dir "$out" >/dev/null 2>&1 && echo "done $base ($((SECONDS - t0)) s)" || echo "FAILED $base" >&2
}

cmd="${1:-}"
[[ -n "$cmd" ]] || { usage >&2; exit 2; }
shift
case "$cmd" in
  -h|--help) usage; exit 0 ;;
  rebuild) rm -f "$link"; build_env; exit 0 ;;
  stems)
    SEPARATOR="$(readlink -f "$HOME/.local/state/nix/gcroots/audio-separator")/bin/audio-separator"
    [[ -x "$SEPARATOR" ]] || { echo "audio-separator not found at $SEPARATOR" >&2; exit 1; }
    # GPU needs ~5 GB per worker. With less free (or LIVE_STEMS_DEVICE=cpu) run on the CPU, one worker per 8 cores.
    free_mb="$(nvidia-smi --query-gpu=memory.free --format=csv,noheader,nounits 2>/dev/null || echo 0)"
    workers=2
    if [[ "${LIVE_STEMS_DEVICE:-}" == cpu || "${free_mb:-0}" -lt 10000 ]]; then
      echo "GPU free ${free_mb:-0} MiB: separating on CPU" >&2; export CUDA_VISIBLE_DEVICES=""; workers=3
    fi
    export SEPARATOR; export -f separate_one set_of
    printf '%s\0' "$@" | xargs -0 -P "$workers" -I{} bash -c 'separate_one "$1"' _ {}
    exit 0 ;;
  ref|melody|develop|compare|listen|mix|stemspec|peaks|pct|bandpk|spikes) ;;
  *) echo "unknown command $cmd" >&2; usage >&2; exit 2 ;;
esac
[[ -x "$link/bin/python" ]] || build_env
# -I: isolated mode, no script directory or user site on the module path.
exec "$link/bin/python" -I "$here/live-analysis/$cmd.py" "$@"
