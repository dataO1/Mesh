# LLM-as-judge pairwise intensity comparator (Audio Flamingo 3).
#
# Companion to grade-tracks. Where grade-tracks asks AF3 for an absolute
# 0-10 intensity score (which collapsed to 7-8 for 97% of our DnB-heavy
# library — see documents/aggression-axis-eval-round-3.md), this asks
# pairwise comparisons that sidestep absolute-scale collapse.
#
# Sampling: anchored tournament. K=3 anchors (low/mid/high) × every other
# track = 3*(N-3) pairs. Optional + N random extra pairs for BT refinement.
#
# Run:
#   nix run .#judge-pairs -- --smoke-mode             # 5 known orderings
#   nix run .#judge-pairs -- --limit-tracks 50        # quick (150 pairs)
#   nix run .#judge-pairs                             # full library (~2700 pairs)
#   nix run .#judge-pairs -- --extra-random-pairs 200 # adds BT data
{ pkgs }:

let
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [ pip ]);
  libstdcppPath = "${pkgs.stdenv.cc.cc.lib}/lib";
  zlibPath = "${pkgs.zlib}/lib";
  judgePy = ./judge-pairs/judge_pairs.py;

  judgeScript = pkgs.writeShellScriptBin "judge-pairs" ''
    set -euo pipefail

    export LD_LIBRARY_PATH="${libstdcppPath}:${zlibPath}:''${LD_LIBRARY_PATH:-}"
    for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
      if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
        export LD_LIBRARY_PATH="$cand:$LD_LIBRARY_PATH"; break
      fi
    done

    SITE="$HOME/.cache/mesh-spike/site-packages-gpu-cu124"
    if [ ! -d "$SITE/torch" ]; then
      echo "[judge] no torch — run convert-muq-mulan-model first" >&2
      exit 1
    fi
    export PYTHONPATH="$SITE:''${PYTHONPATH:-}"

    NVIDIA_LIBS=""
    for d in "$SITE"/nvidia/*/lib; do
      if [ -d "$d" ]; then NVIDIA_LIBS="$d:$NVIDIA_LIBS"; fi
    done
    [ -n "$NVIDIA_LIBS" ] && export LD_LIBRARY_PATH="$NVIDIA_LIBS$LD_LIBRARY_PATH"

    export HF_HOME="$HOME/.cache/mesh-spike/hf"
    export HF_HUB_CACHE="$HF_HOME/hub"

    OUT_DIR="/tmp/track-grading"
    COLLECTION="''${MESH_COLLECTION:-$HOME/Music/mesh-collection}"
    REPO_ROOT="$(pwd)"

    # Refresh track list (cheap)
    DUMP_BIN="$REPO_ROOT/target/release/dump_track_list"
    if [ ! -x "$DUMP_BIN" ]; then
      cargo build --release -p mesh-cue --bin dump_track_list 2>&1 | tail -3
    fi
    "$DUMP_BIN" --collection "$COLLECTION" --out "$OUT_DIR/_track-list.csv"

    # Pick judge by env var: JUDGE_MODEL=qwen for Qwen2.5-Omni, default = AF3
    JUDGE_PY="${judgePy}"
    if [ "''${JUDGE_MODEL:-af3}" = "qwen" ]; then
      JUDGE_PY="${./judge-pairs/judge_pairs_qwen.py}"
      echo "[judge] using Qwen2.5-Omni judge"
    fi
    ${pythonEnv}/bin/python "$JUDGE_PY" \
      --collection "$COLLECTION" --out-dir "$OUT_DIR" "$@"

    echo "[judge] outputs in $OUT_DIR/pairs/<a>_<b>.json"
  '';
in judgeScript
