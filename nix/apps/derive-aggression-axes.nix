# Derive intensity-axis variants from MuQ-MuLan's text tower.
#
# This Nix app re-uses the site-packages cache that
# `convert-muq-mulan-model` already populates (CPU or cu124). It loads the
# MuQ-MuLan checkpoint, runs the text tower over the polar-prompt sets
# baked into derive.py, and writes one JSON per variant under
# ./models/aggression-axes/.
#
# No GPU required — text tower is small (XLM-Roberta-base + 8-layer
# transformer + linear). Runs in seconds.
#
# Usage:
#   nix run .#derive-aggression-axes               # writes to ./models/aggression-axes/
#   nix run .#derive-aggression-axes -- /custom/dir
#
# Once a variant is chosen, copy it as the canonical filename so mesh-cue
# picks it up at runtime:
#   cp models/aggression-axes/V5_aggression_led.json \
#      models/muq-mulan-aggression-axis.json
#
# See documents/aggression-axis-text-tower-plan.md for the full design.
{ pkgs }:

let
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [
    pip
  ]);

  libstdcppPath = "${pkgs.stdenv.cc.cc.lib}/lib";
  zlibPath = "${pkgs.zlib}/lib";

  derivePy = ./derive-aggression-axes/derive.py;

  deriveScript = pkgs.writeShellScriptBin "derive-aggression-axes" ''
    set -euo pipefail

    export LD_LIBRARY_PATH="${libstdcppPath}:${zlibPath}:''${LD_LIBRARY_PATH:-}"

    OUTPUT_DIR="''${1:-./models/aggression-axes}"
    OUTPUT_DIR="$(realpath -m "$OUTPUT_DIR")"

    echo "╔═══════════════════════════════════════════════════════════════════════╗"
    echo "║  derive-aggression-axes  —  text-tower polar-prompt intensity axes   ║"
    echo "╚═══════════════════════════════════════════════════════════════════════╝"
    echo "Output : $OUTPUT_DIR"
    echo ""

    mkdir -p "$OUTPUT_DIR"

    # Re-use the converter's site-packages — same MuQ + transformers stack.
    # Prefer GPU build (cu124) for slightly faster text-tower forward; fall
    # back to CPU. Text tower is small enough that either is fine.
    SITE=""
    for cand in \
      "$HOME/.cache/mesh-spike/site-packages-gpu-cu124" \
      "$HOME/.cache/mesh-spike/site-packages-cpu"; do
      if [ -d "$cand/torch" ] && [ -d "$cand/muq" ]; then
        SITE="$cand"
        break
      fi
    done
    if [ -z "$SITE" ]; then
      echo "[!] No mesh-spike site-packages found." >&2
      echo "    Run \`nix run .#convert-muq-mulan-model\` first to populate the cache." >&2
      exit 1
    fi
    export PYTHONPATH="$SITE:''${PYTHONPATH:-}"
    echo "[derive] reusing deps from $SITE"

    # Pin HF caches to the same dir as the converter so we don't re-download
    # the 2.65 GB MuQ-MuLan checkpoint.
    export HF_HOME="$HOME/.cache/mesh-spike/hf"
    export HF_HUB_CACHE="$HF_HOME/hub"
    mkdir -p "$HF_HUB_CACHE"

    ${pythonEnv}/bin/python ${derivePy} "$OUTPUT_DIR"

    echo ""
    echo "════════════════════════════════════════════════════════════════════════"
    echo "Done. Variants written to $OUTPUT_DIR"
    echo ""
    echo "Compare them across your library via:"
    echo "  scripts/eval-axis-variants.sh"
    echo ""
    echo "Make a variant active for mesh-cue runtime:"
    echo "  cp $OUTPUT_DIR/<chosen>.json models/muq-mulan-aggression-axis.json"
    echo "  # then trigger Build Similarity Index in mesh-cue to refresh"
    echo "  # pca_aggression_axis with the new vector."
  '';

in deriveScript
