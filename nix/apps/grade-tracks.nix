# LLM-based per-track intensity grading via NVIDIA Audio Flamingo 3.
#
# Dev-side ONLY — never bundled into mesh release artifacts. Used to build
# track-level priors that we then evaluate intensity-axis variants against
# (see documents/aggression-axis-eval-round-2.md and the task #52 tree).
#
# Pipeline:
#   1. Cargo-built `dump_track_list` writes (id,path,drop_marker,title,artist)
#      CSV to /tmp/track-grading/_track-list.csv
#   2. Python `grade.py` reads that, decodes each track around drop_marker,
#      runs AF3, persists per-track JSON (resumable)
#   3. Aggregation step writes llm-grading-raw.jsonl + llm-priors.csv
#
# Re-uses the spike's gpu-cu124 site-packages cache where compatible; AF3 is
# installed lazily on first run if not present.
#
# Usage:
#   nix run .#grade-tracks                 # full library
#   nix run .#grade-tracks -- --limit 10   # smoke test
#   nix run .#grade-tracks -- --resume     # resume after interruption
{ pkgs }:

let
  pythonEnv = pkgs.python311.withPackages (ps: with ps; [ pip ]);
  libstdcppPath = "${pkgs.stdenv.cc.cc.lib}/lib";
  zlibPath = "${pkgs.zlib}/lib";
  gradePy = ./grade-tracks/grade.py;

  gradeScript = pkgs.writeShellScriptBin "grade-tracks" ''
    set -euo pipefail

    export LD_LIBRARY_PATH="${libstdcppPath}:${zlibPath}:''${LD_LIBRARY_PATH:-}"

    OUT_DIR="/tmp/track-grading"
    REPO_ROOT="$(pwd)"
    COLLECTION="''${MESH_COLLECTION:-$HOME/Music/mesh-collection}"

    # CUDA driver visibility (NixOS)
    for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
      if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
        export LD_LIBRARY_PATH="$cand:$LD_LIBRARY_PATH"
        break
      fi
    done

    # Re-use the converter's site-packages (gpu-cu124) — has torch + transformers
    # already pinned. AF3 needs `transformers>=4.50` per its model card; we'll
    # ensure that and install librosa + soundfile if missing.
    SITE="$HOME/.cache/mesh-spike/site-packages-gpu-cu124"
    if [ ! -d "$SITE/torch" ]; then
      echo "[grade] no torch in $SITE — run \`nix run .#convert-muq-mulan-model\` first" >&2
      exit 1
    fi
    export PYTHONPATH="$SITE:''${PYTHONPATH:-}"

    # Make sure AF3 deps are present. Idempotent — pip skips if present.
    echo "[grade] ensuring AF3 / librosa / soundfile in site-packages..."
    ${pythonEnv}/bin/pip install --target "$SITE" --upgrade --no-warn-script-location \
      --index-url "https://pypi.org/simple" \
      "transformers>=4.50" "librosa>=0.10" "soundfile>=0.12" "accelerate>=0.30" \
      2>&1 | tail -5

    # Add the cu124 wheel's bundled nvidia .so dirs to LD_LIBRARY_PATH
    NVIDIA_LIBS=""
    for d in "$SITE"/nvidia/*/lib; do
      if [ -d "$d" ]; then NVIDIA_LIBS="$d:$NVIDIA_LIBS"; fi
    done
    [ -n "$NVIDIA_LIBS" ] && export LD_LIBRARY_PATH="$NVIDIA_LIBS$LD_LIBRARY_PATH"

    # HF cache — share with other spike scripts
    export HF_HOME="$HOME/.cache/mesh-spike/hf"
    export HF_HUB_CACHE="$HF_HOME/hub"
    mkdir -p "$HF_HUB_CACHE" "$OUT_DIR"

    # Step 1: dump track list via Rust binary
    DUMP_BIN="$REPO_ROOT/target/release/dump_track_list"
    if [ ! -x "$DUMP_BIN" ]; then
      echo "[grade] building dump_track_list..."
      cargo build --release -p mesh-cue --bin dump_track_list 2>&1 | tail -3
    fi
    "$DUMP_BIN" --collection "$COLLECTION" --out "$OUT_DIR/_track-list.csv"

    # Step 2: run captioner (passes args through)
    ${pythonEnv}/bin/python ${gradePy} \
      --collection "$COLLECTION" \
      --out-dir "$OUT_DIR" \
      "$@"

    echo ""
    echo "[grade] done. Outputs:"
    echo "  $OUT_DIR/llm-grading-raw.jsonl"
    echo "  $OUT_DIR/llm-priors.csv  ← feed to scripts/compare-variants.py"
  '';

in gradeScript
